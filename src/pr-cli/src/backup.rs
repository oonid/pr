use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use crate::commands_extra::force_remove_dir_all;
use crate::install::unpack_entry;
use crate::oci::{link_inside_root, sanitize_layer_path};
use crate::shared::is_valid_container_name;

pub struct BackupDirs {
    pub plugins: PathBuf,
    pub legacy: PathBuf,
    pub containers: PathBuf,
}

enum Output {
    Plain(fs::File),
    Gz(flate2::write::GzEncoder<fs::File>),
    Xz(xz2::write::XzEncoder<fs::File>),
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Output::Plain(w) => w.write(buf),
            Output::Gz(w) => w.write(buf),
            Output::Xz(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Output::Plain(w) => w.flush(),
            Output::Gz(w) => w.flush(),
            Output::Xz(w) => w.flush(),
        }
    }
}

impl Output {
    fn create(path: &Path) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let name = path.to_string_lossy();
        Ok(if name.ends_with(".gz") || name.ends_with(".tgz") {
            Output::Gz(flate2::write::GzEncoder::new(
                file,
                flate2::Compression::default(),
            ))
        } else if name.ends_with(".xz") || name.ends_with(".txz") {
            Output::Xz(xz2::write::XzEncoder::new(file, 6))
        } else {
            Output::Plain(file)
        })
    }

    fn finish(self) -> std::io::Result<()> {
        let file = match self {
            Output::Plain(f) => f,
            Output::Gz(w) => w.finish()?,
            Output::Xz(w) => w.finish()?,
        };
        file.sync_all()
    }
}

pub fn write_backup(output: &Path, members: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let out = Output::create(output).map_err(|e| format!("create {}: {}", output.display(), e))?;
    let result = (|| {
        let mut builder = tar::Builder::new(out);
        builder.follow_symlinks(false);
        for (name, source) in members {
            append_tree(&mut builder, source, name)?;
        }
        builder
            .into_inner()
            .and_then(Output::finish)
            .map_err(|e| format!("write {}: {}", output.display(), e))
    })();
    if result.is_err() {
        let _ = fs::remove_file(output);
    }
    result
}

fn append_tree<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    name: &Path,
) -> Result<(), String> {
    let meta =
        fs::symlink_metadata(source).map_err(|e| format!("stat {}: {}", source.display(), e))?;
    let file_type = meta.file_type();
    let failed = |e: std::io::Error| format!("archive {}: {}", source.display(), e);
    if file_type.is_symlink() {
        let target = fs::read_link(source).map_err(failed)?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&meta);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder
            .append_link(&mut header, name, &target)
            .map_err(failed)?;
    } else if file_type.is_dir() {
        builder.append_dir(name, source).map_err(failed)?;
        let mut children: Vec<_> = fs::read_dir(source)
            .map_err(failed)?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<_, _>>()
            .map_err(failed)?;
        children.sort();
        for child in children {
            append_tree(builder, &source.join(&child), &name.join(&child))?;
        }
    } else if file_type.is_file() {
        builder
            .append_path_with_name(source, name)
            .map_err(failed)?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Legacy,
    Oci,
}

struct Placement {
    kind: Kind,
    name: String,
    in_rootfs: bool,
    relative: PathBuf,
}

enum Target {
    Skip,
    Plugin(String),
    Container(Placement),
}

fn place(path: &Path) -> Result<Target, String> {
    let clean = sanitize_layer_path(path)?;
    let parts: Vec<String> = clean
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let unexpected = || format!("unexpected entry in backup: {}", path.display());
    let Some(top) = parts.first() else {
        return Ok(Target::Skip);
    };
    match top.as_str() {
        "pr" | "proot-distro" => match parts.len() {
            1 => Ok(Target::Skip),
            2 => {
                let file = &parts[1];
                let alias = file
                    .strip_suffix(".override.sh")
                    .or_else(|| file.strip_suffix(".sh"))
                    .ok_or_else(unexpected)?;
                if !is_valid_container_name(alias) {
                    return Err(unexpected());
                }
                Ok(Target::Plugin(file.clone()))
            }
            _ => Err(unexpected()),
        },
        "installed-rootfs" | "containers" => {
            let Some(name) = parts.get(1) else {
                return Ok(Target::Skip);
            };
            if !is_valid_container_name(name) {
                return Err(unexpected());
            }
            let kind = if top == "containers" {
                Kind::Oci
            } else {
                Kind::Legacy
            };
            let rest = &parts[2..];
            let in_rootfs =
                kind == Kind::Legacy || rest.first().map(String::as_str) == Some("rootfs");
            let skip = if kind == Kind::Oci && in_rootfs { 1 } else { 0 };
            Ok(Target::Container(Placement {
                kind,
                name: name.clone(),
                in_rootfs,
                relative: rest.iter().skip(skip).collect(),
            }))
        }
        _ => Err(unexpected()),
    }
}

fn open_backup(path: &Path) -> Result<Box<dyn Read>, String> {
    let failed = |e: std::io::Error| format!("read backup {}: {}", path.display(), e);
    let mut file = fs::File::open(path).map_err(failed)?;
    let mut magic = [0u8; 6];
    let read = file.read(&mut magic).map_err(failed)?;
    file.rewind().map_err(failed)?;
    Ok(if read >= 2 && magic[..2] == [0x1f, 0x8b] {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if read >= 6 && magic == [0xfd, b'7', b'z', b'X', b'Z', 0x00] {
        Box::new(xz2::read::XzDecoder::new(file))
    } else {
        Box::new(file)
    })
}

pub fn restore_backup(archive: &Path, dirs: &BackupDirs) -> Result<Vec<String>, String> {
    let mut staged: BTreeMap<(Kind, String), PathBuf> = BTreeMap::new();
    let mut plugins: Vec<(String, Vec<u8>)> = Vec::new();
    let result = unpack_backup(archive, dirs, &mut staged, &mut plugins);
    if let Err(e) = result {
        for staging in staged.values() {
            force_remove_dir_all(staging);
        }
        return Err(e);
    }
    if staged.is_empty() {
        return Err(format!("no container in backup {}", archive.display()));
    }

    let mut restored = Vec::new();
    for ((kind, name), staging) in &staged {
        let base = if *kind == Kind::Oci {
            &dirs.containers
        } else {
            &dirs.legacy
        };
        let destination = base.join(name);
        if fs::symlink_metadata(&destination).is_ok() {
            force_remove_dir_all(&destination);
        }
        fs::rename(staging, &destination)
            .map_err(|e| format!("move {} into place: {}", name, e))?;
        restored.push(name.clone());
    }
    fs::create_dir_all(&dirs.plugins)
        .map_err(|e| format!("create {}: {}", dirs.plugins.display(), e))?;
    for (file, contents) in plugins {
        let path = dirs.plugins.join(&file);
        let tmp = dirs
            .plugins
            .join(format!(".{}.pr-cli.{}", file, std::process::id()));
        fs::write(&tmp, &contents)
            .and_then(|_| fs::rename(&tmp, &path))
            .map_err(|e| {
                let _ = fs::remove_file(&tmp);
                format!("write {}: {}", path.display(), e)
            })?;
    }
    Ok(restored)
}

fn unpack_backup(
    archive: &Path,
    dirs: &BackupDirs,
    staged: &mut BTreeMap<(Kind, String), PathBuf>,
    plugins: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("read backup {}: {}", archive.display(), e);
    let mut tar = tar::Archive::new(open_backup(archive)?);
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        let path = entry.path().map_err(failed)?.into_owned();
        let placement = match place(&path)? {
            Target::Skip => continue,
            Target::Plugin(file) => {
                if !entry.header().entry_type().is_file() {
                    return Err(format!("plugin {} is not a file", path.display()));
                }
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).map_err(failed)?;
                plugins.push((file, contents));
                continue;
            }
            Target::Container(placement) => placement,
        };

        let key = (placement.kind, placement.name.clone());
        let staging = match staged.get(&key) {
            Some(staging) => staging.clone(),
            None => {
                let base = if placement.kind == Kind::Oci {
                    &dirs.containers
                } else {
                    &dirs.legacy
                };
                let staging = base.join(format!(".{}.pr-cli-restore", placement.name));
                if fs::symlink_metadata(&staging).is_ok() {
                    force_remove_dir_all(&staging);
                }
                fs::create_dir_all(&staging)
                    .map_err(|e| format!("create {}: {}", staging.display(), e))?;
                staged.insert(key, staging.clone());
                staging
            }
        };
        let root = if placement.kind == Kind::Oci && placement.in_rootfs {
            staging.join("rootfs")
        } else {
            staging.clone()
        };
        fs::create_dir_all(&root).map_err(|e| format!("create {}: {}", root.display(), e))?;
        if placement.relative.as_os_str().is_empty() {
            continue;
        }

        if entry.header().entry_type().is_hard_link() {
            let target = entry
                .link_name()
                .map_err(failed)?
                .ok_or_else(|| format!("hardlink {} has no target", path.display()))?
                .into_owned();
            let Target::Container(target) = place(&target)? else {
                return Err(format!("hardlink {} leaves its container", path.display()));
            };
            if (target.kind, &target.name, target.in_rootfs)
                != (placement.kind, &placement.name, placement.in_rootfs)
            {
                return Err(format!("hardlink {} leaves its container", path.display()));
            }
            if !link_inside_root(&root, &placement.relative, &target.relative)? {
                return Err(format!(
                    "hardlink {} target missing {}",
                    path.display(),
                    target.relative.display()
                ));
            }
            continue;
        }
        unpack_entry(&root, &placement.relative, &mut entry)?;
    }
    Ok(())
}
