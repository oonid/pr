use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use libc;

use crate::backup::{restore_backup, write_backup, BackupDirs};
use crate::install_marker::{has_marker, InstallMarker};
use crate::install_model::load_oci_install_metadata;
use crate::plugin::{load_plugins, DistroPlugin};
use crate::shared::{
    container_parent_dir, get_download_cache_dir, get_installed_rootfs_dir, get_oci_container_dir,
    get_oci_container_manifest_path, get_oci_containers_dir, get_plugins_dir,
    is_valid_container_name, msg_error, msg_status, pending_container, resolve_installed_rootfs,
    InstalledSourceType,
};

/// Remove a directory tree, handling 0000-permission proot bind-mount stubs.
///
/// proot creates empty placeholder dirs (apex, odm, product, sdcard, system,
/// system_ext, vendor) with mode 0000.  A plain recursive remove bails on them because
/// it tries `opendir()` which requires execute permission.  `libc::rmdir()` only
/// needs write permission on the *parent*, so it succeeds even on 0000-perms
/// empty dirs.  For non-empty dirs we chmod 0700 first, then recurse.
pub(crate) fn force_remove_dir_all(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_dir()) {
        let _ = fs::remove_file(path);
        return;
    }
    // Fast path: try rmdir first.  For empty dirs (even 0000-perms) this works
    // because the kernel only checks write permission on the parent directory.
    if let Ok(cstr) = CString::new(path.as_os_str().as_bytes()) {
        let ret = unsafe { libc::rmdir(cstr.as_ptr()) };
        if ret == 0 {
            return;
        }
    }

    // rmdir failed — directory is non-empty or we can't stat it.
    // chmod 0700 so we can list and enter it.
    if let Ok(cstr) = CString::new(path.as_os_str().as_bytes()) {
        unsafe {
            libc::chmod(cstr.as_ptr(), 0o700);
        }
    }

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            match fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => force_remove_dir_all(&p),
                _ => {
                    if let Ok(cstr) = CString::new(p.as_os_str().as_bytes()) {
                        unsafe {
                            libc::unlink(cstr.as_ptr());
                        }
                    }
                }
            }
        }
    }

    // rmdir after children are cleared
    if let Ok(cstr) = CString::new(path.as_os_str().as_bytes()) {
        unsafe {
            libc::rmdir(cstr.as_ptr());
        }
    }
}

pub fn command_remove(distro_name: &str, is_reset: bool) -> Result<(), String> {
    let plugins_dir = get_plugins_dir();
    let (rootfs, source_type, marker) = match resolve_installed_rootfs(distro_name) {
        Some((rootfs, source_type)) => (rootfs, source_type, None),
        None => match pending_container(distro_name) {
            Some((dir, source_type, _)) => {
                let parent = container_parent_dir(source_type);
                let marker = InstallMarker::claim_reporting(Path::new(&parent), distro_name)?;
                if !marker.took_over() {
                    marker.finish()?;
                    msg_error(&format!(
                        "the install of '{}' finished meanwhile; run remove again to remove it.",
                        distro_name
                    ));
                    return Err("installed meanwhile".to_string());
                }
                (dir, source_type, Some(marker))
            }
            None => {
                println!();
                msg_error(&format!("distribution '{}' is not installed.", distro_name));
                println!();
                return Err("not installed".to_string());
            }
        },
    };

    if !is_reset && source_type == InstalledSourceType::Legacy {
        let override_path = format!("{}/{}.override.sh", plugins_dir, distro_name);
        if Path::new(&override_path).exists() {
            msg_status(&format!("Deleting file '{}'...", override_path));
            let _ = fs::remove_file(&override_path);
        }
    }

    let plugins = load_plugins(Path::new(&plugins_dir));
    let plugin = plugins.iter().find(|p| p.alias == distro_name);
    let display_name = plugin.map(|p| p.name.as_str()).unwrap_or(distro_name);
    msg_status(&format!("Wiping the rootfs of {}...", display_name));

    let install_path = if source_type == InstalledSourceType::Oci {
        get_oci_container_dir(distro_name)
    } else {
        rootfs.clone()
    };

    let rootfs_path = Path::new(&install_path);
    if rootfs_path.exists() {
        force_remove_dir_all(rootfs_path);
    }
    if rootfs_path.exists() {
        return Err(format!("failed to fully remove path '{}'", install_path));
    }
    if let Some(marker) = marker {
        marker.finish()?;
    }

    msg_status("Finished.");
    Ok(())
}

pub fn command_reset(distro_name: &str) -> Result<(), String> {
    let Some((_rootfs, source_type)) = resolve_installed_rootfs(distro_name) else {
        println!();
        msg_error(&format!("distribution '{}' is not installed.", distro_name));
        println!();
        return Err("not installed".to_string());
    };

    if source_type == InstalledSourceType::Oci {
        let metadata_path = get_oci_container_manifest_path(distro_name);
        let metadata = load_oci_install_metadata(Path::new(&metadata_path)).map_err(|e| {
            msg_error(&format!(
                "cannot reset OCI install '{}' because metadata is unreadable: {}",
                distro_name, e
            ));
            e
        })?;
        command_remove(distro_name, true)?;
        return crate::install::command_install(
            &metadata.original_source_reference,
            Some(distro_name),
            None,
            None,
        );
    }

    let plugins_dir = get_plugins_dir();

    let plugins = load_plugins(Path::new(&plugins_dir));
    if !plugins.iter().any(|p| p.alias == distro_name)
        && crate::catalog::catalog_entry(distro_name).is_none()
    {
        println!();
        msg_error(&format!(
            "unknown distribution '{}' was requested to be reset.",
            distro_name
        ));
        println!();
        return Err("unknown distribution".to_string());
    }
    if !plugins.iter().any(|p| p.alias == distro_name) {
        if let Some(entry) = crate::catalog::catalog_entry(distro_name) {
            if Path::new(&get_oci_container_dir(distro_name)).exists() {
                msg_error(&format!(
                    "an OCI container named '{}' already exists; remove it before resetting the legacy one.",
                    distro_name
                ));
                return Err("name taken".to_string());
            }
            msg_status(&format!(
                "Reinstalling '{}' from {}.",
                distro_name, entry.reference
            ));
        }
    }

    command_remove(distro_name, true)?;

    crate::install::command_install(distro_name, None, None, None)?;

    Ok(())
}

pub fn command_clear_cache() -> Result<(), String> {
    let cache_dir = get_download_cache_dir();

    if !Path::new(&cache_dir).is_dir() {
        msg_status("Download cache is empty.");
        msg_status("Finished.");
        return Ok(());
    }

    let entries: Vec<_> = fs::read_dir(&cache_dir)
        .map_err(|e| format!("read cache dir: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .collect();

    if entries.is_empty() {
        msg_status("Download cache is empty.");
        msg_status("Finished.");
        return Ok(());
    }

    let total_size: u64 = entries
        .iter()
        .filter_map(|e| e.metadata().ok().map(|m| m.len()))
        .sum();
    let size_str = if total_size >= 1024 * 1024 {
        format!("{:.1}MB", total_size as f64 / (1024.0 * 1024.0))
    } else if total_size >= 1024 {
        format!("{:.1}KB", total_size as f64 / 1024.0)
    } else {
        format!("{}B", total_size)
    };

    msg_status("Clearing cache files...");

    for entry in &entries {
        msg_status(&format!("Deleting '{}'", entry.path().display()));
        let _ = fs::remove_file(entry.path());
    }

    msg_status(&format!("Reclaimed {} of disk space.", size_str));
    msg_status("Finished.");
    Ok(())
}

fn chmod_readable_recursive(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    let mode = meta.permissions().mode();
    if meta.is_dir() {
        if mode & 0o500 != 0o500 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o500));
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                chmod_readable_recursive(&entry.path());
            }
        }
    } else if meta.is_file() && mode & 0o400 == 0 {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o400));
    }
}

fn render_override_plugin(plugin: &DistroPlugin, alias: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("DISTRO_NAME=\"{} - {}\"", plugin.name, alias));
    if let Some(comment) = &plugin.comment {
        lines.push(format!("DISTRO_COMMENT=\"{}\"", comment));
    }
    let mut arches: Vec<&String> = plugin.tarballs.keys().collect();
    arches.sort();
    for arch in arches {
        if let Some(tb) = plugin.tarballs.get(arch) {
            lines.push(format!("TARBALL_URL_{}=\"{}\"", arch, tb.url));
            lines.push(format!("TARBALL_SHA256_{}=\"{}\"", arch, tb.sha256));
        }
    }
    lines.join("\n") + "\n"
}

pub fn command_backup(distro_name: &str, output_path: Option<&str>) -> Result<(), String> {
    let plugins_dir = get_plugins_dir();
    let Some((rootfs, source_type)) = resolve_installed_rootfs(distro_name) else {
        println!();
        msg_error(&format!("distribution '{}' is not installed.", distro_name));
        return Err("not installed".to_string());
    };

    let plugins = load_plugins(Path::new(&plugins_dir));
    let plugin = plugins.iter().find(|p| p.alias == distro_name);
    let display_name = plugin.map(|p| p.name.as_str()).unwrap_or(distro_name);

    let output = match output_path {
        Some(p) => p.to_string(),
        None => {
            msg_error("tarball output path is not specified. Use --output <path>.");
            return Err("no output path".to_string());
        }
    };

    if Path::new(&output).is_dir() {
        msg_error(&format!(
            "cannot write to '{}' because this path is a directory.",
            output
        ));
        return Err("output is directory".to_string());
    }
    if Path::new(&output).exists() {
        msg_error(&format!(
            "file '{}' already exists. Please specify a different name.",
            output
        ));
        return Err("output exists".to_string());
    }

    msg_status(&format!("Backing up {}...", display_name));
    msg_status(&format!("Tarball will be written to '{}'.", output));

    msg_status("Fixing file permissions in rootfs...");
    chmod_readable_recursive(Path::new(&rootfs));

    let mut members: Vec<(PathBuf, PathBuf)> = Vec::new();
    if source_type == InstalledSourceType::Legacy {
        for plugin_file in [
            format!("{}.override.sh", distro_name),
            format!("{}.sh", distro_name),
        ] {
            let source = Path::new(&plugins_dir).join(&plugin_file);
            if source.is_file() {
                members.push((Path::new("pr").join(&plugin_file), source));
                break;
            }
        }
        msg_status("Archiving the rootfs and plug-in...");
        members.push((
            Path::new("installed-rootfs").join(distro_name),
            PathBuf::from(&rootfs),
        ));
    } else {
        msg_status("Archiving the OCI container directory...");
        members.push((
            Path::new("containers").join(distro_name),
            PathBuf::from(get_oci_container_dir(distro_name)),
        ));
    }

    if let Err(e) = write_backup(Path::new(&output), &members) {
        msg_error("Backup failed.");
        return Err(e);
    }

    msg_status("Finished.");
    Ok(())
}

pub fn command_restore(tarball_path: &str) -> Result<(), String> {
    let installed_rootfs_dir = get_installed_rootfs_dir();
    let oci_containers_dir = get_oci_containers_dir();
    let plugins_dir = get_plugins_dir();

    if !Path::new(tarball_path).exists() {
        msg_error(&format!("file '{}' does not exist.", tarball_path));
        return Err("file not found".to_string());
    }
    if Path::new(tarball_path).is_dir() {
        msg_error(&format!("path '{}' is a directory.", tarball_path));
        return Err("path is directory".to_string());
    }

    msg_status("Restoring containers from the tarball...");
    let dirs = BackupDirs {
        staging: Path::new(&installed_rootfs_dir)
            .parent()
            .unwrap_or(Path::new("/"))
            .join(".restore"),
        plugins: PathBuf::from(&plugins_dir),
        legacy: PathBuf::from(&installed_rootfs_dir),
        containers: PathBuf::from(&oci_containers_dir),
    };
    match restore_backup(Path::new(tarball_path), &dirs) {
        Ok(restored) => {
            for name in restored {
                msg_status(&format!("Restored '{}'.", name));
            }
        }
        Err(e) => {
            msg_error("Restore failed.");
            return Err(e);
        }
    }

    msg_status("Finished.");
    Ok(())
}

pub fn command_rename(old_alias: &str, new_alias: &str) -> Result<(), String> {
    let installed_rootfs_dir = get_installed_rootfs_dir();
    let plugins_dir = get_plugins_dir();

    if old_alias == new_alias {
        msg_error("the original and new distribution aliases should not be same.");
        return Err("same alias".to_string());
    }

    if new_alias.is_empty() {
        msg_error("the new alias of distribution should not be empty.");
        return Err("empty alias".to_string());
    }
    if !is_valid_container_name(new_alias) {
        msg_error(
            "the new alias should start with an alphanumeric character and consist of alphanumeric characters including symbols '_.+-'.",
        );
        return Err("invalid alias".to_string());
    }
    if new_alias.ends_with(".sh") {
        msg_error("the new alias should not end with '.sh'.");
        return Err("alias ends with .sh".to_string());
    }

    let plugins = load_plugins(Path::new(&plugins_dir));

    if has_marker(Path::new(&installed_rootfs_dir), old_alias) {
        msg_error(&format!(
            "'{}' was not fully installed; install it again or remove it.",
            old_alias
        ));
        return Err("not fully installed".to_string());
    }

    let old_rootfs = format!("{}/{}", installed_rootfs_dir, old_alias);
    if !Path::new(&old_rootfs).is_dir()
        && Path::new(&get_oci_container_dir(old_alias))
            .join("rootfs")
            .is_dir()
    {
        msg_error(&format!(
            "'{}' is an OCI container; rename handles legacy containers only.",
            old_alias
        ));
        return Err("not a legacy container".to_string());
    }
    if !Path::new(&old_rootfs).is_dir() {
        println!();
        msg_error(&format!(
            "cannot rename because the distribution '{}' is not installed.",
            old_alias
        ));
        return Err("not installed".to_string());
    }

    let new_rootfs = format!("{}/{}", installed_rootfs_dir, new_alias);
    if Path::new(&new_rootfs).is_dir()
        || Path::new(&get_oci_container_dir(new_alias)).exists()
        || pending_container(new_alias).is_some()
    {
        msg_error(&format!(
            "cannot rename because rootfs directory for '{}' already exists.",
            new_alias
        ));
        return Err("target exists".to_string());
    }

    let new_override = format!("{}/{}.override.sh", plugins_dir, new_alias);
    if Path::new(&new_override).exists() || plugins.iter().any(|p| p.alias == new_alias) {
        msg_error(&format!(
            "distribution with alias '{}' already exists.",
            new_alias
        ));
        return Err("alias exists".to_string());
    }

    msg_status(&format!("Renaming '{}' to '{}'...", old_alias, new_alias));

    let target = InstallMarker::claim_reporting(Path::new(&installed_rootfs_dir), new_alias)?;
    if target.took_over() || Path::new(&new_rootfs).exists() {
        if !target.took_over() {
            target.finish()?;
        }
        msg_error(&format!(
            "cannot rename because '{}' is in use by an install.",
            new_alias
        ));
        return Err("target exists".to_string());
    }
    let renamed = fs::rename(&old_rootfs, &new_rootfs).map_err(|e| format!("rename rootfs: {}", e));
    target.finish()?;
    renamed?;

    let old_plugin = format!("{}/{}.sh", plugins_dir, old_alias);
    let old_override = format!("{}/{}.override.sh", plugins_dir, old_alias);
    if Path::new(&old_override).exists() {
        fs::rename(&old_override, &new_override)
            .map_err(|e| format!("rename override plugin: {}", e))?;
    } else {
        let plugin = plugins.iter().find(|p| p.alias == old_alias);
        if let Some(p) = plugin {
            fs::create_dir_all(&plugins_dir).map_err(|e| format!("create plugins dir: {}", e))?;
            fs::write(&new_override, render_override_plugin(p, new_alias))
                .map_err(|e| format!("write override plugin: {}", e))?;
        } else if Path::new(&old_plugin).exists() {
            fs::copy(&old_plugin, &new_override).map_err(|e| format!("copy plugin: {}", e))?;
        }
    }

    msg_status("Finished.");
    Ok(())
}

fn copy_times(meta: &fs::Metadata, path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let times = [
        libc::timespec {
            tv_sec: meta.atime() as libc::time_t,
            tv_nsec: meta.atime_nsec() as _,
        },
        libc::timespec {
            tv_sec: meta.mtime() as libc::time_t,
            tv_nsec: meta.mtime_nsec() as _,
        },
    ];
    let c_path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let ret = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn copy_tree(source: &Path, destination: &Path, failures: &mut Vec<String>) {
    use std::os::unix::fs::FileTypeExt;
    let result = (|| -> std::io::Result<()> {
        let meta = fs::symlink_metadata(source)?;
        let file_type = meta.file_type();
        if !file_type.is_dir() {
            if let Ok(existing) = fs::symlink_metadata(destination) {
                if existing.is_dir() {
                    return Err(std::io::Error::other("cannot overwrite a directory"));
                }
                fs::remove_file(destination)?;
            }
        }
        if file_type.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        } else if file_type.is_dir() {
            if !destination.is_dir() {
                fs::create_dir(destination)?;
            }
            fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
            match fs::read_dir(source) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(entry) => copy_tree(
                                &entry.path(),
                                &destination.join(entry.file_name()),
                                failures,
                            ),
                            Err(e) => failures.push(format!("{}: {}", source.display(), e)),
                        }
                    }
                }
                Err(e) => failures.push(format!("{}: {}", source.display(), e)),
            }
            fs::set_permissions(destination, meta.permissions())?;
        } else if file_type.is_file() {
            fs::copy(source, destination)?;
        } else if file_type.is_fifo() {
            let path =
                CString::new(destination.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
            let mode = (meta.permissions().mode() & 0o7777) as libc::mode_t;
            if unsafe { libc::mkfifo(path.as_ptr(), mode) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            fs::set_permissions(destination, meta.permissions())?;
        } else {
            return Err(std::io::Error::other(
                "sockets and device files are not copied",
            ));
        }
        let _ = copy_times(&meta, destination);
        Ok(())
    })();
    if let Err(e) = result {
        failures.push(format!("{}: {}", source.display(), e));
    }
}

fn resolved_target(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path;
    let mut rest = Vec::new();
    while fs::symlink_metadata(existing).is_err() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent;
            }
            _ => break,
        }
    }
    let mut resolved = fs::canonicalize(existing)?;
    for name in rest.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

pub fn command_copy(src: &str, dst: &str) -> Result<(), String> {
    let (src_dist, src_path) = parse_dist_path(src);
    let (dst_dist, dst_path) = parse_dist_path(dst);

    let src_full = resolve_path(src_dist.as_deref(), &src_path)?;
    let dst_full = resolve_path(dst_dist.as_deref(), &dst_path)?;

    if !Path::new(&src_full).exists() {
        msg_error(&format!(
            "can't copy '{}' because file does not exist.",
            src
        ));
        return Err("source not found".to_string());
    }

    msg_status(&format!("Source: '{}'", src_full));
    msg_status(&format!("Destination: '{}'", dst_full));

    msg_status("Copying files, this may take a while...");

    let source = Path::new(&src_full);
    let mut destination = PathBuf::from(&dst_full);
    if destination.is_dir() {
        if let Some(name) = source.file_name() {
            destination.push(name);
        }
    }
    let real_source = fs::canonicalize(source).map_err(|e| format!("resolve source: {}", e))?;
    let real_destination =
        resolved_target(&destination).map_err(|e| format!("resolve destination: {}", e))?;
    if real_destination == real_source {
        msg_error("source and destination are the same file.");
        return Err("same file".to_string());
    }
    if real_source.is_dir() && real_destination.starts_with(&real_source) {
        msg_error("cannot copy a directory into itself.");
        return Err("copy into itself".to_string());
    }
    if let Some(parent) = destination.parent() {
        if !parent.exists() {
            msg_status(&format!("Creating directory '{}'...", parent.display()));
            fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
    }
    let mut failures = Vec::new();
    copy_tree(source, &destination, &mut failures);
    if !failures.is_empty() {
        for failure in &failures {
            msg_error(failure);
        }
        msg_error("Copy failed for the entries above; the rest was copied.");
        return Err(format!("copy: {} entries failed", failures.len()));
    }

    msg_status("Finished.");
    Ok(())
}

fn parse_dist_path(input: &str) -> (Option<String>, String) {
    if let Some(pos) = input.find(':') {
        let dist = &input[..pos];
        let path = &input[pos + 1..];
        if !dist.is_empty() && !path.is_empty() {
            return (Some(dist.to_string()), path.to_string());
        }
    }
    (None, input.to_string())
}

fn resolve_path(dist: Option<&str>, path: &str) -> Result<String, String> {
    if let Some(d) = dist {
        let Some((rootfs, _source_type)) = resolve_installed_rootfs(d) else {
            msg_error(&format!("distribution '{}' is not installed.", d));
            return Err("distro not installed".to_string());
        };
        let full = format!("{}/{}", rootfs, path);
        let canonical = Path::new(&full)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(&full).to_path_buf());
        Ok(canonical.to_string_lossy().to_string())
    } else {
        let canonical = Path::new(path)
            .canonicalize()
            .unwrap_or_else(|_| Path::new(path).to_path_buf());
        Ok(canonical.to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_marker::{pending_install, BEING_INSTALLED};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static std::sync::Mutex<()> {
        crate::shared::global_test_env_lock()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos))
    }

    #[test]
    fn parse_dist_path_splits_only_valid_dist_prefix() {
        assert_eq!(
            parse_dist_path("debian:/etc/os-release"),
            (Some("debian".to_string()), "/etc/os-release".to_string())
        );
        assert_eq!(
            parse_dist_path("/tmp/file.txt"),
            (None, "/tmp/file.txt".to_string())
        );
        assert_eq!(parse_dist_path("debian:"), (None, "debian:".to_string()));
    }

    #[test]
    fn resolve_path_without_dist_keeps_missing_path_string() {
        let path = "/this/path/should/not/exist-for-pr-cli";
        let resolved = resolve_path(None, path).expect("resolve path");
        assert_eq!(resolved, path);
    }

    #[test]
    fn resolve_path_with_missing_dist_returns_error() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-resolve");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = resolve_path(Some("debian"), "/etc/os-release")
            .expect_err("must fail when distro is not installed");
        assert_eq!(err, "distro not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_clear_cache_ok_when_cache_dir_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-clear-missing");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let result = command_clear_cache();
        assert!(result.is_ok());

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_clear_cache_deletes_cached_files() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-clear-files");
        let prefix = tmp_dir.join("usr");
        let cache_dir = prefix.join("var/lib/pr/dlcache");
        fs::create_dir_all(&cache_dir).expect("create cache dir");
        fs::write(cache_dir.join("one.tar.xz"), b"a").expect("write cache file");
        fs::write(cache_dir.join("two.tar.xz"), b"bb").expect("write cache file");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        command_clear_cache().expect("clear cache");

        let remaining = fs::read_dir(&cache_dir)
            .expect("read cache dir")
            .filter_map(|e| e.ok())
            .count();
        assert_eq!(remaining, 0);

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_backup_requires_output_path_for_installed_distro() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-backup-no-output");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(&rootfs).expect("create installed rootfs");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_backup("debian", None).expect_err("must require output path");
        assert_eq!(err, "no output path");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_backup_rejects_output_directory() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-backup-output-dir");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        let out_dir = tmp_dir.join("output-dir");
        fs::create_dir_all(&rootfs).expect("create installed rootfs");
        fs::create_dir_all(&out_dir).expect("create output dir");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_backup("debian", Some(out_dir.to_str().expect("output dir path")))
            .expect_err("must reject output directory");
        assert_eq!(err, "output is directory");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_restore_rejects_missing_tarball() {
        let err = command_restore("/path/that/does/not/exist.tar.xz")
            .expect_err("must reject missing tarball");
        assert_eq!(err, "file not found");
    }

    #[test]
    fn command_restore_rejects_directory_path() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-restore-dir");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let err = command_restore(tmp_dir.to_str().expect("tmp dir path"))
            .expect_err("must reject directory path");
        assert_eq!(err, "path is directory");

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_rename_rejects_same_alias() {
        let err = command_rename("debian", "debian").expect_err("must reject same alias");
        assert_eq!(err, "same alias");
    }

    #[test]
    fn command_rename_rejects_a_new_alias_that_is_a_path() {
        for alias in ["a/../../x", "a/b", "a..b", "a b", "a\\b"] {
            assert_eq!(
                command_rename("debian", alias).expect_err("path alias must fail"),
                "invalid alias",
                "{}",
                alias
            );
        }
    }

    fn mtime_of(path: &Path) -> i64 {
        use std::os::unix::fs::MetadataExt;
        fs::symlink_metadata(path).expect("stat").mtime()
    }

    #[test]
    fn command_copy_copies_a_tree_like_cp_a() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-tree");
        let src = tmp_dir.join("src");
        fs::create_dir_all(src.join("bin")).expect("create src");
        fs::write(src.join("bin/tool"), b"\x7fELF").expect("write tool");
        fs::set_permissions(src.join("bin/tool"), fs::Permissions::from_mode(0o750))
            .expect("chmod tool");
        std::os::unix::fs::symlink("bin/tool", src.join("link")).expect("symlink");
        let old = libc::timespec {
            tv_sec: 1_000_000_000,
            tv_nsec: 0,
        };
        let times = [old, old];
        let c = std::ffi::CString::new(src.join("bin/tool").to_str().unwrap()).unwrap();
        unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) };
        let dst = tmp_dir.join("dst");

        command_copy(src.to_str().unwrap(), dst.to_str().unwrap()).expect("copy");

        assert_eq!(fs::read(dst.join("bin/tool")).unwrap(), b"\x7fELF");
        assert_eq!(mode_of(&dst.join("bin/tool")), 0o750);
        assert_eq!(mtime_of(&dst.join("bin/tool")), 1_000_000_000);
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            Path::new("bin/tool")
        );

        let into = tmp_dir.join("into");
        fs::create_dir_all(&into).expect("create into");
        command_copy(src.to_str().unwrap(), into.to_str().unwrap()).expect("copy into dir");
        assert_eq!(fs::read(into.join("src/bin/tool")).unwrap(), b"\x7fELF");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_refuses_to_copy_onto_the_source() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-onto-itself");
        let proj = tmp_dir.join("proj");
        fs::create_dir_all(&proj).expect("create proj");
        fs::write(proj.join("data"), b"keep").expect("seed");

        assert!(command_copy(proj.to_str().unwrap(), tmp_dir.to_str().unwrap()).is_err());
        assert!(command_copy(
            proj.join("data").to_str().unwrap(),
            proj.join("data").to_str().unwrap()
        )
        .is_err());

        assert_eq!(fs::read(proj.join("data")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_refuses_to_copy_a_directory_into_itself() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-into-itself");
        let a = tmp_dir.join("a");
        fs::create_dir_all(&a).expect("create a");
        fs::write(a.join("f"), b"x").expect("seed");

        assert!(command_copy(a.to_str().unwrap(), a.join("b").to_str().unwrap()).is_err());

        assert!(!a.join("b").exists());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_replaces_symlinks_when_copying_again() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-again");
        let src = tmp_dir.join("src");
        fs::create_dir_all(&src).expect("create src");
        fs::write(src.join("f"), b"x").expect("seed");
        std::os::unix::fs::symlink("f", src.join("link")).expect("symlink");
        let parent = tmp_dir.join("out");
        fs::create_dir_all(&parent).expect("create out");

        command_copy(src.to_str().unwrap(), parent.to_str().unwrap()).expect("first copy");
        command_copy(src.to_str().unwrap(), parent.to_str().unwrap()).expect("second copy");

        assert_eq!(
            fs::read_link(parent.join("src/link")).unwrap(),
            Path::new("f")
        );
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_copies_what_it_can_and_reports_the_rest() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-partial");
        let src = tmp_dir.join("src");
        fs::create_dir_all(src.join("locked")).expect("create locked");
        fs::write(src.join("locked/secret"), b"x").expect("seed secret");
        for name in ["a", "m", "z"] {
            fs::write(src.join(name), name).expect("seed file");
        }
        fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o000))
            .expect("chmod locked");
        let dst = tmp_dir.join("dst");

        let result = command_copy(src.to_str().unwrap(), dst.to_str().unwrap());

        fs::set_permissions(src.join("locked"), fs::Permissions::from_mode(0o755))
            .expect("chmod back");
        assert!(result.is_err());
        for name in ["a", "m", "z"] {
            assert_eq!(fs::read(dst.join(name)).unwrap(), name.as_bytes());
        }
        assert_eq!(mode_of(&dst.join("locked")), 0o000);
        fs::set_permissions(dst.join("locked"), fs::Permissions::from_mode(0o755)).ok();
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_recreates_fifos() {
        let tmp_dir = unique_temp_dir("pr-cli-copy-fifo");
        let src = tmp_dir.join("src");
        fs::create_dir_all(&src).expect("create src");
        let fifo = std::ffi::CString::new(src.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o640) }, 0);
        let dst = tmp_dir.join("dst");

        command_copy(src.to_str().unwrap(), dst.to_str().unwrap()).expect("copy");

        use std::os::unix::fs::FileTypeExt;
        let meta = fs::symlink_metadata(dst.join("pipe")).expect("stat pipe");
        assert!(meta.file_type().is_fifo());
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn force_remove_dir_all_removes_names_that_are_not_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let tmp_dir = unique_temp_dir("pr-cli-force-remove-bytes");
        let dir = tmp_dir.join("tree");
        let odd = dir.join(std::ffi::OsStr::from_bytes(b"bad\xff"));
        fs::create_dir_all(&odd).expect("create odd dir");
        fs::write(odd.join(std::ffi::OsStr::from_bytes(b"f\xfe")), b"x").expect("seed");

        force_remove_dir_all(&dir);

        assert!(fs::symlink_metadata(&dir).is_err());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_rejects_missing_source() {
        let err = command_copy("/tmp/does-not-exist", "/tmp/destination-file")
            .expect_err("must reject missing source path");
        assert_eq!(err, "source not found");
    }

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn chmod_readable_recursive_only_adds_owner_read_and_search() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-chmod-readable");
        let nested = tmp_dir.join("nested");
        let unreadable = nested.join("unreadable");
        let program = nested.join("program");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(&unreadable, b"hello").expect("write file");
        fs::write(&program, b"\x7fELF").expect("write program");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o200)).expect("chmod");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("chmod");
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o300)).expect("chmod");

        chmod_readable_recursive(&tmp_dir);

        assert_eq!(mode_of(&nested), 0o700);
        assert_eq!(mode_of(&unreadable), 0o600);
        assert_eq!(mode_of(&program), 0o755);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn chmod_readable_recursive_does_not_follow_symlinks() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-chmod-symlink");
        let outside = tmp_dir.join("outside");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&outside).expect("create outside");
        fs::create_dir_all(&rootfs).expect("create rootfs");
        fs::write(outside.join("file"), b"x").expect("write file");
        fs::set_permissions(outside.join("file"), fs::Permissions::from_mode(0o200))
            .expect("chmod file");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o300)).expect("chmod dir");
        std::os::unix::fs::symlink(outside.join("file"), rootfs.join("file-link"))
            .expect("symlink file");
        std::os::unix::fs::symlink(&outside, rootfs.join("dir-link")).expect("symlink dir");

        chmod_readable_recursive(&rootfs);

        assert_eq!(mode_of(&outside), 0o300);
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).expect("chmod dir");
        assert_eq!(mode_of(&outside.join("file")), 0o200);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn force_remove_dir_all_removes_non_empty_tree() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-force-remove");
        let tree = tmp_dir.join("tree/sub");
        fs::create_dir_all(&tree).expect("create tree");
        fs::write(tree.join("payload.txt"), b"payload").expect("write file");

        force_remove_dir_all(&tmp_dir.join("tree"));
        assert!(!tmp_dir.join("tree").exists());

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_remove_and_reset_report_not_installed() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-not-installed");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let remove_err = command_remove("debian", false).expect_err("remove should fail");
        assert_eq!(remove_err, "not installed");

        let reset_err = command_reset("debian").expect_err("reset should fail");
        assert_eq!(reset_err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_rename_rejects_empty_or_invalid_new_alias() {
        assert_eq!(
            command_rename("debian", "").expect_err("empty alias must fail"),
            "empty alias"
        );
        assert_eq!(
            command_rename("debian", ".hidden").expect_err("invalid alias must fail"),
            "invalid alias"
        );
        assert_eq!(
            command_rename("debian", "name.sh").expect_err("suffix .sh must fail"),
            "alias ends with .sh"
        );
    }

    #[test]
    fn command_backup_rejects_existing_output_file() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-backup-existing-file");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        let output = tmp_dir.join("backup.tar.xz");
        fs::create_dir_all(&rootfs).expect("create installed rootfs");
        fs::write(&output, b"exists").expect("create output file");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_backup("debian", Some(output.to_str().expect("output path")))
            .expect_err("must reject existing output file");
        assert_eq!(err, "output exists");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn parse_dist_path_treats_empty_prefix_as_local_path() {
        assert_eq!(
            parse_dist_path(":/etc/os-release"),
            (None, ":/etc/os-release".to_string())
        );
    }

    #[test]
    fn command_backup_reports_not_installed_when_rootfs_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-backup-not-installed");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_backup("debian", Some("/tmp/out.tar.xz"))
            .expect_err("backup should fail without installed rootfs");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_rename_reports_not_installed_with_builtin_distribution_catalog() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-rename-unknown");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(prefix.join("etc/pr")).expect("create plugins dir");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_rename("debian", "debian-new")
            .expect_err("rename should fail when distro rootfs is absent");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_rename_reports_not_installed_when_rootfs_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-rename-not-installed");
        let prefix = tmp_dir.join("usr");
        let plugins_dir = prefix.join("etc/pr");
        fs::create_dir_all(&plugins_dir).expect("create plugins dir");
        fs::write(
            plugins_dir.join("debian.sh"),
            "DISTRO_NAME=\"Debian\"\nTARBALL_URL_aarch64=\"https://example.invalid/debian.tar.xz\"\nTARBALL_SHA256_aarch64=\"abc\"\n",
        )
        .expect("write plugin");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_rename("debian", "debian-new")
            .expect_err("rename should fail without installed rootfs");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_copy_with_distro_prefix_reports_missing_distro() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-copy-missing-distro");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_copy("debian:/etc/passwd", "/tmp/out")
            .expect_err("copy should fail for missing distro");
        assert_eq!(err, "distro not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn reset_keeps_a_legacy_container_when_its_catalog_name_is_taken() {
        let env = TestPrefix::new("pr-cli-reset-catalog-taken");
        seed_rootfs(&env.legacy().join("debian"));
        seed_rootfs(&env.containers().join("debian/rootfs"));

        assert!(command_reset("debian").is_err());

        assert_rootfs(&env.legacy().join("debian"));
        assert_rootfs(&env.containers().join("debian/rootfs"));
    }

    #[test]
    fn rename_refuses_a_name_another_container_has() {
        let env = TestPrefix::new("pr-cli-rename-taken");
        seed_rootfs(&env.legacy().join("mydistro"));
        seed_rootfs(&env.containers().join("box/rootfs"));

        assert!(command_rename("mydistro", "box").is_err());

        assert_rootfs(&env.legacy().join("mydistro"));
        assert!(!env.legacy().join("box").exists());
    }

    #[test]
    fn rename_says_it_handles_legacy_containers_only() {
        let env = TestPrefix::new("pr-cli-rename-oci");
        seed_rootfs(&env.containers().join("box/rootfs"));

        let err = command_rename("box", "box2").expect_err("must refuse");
        assert_eq!(err, "not a legacy container");
    }

    #[test]
    fn remove_deletes_an_interrupted_install_and_its_marker() {
        let env = TestPrefix::new("pr-cli-remove-interrupted");
        drop(InstallMarker::claim(&env.legacy(), "deb").expect("mark legacy"));
        fs::create_dir_all(env.legacy().join("deb/etc")).expect("partial legacy");
        drop(InstallMarker::claim(&env.containers(), "box").expect("mark oci"));
        fs::create_dir_all(env.containers().join("box/rootfs/etc")).expect("partial rootfs");
        drop(InstallMarker::claim(&env.containers(), "early").expect("mark early"));

        command_remove("deb", false).expect("remove legacy");
        command_remove("box", false).expect("remove oci");
        command_remove("early", false).expect("remove a marker without a directory");

        assert!(!env.legacy().join("deb").exists());
        assert!(!env.containers().join("box").exists());
        for (parent, name) in [
            (env.legacy(), "deb"),
            (env.containers(), "box"),
            (env.containers(), "early"),
        ] {
            assert_eq!(pending_install(&parent, name), None, "{}", name);
        }
    }

    #[test]
    fn remove_refuses_a_container_that_is_being_installed() {
        let env = TestPrefix::new("pr-cli-remove-running");
        let running = InstallMarker::claim(&env.containers(), "box").expect("mark");
        fs::create_dir_all(env.containers().join("box/rootfs")).expect("rootfs");

        let err = command_remove("box", false).expect_err("must refuse");

        assert_eq!(err, BEING_INSTALLED);
        assert!(env.containers().join("box/rootfs").exists());
        drop(running);
    }

    #[test]
    fn remove_keeps_a_container_whose_install_finishes_while_it_waits() {
        let env = TestPrefix::new("pr-cli-remove-finished-meanwhile");
        let running = InstallMarker::claim(&env.containers(), "box").expect("mark");
        seed_rootfs(&env.containers().join("box/rootfs"));
        let installer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            running.finish().expect("finish");
        });

        let result = command_remove("box", false);

        installer.join().expect("join");
        assert!(result.is_err());
        assert_rootfs(&env.containers().join("box/rootfs"));
        assert_eq!(pending_install(&env.containers(), "box"), None);
    }

    #[test]
    fn rename_refuses_an_interrupted_legacy_install() {
        let env = TestPrefix::new("pr-cli-rename-interrupted");
        seed_rootfs(&env.legacy().join("mydistro"));
        drop(InstallMarker::claim(&env.legacy(), "mydistro").expect("mark"));

        let err = command_rename("mydistro", "other").expect_err("must refuse");

        assert_eq!(err, "not fully installed");
        assert!(env.legacy().join("mydistro").exists());
        assert!(!env.legacy().join("other").exists());
    }

    #[test]
    fn rename_ignores_an_oci_marker_for_the_same_name() {
        let env = TestPrefix::new("pr-cli-rename-oci-marker");
        seed_rootfs(&env.legacy().join("mydistro"));
        drop(InstallMarker::claim(&env.containers(), "mydistro").expect("mark"));

        command_rename("mydistro", "other").expect("rename");

        assert_rootfs(&env.legacy().join("other"));
    }

    #[test]
    fn rename_refuses_a_new_name_with_an_unfinished_install() {
        let env = TestPrefix::new("pr-cli-rename-onto-pending");
        seed_rootfs(&env.legacy().join("mydistro"));
        drop(InstallMarker::claim(&env.legacy(), "other").expect("mark"));

        assert!(command_rename("mydistro", "other").is_err());

        assert_rootfs(&env.legacy().join("mydistro"));
        assert!(!env.legacy().join("other").exists());
    }

    #[test]
    fn rename_moves_a_legacy_container_that_has_no_plugin() {
        let env = TestPrefix::new("pr-cli-rename-no-plugin");
        seed_rootfs(&env.legacy().join("mydistro"));

        command_rename("mydistro", "mydistro-old").expect("rename");

        assert!(!env.legacy().join("mydistro").exists());
        assert_rootfs(&env.legacy().join("mydistro-old"));
    }

    #[test]
    fn command_restore_reports_a_file_that_is_not_a_backup() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-restore-uninspectable");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let tarball = tmp_dir.join("backup.tar.xz");
        fs::write(&tarball, b"not-a-real-tarball").expect("write file");

        let err = command_restore(tarball.to_str().expect("tarball path"))
            .expect_err("restore should fail to read the tarball");
        assert!(err.contains("backup"), "{}", err);

        let _ = fs::remove_dir_all(tmp_dir);
    }

    struct TestPrefix {
        _guard: std::sync::MutexGuard<'static, ()>,
        base: PathBuf,
        prefix: PathBuf,
    }

    impl TestPrefix {
        fn new(label: &str) -> Self {
            let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
            let base = unique_temp_dir(label);
            let prefix = base.join("usr");
            fs::create_dir_all(prefix.join("etc/pr")).expect("create plugins dir");
            std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
            TestPrefix {
                _guard: guard,
                base,
                prefix,
            }
        }

        fn containers(&self) -> PathBuf {
            self.prefix.join("var/lib/pr/containers")
        }

        fn legacy(&self) -> PathBuf {
            self.prefix.join("var/lib/pr/installed-rootfs")
        }
    }

    impl Drop for TestPrefix {
        fn drop(&mut self) {
            std::env::remove_var("APP_PREFIX");
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn seed_rootfs(rootfs: &Path) {
        fs::create_dir_all(rootfs.join("bin")).expect("create bin");
        fs::create_dir_all(rootfs.join("usr/lib")).expect("create usr/lib");
        fs::write(rootfs.join("bin/sh"), b"\x7fELFsh").expect("write sh");
        fs::set_permissions(rootfs.join("bin/sh"), fs::Permissions::from_mode(0o755))
            .expect("chmod sh");
        fs::write(rootfs.join("usr/lib/os-release"), b"ID=test\n").expect("write os-release");
        std::os::unix::fs::symlink("usr/lib", rootfs.join("lib")).expect("symlink lib");
    }

    fn assert_rootfs(rootfs: &Path) {
        assert_eq!(
            fs::read(rootfs.join("bin/sh")).expect("read sh"),
            b"\x7fELFsh"
        );
        assert_eq!(mode_of(&rootfs.join("bin/sh")), 0o755);
        assert_eq!(
            fs::read_link(rootfs.join("lib")).expect("read link"),
            Path::new("usr/lib")
        );
        assert_eq!(
            fs::read(rootfs.join("lib/os-release")).expect("read through link"),
            b"ID=test\n"
        );
    }

    #[test]
    fn backup_then_restore_round_trips_an_oci_container() {
        let env = TestPrefix::new("pr-cli-backup-oci");
        let container = env.containers().join("box");
        seed_rootfs(&container.join("rootfs"));
        fs::write(container.join("manifest.json"), b"{}").expect("write manifest");
        let output = env.base.join("box.tar.gz");

        command_backup("box", Some(output.to_str().unwrap())).expect("backup");
        assert_eq!(&fs::read(&output).expect("read backup")[..2], &[0x1f, 0x8b]);
        force_remove_dir_all(&container);
        command_restore(output.to_str().unwrap()).expect("restore");

        assert_rootfs(&container.join("rootfs"));
        assert_eq!(fs::read(container.join("manifest.json")).unwrap(), b"{}");
    }

    fn backup_of_a_good_box(env: &TestPrefix) -> PathBuf {
        let container = env.containers().join("box");
        seed_rootfs(&container.join("rootfs"));
        let output = env.base.join("box.tar.gz");
        command_backup("box", Some(output.to_str().unwrap())).expect("backup");
        force_remove_dir_all(&container);
        output
    }

    #[test]
    fn restore_over_an_interrupted_install_clears_its_marker() {
        let env = TestPrefix::new("pr-cli-restore-interrupted");
        let output = backup_of_a_good_box(&env);
        drop(InstallMarker::claim(&env.containers(), "box").expect("mark"));
        fs::create_dir_all(env.containers().join("box/rootfs/etc")).expect("partial rootfs");

        command_restore(output.to_str().unwrap()).expect("restore");

        assert_rootfs(&env.containers().join("box/rootfs"));
        assert_eq!(pending_install(&env.containers(), "box"), None);
        assert!(resolve_installed_rootfs("box").is_some());
    }

    #[test]
    fn restore_moves_nothing_when_one_of_its_containers_is_being_installed() {
        let env = TestPrefix::new("pr-cli-restore-claims-first");
        let output = env.base.join("two.tar.gz");
        write_gz_backup(
            &output,
            &[
                ("containers/a/rootfs/f", Some(b"new")),
                ("containers/b/rootfs/f", Some(b"new")),
            ],
        );
        fs::create_dir_all(env.containers().join("a/rootfs")).expect("existing a");
        fs::write(env.containers().join("a/rootfs/f"), b"old").expect("old a");
        let running = InstallMarker::claim(&env.containers(), "b").expect("mark b");

        assert!(command_restore(output.to_str().unwrap()).is_err());

        assert_eq!(
            fs::read(env.containers().join("a/rootfs/f")).unwrap(),
            b"old"
        );
        assert_eq!(pending_install(&env.containers(), "a"), None);
        drop(running);
    }

    #[test]
    fn a_refused_restore_keeps_an_interrupted_install_interrupted() {
        let env = TestPrefix::new("pr-cli-restore-keeps-interrupted");
        let output = env.base.join("two.tar.gz");
        write_gz_backup(
            &output,
            &[
                ("containers/a/rootfs/f", Some(b"new")),
                ("containers/b/rootfs/f", Some(b"new")),
            ],
        );
        drop(InstallMarker::claim(&env.containers(), "a").expect("mark a"));
        let running = InstallMarker::claim(&env.containers(), "b").expect("mark b");

        assert!(command_restore(output.to_str().unwrap()).is_err());

        assert_eq!(
            pending_install(&env.containers(), "a"),
            Some(crate::install_marker::PendingInstall::Interrupted)
        );
        drop(running);
    }

    #[test]
    fn restore_refuses_a_container_that_is_being_installed() {
        let env = TestPrefix::new("pr-cli-restore-running");
        let output = backup_of_a_good_box(&env);
        let running = InstallMarker::claim(&env.containers(), "box").expect("mark");
        fs::create_dir_all(env.containers().join("box/rootfs/in-progress")).expect("rootfs");

        assert!(command_restore(output.to_str().unwrap()).is_err());

        assert!(env.containers().join("box/rootfs/in-progress").is_dir());
        drop(running);
    }

    #[test]
    fn backup_then_restore_round_trips_a_legacy_container_with_its_plugin() {
        let env = TestPrefix::new("pr-cli-backup-legacy");
        seed_rootfs(&env.legacy().join("old"));
        fs::write(env.prefix.join("etc/pr/old.sh"), b"DISTRO_NAME=\"Old\"\n").expect("plugin");
        let output = env.base.join("old.tar.xz");

        command_backup("old", Some(output.to_str().unwrap())).expect("backup");
        assert_eq!(
            &fs::read(&output).expect("read backup")[..6],
            b"\xfd7zXZ\x00"
        );
        force_remove_dir_all(&env.legacy().join("old"));
        fs::remove_file(env.prefix.join("etc/pr/old.sh")).expect("remove plugin");
        command_restore(output.to_str().unwrap()).expect("restore");

        assert_rootfs(&env.legacy().join("old"));
        assert_eq!(
            fs::read(env.prefix.join("etc/pr/old.sh")).unwrap(),
            b"DISTRO_NAME=\"Old\"\n"
        );
    }

    fn write_gz_backup(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
        let file = fs::File::create(path).expect("create backup");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, content) in entries {
            let mut header = tar::Header::new_gnu();
            let gnu = header.as_gnu_mut().expect("gnu header");
            gnu.name[..name.len()].copy_from_slice(name.as_bytes());
            match content {
                Some(data) => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_mode(0o644);
                    header.set_size(data.len() as u64);
                    header.set_cksum();
                    builder.append(&header, *data).expect("append file");
                }
                None => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_mode(0o755);
                    header.set_size(0);
                    header.set_cksum();
                    builder
                        .append(&header, std::io::empty())
                        .expect("append dir");
                }
            }
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gz");
    }

    #[test]
    fn restore_accepts_an_upstream_proot_distro_backup() {
        let env = TestPrefix::new("pr-cli-restore-upstream");
        let output = env.base.join("up.tar.gz");
        write_gz_backup(
            &output,
            &[
                ("proot-distro/up.sh", Some(b"DISTRO_NAME=\"Up\"\n")),
                ("installed-rootfs/up/", None),
                ("installed-rootfs/up/etc/", None),
                ("installed-rootfs/up/etc/os-release", Some(b"ID=up\n")),
            ],
        );

        command_restore(output.to_str().unwrap()).expect("restore");

        assert_eq!(
            fs::read(env.legacy().join("up/etc/os-release")).unwrap(),
            b"ID=up\n"
        );
        assert!(env.prefix.join("etc/pr/up.sh").is_file());
    }

    fn write_gz_backup_with_hardlink(path: &Path, file: &str, link: &str, target: &str) {
        let out = fs::File::create(path).expect("create backup");
        let encoder = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o755);
        header.set_size(4);
        header.set_cksum();
        builder
            .append_data(&mut header, file, &b"perl"[..])
            .expect("append file");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Link);
        header.set_size(0);
        header.set_cksum();
        builder
            .append_link(&mut header, link, target)
            .expect("append hardlink");
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gz");
    }

    #[test]
    fn restore_recreates_a_hardlink_inside_its_container() {
        let env = TestPrefix::new("pr-cli-restore-hardlink");
        let output = env.base.join("up.tar.gz");
        write_gz_backup_with_hardlink(
            &output,
            "installed-rootfs/up/usr/bin/perl",
            "installed-rootfs/up/usr/bin/perl5",
            "installed-rootfs/up/usr/bin/perl",
        );

        command_restore(output.to_str().unwrap()).expect("restore");

        assert_eq!(
            fs::read(env.legacy().join("up/usr/bin/perl5")).unwrap(),
            b"perl"
        );
    }

    #[test]
    fn restore_refuses_a_hardlink_into_another_container() {
        let env = TestPrefix::new("pr-cli-restore-hardlink-other");
        let output = env.base.join("up.tar.gz");
        write_gz_backup_with_hardlink(
            &output,
            "installed-rootfs/one/secret",
            "installed-rootfs/two/stolen",
            "installed-rootfs/one/secret",
        );

        assert!(command_restore(output.to_str().unwrap()).is_err());
        assert!(!env.legacy().join("two").exists());
        assert!(!env.legacy().join("one").exists());
    }

    #[test]
    fn restore_refuses_a_plain_tar_without_its_end_marker() {
        let env = TestPrefix::new("pr-cli-restore-truncated");
        seed_rootfs(&env.containers().join("box/rootfs"));
        let output = env.base.join("box.tar");
        command_backup("box", Some(output.to_str().unwrap())).expect("backup");
        let bytes = fs::read(&output).expect("read backup");
        assert!(bytes[bytes.len() - 1024..].iter().all(|b| *b == 0));
        fs::write(&output, &bytes[..bytes.len() - 1024]).expect("truncate");
        fs::write(env.containers().join("box/rootfs/keep"), b"data").expect("seed keep");

        assert!(command_restore(output.to_str().unwrap()).is_err());
        assert_eq!(
            fs::read(env.containers().join("box/rootfs/keep")).unwrap(),
            b"data"
        );
    }

    #[test]
    fn restore_keeps_the_existing_container_when_moving_it_aside_fails() {
        let env = TestPrefix::new("pr-cli-restore-move-fails");
        let rootfs = env.containers().join("box/rootfs");
        seed_rootfs(&rootfs);
        let output = env.base.join("box.tar.gz");
        command_backup("box", Some(output.to_str().unwrap())).expect("backup");
        fs::write(rootfs.join("keep"), b"data").expect("seed keep");
        fs::set_permissions(env.containers(), fs::Permissions::from_mode(0o555))
            .expect("chmod containers");

        let result = command_restore(output.to_str().unwrap());

        fs::set_permissions(env.containers(), fs::Permissions::from_mode(0o755))
            .expect("chmod containers back");
        assert!(result.is_err());
        assert_eq!(fs::read(rootfs.join("keep")).unwrap(), b"data");
        assert_rootfs(&rootfs);
        assert!(resolve_installed_rootfs("box").is_some());
    }

    #[test]
    fn restore_replaces_a_symlinked_container_without_touching_its_target() {
        let env = TestPrefix::new("pr-cli-restore-symlink-destination");
        seed_rootfs(&env.legacy().join("old"));
        let output = env.base.join("old.tar.gz");
        command_backup("old", Some(output.to_str().unwrap())).expect("backup");
        let elsewhere = env.base.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("create elsewhere");
        fs::write(elsewhere.join("precious"), b"data").expect("seed precious");
        force_remove_dir_all(&env.legacy().join("old"));
        std::os::unix::fs::symlink(&elsewhere, env.legacy().join("old")).expect("symlink");

        command_restore(output.to_str().unwrap()).expect("restore");

        assert_eq!(fs::read(elsewhere.join("precious")).unwrap(), b"data");
        assert!(!fs::symlink_metadata(env.legacy().join("old"))
            .unwrap()
            .is_symlink());
        assert_rootfs(&env.legacy().join("old"));
    }

    #[test]
    fn restore_over_an_installed_container_deletes_the_replaced_copy() {
        let env = TestPrefix::new("pr-cli-restore-replaced");
        let output = backup_of_a_good_box(&env);
        fs::create_dir_all(env.containers().join("box/rootfs/stale")).expect("current box");

        command_restore(output.to_str().unwrap()).expect("restore");

        assert_rootfs(&env.containers().join("box/rootfs"));
        assert!(!env.containers().join("box/rootfs/stale").exists());
        assert!(fs::symlink_metadata(env.prefix.join("var/lib/pr/.restore")).is_err());
    }

    #[test]
    fn force_remove_dir_all_removes_a_symlink_without_following_it() {
        let tmp_dir = unique_temp_dir("pr-cli-force-remove-symlink");
        let target = tmp_dir.join("target");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("precious"), b"data").expect("seed");
        std::os::unix::fs::symlink(&target, tmp_dir.join("link")).expect("symlink");

        force_remove_dir_all(&tmp_dir.join("link"));

        assert!(fs::symlink_metadata(tmp_dir.join("link")).is_err());
        assert_eq!(fs::read(target.join("precious")).unwrap(), b"data");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn restore_refuses_a_plugin_without_its_container() {
        let env = TestPrefix::new("pr-cli-restore-stray-plugin");
        let output = env.base.join("box.tar.gz");
        write_gz_backup(
            &output,
            &[
                ("containers/box/rootfs/f", Some(b"x")),
                ("pr/ubuntu.sh", Some(b"DISTRO_NAME=\"Evil\"\n")),
            ],
        );

        assert!(command_restore(output.to_str().unwrap()).is_err());
        assert!(!env.prefix.join("etc/pr/ubuntu.sh").exists());
        assert!(!env.containers().join("box").exists());
    }

    #[test]
    fn backup_refuses_a_compression_it_cannot_write() {
        let env = TestPrefix::new("pr-cli-backup-bz2");
        seed_rootfs(&env.containers().join("box/rootfs"));
        for name in ["box.tar.bz2", "box.tar.zst", "box.TAR.BZ2"] {
            let output = env.base.join(name);
            assert!(command_backup("box", Some(output.to_str().unwrap())).is_err());
            assert!(!output.exists(), "{}", name);
        }
        let upper = env.base.join("box.TAR.GZ");
        command_backup("box", Some(upper.to_str().unwrap())).expect("backup .TAR.GZ");
        assert_eq!(&fs::read(&upper).unwrap()[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn restore_names_a_compression_it_cannot_read() {
        let env = TestPrefix::new("pr-cli-restore-bz2");
        let output = env.base.join("box.tar.bz2");
        fs::write(&output, b"BZh91AY&SYxxxxxxxxxxxxxxxxxxxx").expect("write bz2");

        let err = command_restore(output.to_str().unwrap()).expect_err("must fail");
        assert!(err.contains("bzip2"), "{}", err);
    }

    #[test]
    fn restore_refuses_entries_outside_the_backup_layout() {
        for bad in [
            "etc/passwd",
            "installed-rootfs/../escape",
            "containers/a b/rootfs/x",
            "pr/sub/x.sh",
        ] {
            let env = TestPrefix::new("pr-cli-restore-bad-entry");
            let output = env.base.join("bad.tar.gz");
            write_gz_backup(
                &output,
                &[("containers/ok/rootfs/f", Some(b"x")), (bad, Some(b"x"))],
            );

            assert!(
                command_restore(output.to_str().unwrap()).is_err(),
                "{} was accepted",
                bad
            );
            assert!(!env.base.join("escape").exists());
            assert!(!env.prefix.join("etc/passwd").exists());
            assert!(!env.containers().join("ok").exists());
        }
    }

    #[test]
    fn restore_keeps_the_existing_container_when_the_backup_is_bad() {
        let env = TestPrefix::new("pr-cli-restore-keeps-existing");
        let existing = env.containers().join("box/rootfs");
        fs::create_dir_all(&existing).expect("create existing");
        fs::write(existing.join("keep"), b"data").expect("seed existing");
        let output = env.base.join("bad.tar.gz");
        write_gz_backup(
            &output,
            &[
                ("containers/box/rootfs/new", Some(b"x")),
                ("etc/passwd", Some(b"x")),
            ],
        );

        assert!(command_restore(output.to_str().unwrap()).is_err());

        assert_eq!(fs::read(existing.join("keep")).unwrap(), b"data");
        assert!(!existing.join("new").exists());
        let leftovers: Vec<_> = fs::read_dir(env.containers())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("box")]);
    }

    #[test]
    fn resolve_path_with_installed_distro_prefixes_rootfs_path() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-resolve-installed");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(rootfs.join("etc")).expect("create rootfs etc");
        fs::write(rootfs.join("etc/os-release"), "NAME=Debian\n").expect("write os-release");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let resolved =
            resolve_path(Some("debian"), "etc/os-release").expect("resolve installed path");
        assert!(resolved.ends_with("/etc/os-release"));
        assert!(resolved.contains("/installed-rootfs/debian/"));

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }
}
