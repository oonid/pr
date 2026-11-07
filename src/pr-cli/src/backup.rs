use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use crate::commands_extra::force_remove_dir_all;
use crate::install::unpack_entry;
use crate::install_marker::InstallMarker;
use crate::oci::{decompressed_reader, link_inside_root, sanitize_layer_path};
use crate::shared::is_valid_container_name;

pub struct BackupDirs {
    pub staging: PathBuf,
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
        let name = path.to_string_lossy().to_ascii_lowercase();
        let unsupported = [
            ".bz2", ".tbz", ".tbz2", ".zst", ".tzst", ".lz4", ".lz", ".lzma", ".z",
        ];
        if unsupported.iter().any(|suffix| name.ends_with(suffix)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsupported compression; use .tar, .tar.gz or .tar.xz",
            ));
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_tree_skips_a_file_that_vanished() {
        let missing = std::env::temp_dir().join(format!(
            "pr-cli-backup-vanished-{}-does-not-exist",
            std::process::id()
        ));
        let mut builder = tar::Builder::new(Vec::new());
        append_tree(&mut builder, &missing, Path::new("x")).expect("skip vanished file");
        let bytes = builder.into_inner().expect("finish");
        assert!(bytes.iter().all(|b| *b == 0));
    }

    #[test]
    fn move_into_place_releases_every_name_before_deleting_the_old_copies() {
        let base = std::env::temp_dir().join(format!(
            "pr-cli-backup-move-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let dirs = BackupDirs {
            staging: base.join("staging"),
            plugins: base.join("plugins"),
            legacy: base.join("legacy"),
            containers: base.join("containers"),
        };
        let mut staged = BTreeMap::new();
        for name in ["a", "b"] {
            fs::create_dir_all(dirs.containers.join(name).join("rootfs/old")).expect("current");
            let staging = dirs.staging.join(format!("oci-{}", name));
            fs::create_dir_all(staging.join("rootfs/new")).expect("staged");
            staged.insert(
                (Kind::Oci, name.to_string()),
                Staged {
                    root: staging.join("rootfs"),
                    staging,
                },
            );
        }

        let mut replaced = Vec::new();

        let restored = move_into_place(&dirs, &mut staged, &[], &mut replaced).expect("move");

        assert_eq!(restored, ["a", "b"]);
        let olds: Vec<PathBuf> = ["a", "b"]
            .iter()
            .map(|name| dirs.staging.join(format!("oci-{}.old", name)))
            .collect();
        assert_eq!(replaced, olds);
        for (name, old) in ["a", "b"].iter().zip(&olds) {
            assert!(dirs.containers.join(name).join("rootfs/new").is_dir());
            assert!(!crate::install_marker::has_marker(&dirs.containers, name));
            assert!(old.join("rootfs/old").is_dir());
        }
        let _ = fs::remove_dir_all(base);
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
    match append_entry(builder, source, name) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("archive {}: {}", source.display(), e)),
        Ok(None) => Ok(()),
        Ok(Some(children)) => {
            for child in children {
                append_tree(builder, &source.join(&child), &name.join(&child))?;
            }
            Ok(())
        }
    }
}

fn append_entry<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    name: &Path,
) -> std::io::Result<Option<Vec<std::ffi::OsString>>> {
    let meta = fs::symlink_metadata(source)?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target = fs::read_link(source)?;
        let mut header = tar::Header::new_gnu();
        header.set_metadata(&meta);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        builder.append_link(&mut header, name, &target)?;
    } else if file_type.is_dir() {
        let mut children: Vec<_> = fs::read_dir(source)?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<_, _>>()?;
        children.sort();
        builder.append_dir(name, source)?;
        return Ok(Some(children));
    } else if file_type.is_file() {
        builder.append_path_with_name(source, name)?;
    }
    Ok(None)
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

struct Counted {
    inner: Box<dyn Read>,
    count: Rc<Cell<u64>>,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.count.set(self.count.get() + read as u64);
        Ok(read)
    }
}

fn open_backup(path: &Path, count: Rc<Cell<u64>>) -> Result<Counted, String> {
    let failed = |e: String| format!("read backup {}: {}", path.display(), e);
    let file = fs::File::open(path).map_err(|e| failed(e.to_string()))?;
    let inner = decompressed_reader(file).map_err(failed)?;
    Ok(Counted { inner, count })
}

struct Staged {
    staging: PathBuf,
    root: PathBuf,
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Legacy => "legacy",
        Kind::Oci => "oci",
    }
}

fn plugin_alias(file: &str) -> &str {
    file.strip_suffix(".override.sh")
        .or_else(|| file.strip_suffix(".sh"))
        .unwrap_or(file)
}

pub fn restore_backup(archive: &Path, dirs: &BackupDirs) -> Result<Vec<String>, String> {
    let mut staged: BTreeMap<(Kind, String), Staged> = BTreeMap::new();
    let mut plugins: Vec<(String, Vec<u8>)> = Vec::new();
    let mut replaced: Vec<PathBuf> = Vec::new();
    let result = unpack_backup(archive, dirs, &mut staged, &mut plugins)
        .and_then(|_| check_plugins(&staged, &plugins))
        .and_then(|_| move_into_place(dirs, &mut staged, &plugins, &mut replaced));
    for entry in staged.values() {
        force_remove_dir_all(&entry.staging);
    }
    for old in &replaced {
        force_remove_dir_all(old);
    }
    let _ = fs::remove_dir(&dirs.staging);
    result
}

fn check_plugins(
    staged: &BTreeMap<(Kind, String), Staged>,
    plugins: &[(String, Vec<u8>)],
) -> Result<(), String> {
    if staged.is_empty() {
        return Err("no container in backup".to_string());
    }
    for (file, _) in plugins {
        if !staged.contains_key(&(Kind::Legacy, plugin_alias(file).to_string())) {
            return Err(format!("plugin {} has no container in the backup", file));
        }
    }
    Ok(())
}

fn release(markers: impl IntoIterator<Item = InstallMarker>) {
    for marker in markers {
        if !marker.took_over() {
            let _ = marker.finish();
        }
    }
}

fn move_into_place(
    dirs: &BackupDirs,
    staged: &mut BTreeMap<(Kind, String), Staged>,
    plugins: &[(String, Vec<u8>)],
    replaced: &mut Vec<PathBuf>,
) -> Result<Vec<String>, String> {
    let mut restored = Vec::new();
    let keys: Vec<(Kind, String)> = staged.keys().cloned().collect();
    let base_of = |kind: Kind| {
        if kind == Kind::Oci {
            &dirs.containers
        } else {
            &dirs.legacy
        }
    };
    let mut markers = Vec::new();
    for (kind, name) in &keys {
        let base = base_of(*kind);
        let claimed = fs::create_dir_all(base)
            .map_err(|e| format!("create {}: {}", base.display(), e))
            .and_then(|_| InstallMarker::claim_reporting(base, name));
        match claimed {
            Ok(marker) => markers.push(marker),
            Err(e) => {
                release(markers);
                return Err(e);
            }
        }
    }
    let mut markers = markers.into_iter();
    for key in keys {
        let marker = markers.next().expect("one marker per container");
        let (kind, name) = &key;
        let base = base_of(*kind);
        let destination = base.join(name);
        let old = dirs
            .staging
            .join(format!("{}-{}.old", kind_label(*kind), name));
        if fs::symlink_metadata(&old).is_ok() {
            force_remove_dir_all(&old);
        }
        let had_old = fs::symlink_metadata(&destination).is_ok();
        if had_old {
            if let Err(e) = fs::rename(&destination, &old) {
                release(std::iter::once(marker).chain(markers));
                return Err(format!("move the current {} aside: {}", name, e));
            }
        }
        let staging = staged[&key].staging.clone();
        if let Err(e) = fs::rename(&staging, &destination) {
            if had_old {
                let _ = fs::rename(&old, &destination);
            }
            release(std::iter::once(marker).chain(markers));
            return Err(format!("move {} into place: {}", name, e));
        }
        if had_old {
            replaced.push(old);
        }
        if let Err(e) = marker.finish() {
            release(markers);
            return Err(e);
        }
        staged.remove(&key);
        restored.push(name.clone());
    }
    fs::create_dir_all(&dirs.plugins)
        .map_err(|e| format!("create {}: {}", dirs.plugins.display(), e))?;
    for (file, contents) in plugins {
        let path = dirs.plugins.join(file);
        let tmp = dirs
            .plugins
            .join(format!(".{}.pr-cli.{}", file, std::process::id()));
        fs::write(&tmp, contents)
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
    staged: &mut BTreeMap<(Kind, String), Staged>,
    plugins: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("read backup {}: {}", archive.display(), e);
    let count = Rc::new(Cell::new(0u64));
    let mut tar = tar::Archive::new(open_backup(archive, count.clone())?);
    let mut entries_end = 0u64;
    for entry in tar.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        entries_end = entry.raw_file_position() + entry.size().div_ceil(512) * 512;
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
        if !staged.contains_key(&key) {
            let staging =
                dirs.staging
                    .join(format!("{}-{}", kind_label(placement.kind), placement.name));
            if fs::symlink_metadata(&staging).is_ok() {
                force_remove_dir_all(&staging);
            }
            let root = if placement.kind == Kind::Oci {
                staging.join("rootfs")
            } else {
                staging.clone()
            };
            fs::create_dir_all(&root).map_err(|e| format!("create {}: {}", root.display(), e))?;
            staged.insert(key.clone(), Staged { staging, root });
        }
        let root = if placement.kind == Kind::Oci && !placement.in_rootfs {
            staged[&key].staging.clone()
        } else {
            staged[&key].root.clone()
        };
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
            if !link_inside_root(&root, &placement.relative, &target.relative, false)? {
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
    if count.get() < entries_end + 512 {
        return Err(format!(
            "backup {} ends before its end-of-archive marker; it may be truncated",
            archive.display()
        ));
    }
    Ok(())
}
