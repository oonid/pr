use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use libc;

use crate::install_model::load_oci_install_metadata;
use crate::plugin::{load_plugins, DistroPlugin};
use crate::shared::{
    get_download_cache_dir, get_installed_rootfs_dir, get_native_busybox, get_oci_container_dir,
    get_oci_container_manifest_path, get_oci_containers_dir, get_plugins_dir, msg_error,
    msg_status, resolve_installed_rootfs, InstalledSourceType,
};

/// Remove a directory tree, handling 0000-permission proot bind-mount stubs.
///
/// proot creates empty placeholder dirs (apex, odm, product, sdcard, system,
/// system_ext, vendor) with mode 0000.  `busybox rm -rf` bails on them because
/// it tries `opendir()` which requires execute permission.  `libc::rmdir()` only
/// needs write permission on the *parent*, so it succeeds even on 0000-perms
/// empty dirs.  For non-empty dirs we chmod 0700 first, then recurse.
pub(crate) fn force_remove_dir_all(path: &Path) {
    // Fast path: try rmdir first.  For empty dirs (even 0000-perms) this works
    // because the kernel only checks write permission on the parent directory.
    if let Ok(cstr) = CString::new(path.to_string_lossy().as_bytes()) {
        let ret = unsafe { libc::rmdir(cstr.as_ptr()) };
        if ret == 0 {
            return;
        }
    }

    // rmdir failed — directory is non-empty or we can't stat it.
    // chmod 0700 so we can list and enter it.
    if let Ok(cstr) = CString::new(path.to_string_lossy().as_bytes()) {
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
                    if let Ok(cstr) = CString::new(p.to_string_lossy().as_bytes()) {
                        unsafe {
                            libc::unlink(cstr.as_ptr());
                        }
                    }
                }
            }
        }
    }

    // rmdir after children are cleared
    if let Ok(cstr) = CString::new(path.to_string_lossy().as_bytes()) {
        unsafe {
            libc::rmdir(cstr.as_ptr());
        }
    }
}

pub fn command_remove(distro_name: &str, is_reset: bool) -> Result<(), String> {
    let plugins_dir = get_plugins_dir();
    let Some((rootfs, source_type)) = resolve_installed_rootfs(distro_name) else {
        println!();
        msg_error(&format!("distribution '{}' is not installed.", distro_name));
        println!();
        return Err("not installed".to_string());
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

    // Use busybox rm -rf for the bulk of the tree (fast, handles symlinks, etc.).
    // Ignore its exit code: it exits 1 on proot's 0000-permission bind-mount
    // stub dirs (apex, odm, product, sdcard, system, system_ext, vendor) because
    // it can't opendir() them.  We clean those up with force_remove_dir_all().
    let busybox = get_native_busybox();
    let _ = Command::new(&busybox)
        .arg0("busybox")
        .args(["rm", "-rf", &install_path])
        .status();

    // Sweep anything busybox left behind (empty 0000-perms dirs, etc.).
    let rootfs_path = Path::new(&install_path);
    if rootfs_path.exists() {
        force_remove_dir_all(rootfs_path);
    }
    if rootfs_path.exists() {
        return Err(format!("failed to fully remove path '{}'", install_path));
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
    if !plugins.iter().any(|p| p.alias == distro_name) {
        println!();
        msg_error(&format!(
            "unknown distribution '{}' was requested to be reset.",
            distro_name
        ));
        println!();
        return Err("unknown distribution".to_string());
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
    fn inner(p: &Path) {
        if let Ok(entries) = fs::read_dir(p) {
            for entry in entries.flatten() {
                let ep = entry.path();
                if ep.is_dir() {
                    let _ = fs::set_permissions(&ep, fs::Permissions::from_mode(0o755));
                    inner(&ep);
                } else {
                    let _ = fs::set_permissions(&ep, fs::Permissions::from_mode(0o644));
                }
            }
        }
    }
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
    inner(path);
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
    let installed_rootfs_dir = get_installed_rootfs_dir();
    let oci_containers_dir = get_oci_containers_dir();
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

    let busybox = get_native_busybox();
    let status = if source_type == InstalledSourceType::Legacy {
        let plugin_file =
            if Path::new(&format!("{}/{}.override.sh", plugins_dir, distro_name)).exists() {
                Some(format!("{}.override.sh", distro_name))
            } else if Path::new(&format!("{}/{}.sh", plugins_dir, distro_name)).exists() {
                Some(format!("{}.sh", distro_name))
            } else {
                None
            };
        msg_status("Archiving the rootfs and plug-in...");
        let parent_rootfs = Path::new(&installed_rootfs_dir)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .to_string();
        let rootfs_basename = Path::new(&installed_rootfs_dir)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let parent_plugins = Path::new(&plugins_dir)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .to_string();
        let plugins_basename = Path::new(&plugins_dir)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mut args = vec![
            "tar".to_string(),
            "-c".to_string(),
            "--auto-compress".to_string(),
            "--warning=no-file-ignored".to_string(),
            "-f".to_string(),
            output.clone(),
        ];
        if let Some(plugin_file) = plugin_file {
            args.push("-C".to_string());
            args.push(parent_plugins.clone());
            args.push(format!("{}/{}", plugins_basename, plugin_file));
        }
        args.push("-C".to_string());
        args.push(parent_rootfs.clone());
        args.push(format!("{}/{}", rootfs_basename, distro_name));
        Command::new(&busybox)
            .arg0("busybox")
            .args(&args)
            .status()
            .map_err(|e| format!("exec tar: {}", e))?
    } else {
        msg_status("Archiving the OCI container directory...");
        let parent_containers = Path::new(&oci_containers_dir)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .to_string();
        let containers_basename = Path::new(&oci_containers_dir)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Command::new(&busybox)
            .arg0("busybox")
            .arg("tar")
            .args([
                "-c",
                "--auto-compress",
                "--warning=no-file-ignored",
                "-f",
                &output,
                "-C",
                &parent_containers,
                &format!("{}/{}", containers_basename, distro_name),
            ])
            .status()
            .map_err(|e| format!("exec tar: {}", e))?
    };

    if !status.success() {
        let _ = fs::remove_file(&output);
        msg_error("Backup failed.");
        return Err("tar failed".to_string());
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

    let busybox = get_native_busybox();
    let list_output = Command::new(&busybox)
        .arg0("busybox")
        .arg("tar")
        .args(["-tf", tarball_path])
        .output()
        .map_err(|e| format!("list tarball: {}", e))?;
    if !list_output.status.success() {
        return Err("unable to inspect tarball".to_string());
    }
    let listing = String::from_utf8_lossy(&list_output.stdout);
    let contains_oci = listing.lines().any(|line| line.starts_with("containers/"));

    if contains_oci {
        fs::create_dir_all(&oci_containers_dir)
            .map_err(|e| format!("create containers dir: {}", e))?;
        msg_status("Extracting OCI container data from the tarball...");
    } else {
        fs::create_dir_all(&installed_rootfs_dir)
            .map_err(|e| format!("create installed-rootfs dir: {}", e))?;
        fs::create_dir_all(&plugins_dir).map_err(|e| format!("create plugins dir: {}", e))?;
        msg_status("Extracting distribution plug-in and rootfs from the tarball...");
    }
    let parent_rootfs = Path::new(&installed_rootfs_dir)
        .parent()
        .unwrap_or(Path::new("/"))
        .to_string_lossy()
        .to_string();
    let rootfs_basename = Path::new(&installed_rootfs_dir)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent_plugins = Path::new(&plugins_dir)
        .parent()
        .unwrap_or(Path::new("/"))
        .to_string_lossy()
        .to_string();
    let plugins_basename = Path::new(&plugins_dir)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let contains_plugin_entries = listing
        .lines()
        .any(|line| line.starts_with("pr/") || line.starts_with("proot-distro/"));

    let status = if contains_oci {
        let parent_containers = Path::new(&oci_containers_dir)
            .parent()
            .unwrap_or(Path::new("/"))
            .to_string_lossy()
            .to_string();
        let containers_basename = Path::new(&oci_containers_dir)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Command::new(&busybox)
            .arg0("busybox")
            .arg("tar")
            .args([
                "-x",
                "--auto-compress",
                "--recursive-unlink",
                "--preserve-permissions",
                "-f",
                tarball_path,
                "-C",
                &parent_containers,
                &format!("{}/", containers_basename),
            ])
            .status()
            .map_err(|e| format!("exec tar: {}", e))?
    } else {
        let mut args = vec![
            "tar".to_string(),
            "-x".to_string(),
            "--auto-compress".to_string(),
            "--recursive-unlink".to_string(),
            "--preserve-permissions".to_string(),
            "-f".to_string(),
            tarball_path.to_string(),
        ];
        if contains_plugin_entries {
            args.push("-C".to_string());
            args.push(parent_plugins.clone());
            args.push(format!("{}/", plugins_basename));
        }
        args.push("-C".to_string());
        args.push(parent_rootfs.clone());
        args.push(format!("{}/", rootfs_basename));
        Command::new(&busybox)
            .arg0("busybox")
            .args(&args)
            .status()
            .map_err(|e| format!("exec tar: {}", e))?
    };

    if !status.success() {
        msg_error("Restore failed.");
        return Err("tar extract failed".to_string());
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
    if !new_alias
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
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
    if !plugins.iter().any(|p| p.alias == old_alias) {
        println!();
        msg_error(&format!(
            "unknown distribution '{}' was requested to be renamed.",
            old_alias
        ));
        println!();
        return Err("unknown distribution".to_string());
    }

    let old_rootfs = format!("{}/{}", installed_rootfs_dir, old_alias);
    if !Path::new(&old_rootfs).is_dir() {
        println!();
        msg_error(&format!(
            "cannot rename because the distribution '{}' is not installed.",
            old_alias
        ));
        return Err("not installed".to_string());
    }

    let new_rootfs = format!("{}/{}", installed_rootfs_dir, new_alias);
    if Path::new(&new_rootfs).is_dir() {
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

    fs::rename(&old_rootfs, &new_rootfs).map_err(|e| format!("rename rootfs: {}", e))?;

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

    if let Some(parent) = Path::new(&dst_full).parent() {
        if !parent.exists() {
            msg_status(&format!("Creating directory '{}'...", parent.display()));
            fs::create_dir_all(parent).map_err(|e| format!("create dir: {}", e))?;
        }
    }

    msg_status("Copying files, this may take a while...");

    let busybox = get_native_busybox();
    let status = Command::new(&busybox)
        .arg0("busybox")
        .args(["cp", "-a", &src_full, &dst_full])
        .status()
        .map_err(|e| format!("exec cp: {}", e))?;

    if !status.success() {
        msg_error("Copy failed.");
        return Err("cp failed".to_string());
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
    fn command_copy_rejects_missing_source() {
        let err = command_copy("/tmp/does-not-exist", "/tmp/destination-file")
            .expect_err("must reject missing source path");
        assert_eq!(err, "source not found");
    }

    #[test]
    fn chmod_readable_recursive_sets_readable_permissions() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-chmod-readable");
        let nested = tmp_dir.join("nested");
        let file = nested.join("note.txt");
        fs::create_dir_all(&nested).expect("create nested dir");
        fs::write(&file, b"hello").expect("write file");
        fs::set_permissions(&tmp_dir, fs::Permissions::from_mode(0o700)).expect("chmod root");
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).expect("chmod nested");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("chmod file");

        chmod_readable_recursive(&tmp_dir);

        let root_mode = fs::metadata(&tmp_dir)
            .expect("root meta")
            .permissions()
            .mode()
            & 0o777;
        let nested_mode = fs::metadata(&nested)
            .expect("nested meta")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&file).expect("file meta").permissions().mode() & 0o777;
        assert_eq!(root_mode, 0o755);
        assert_eq!(nested_mode, 0o755);
        assert_eq!(file_mode, 0o644);

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
    fn command_restore_reports_uninspectable_tarball_without_busybox() {
        let tmp_dir = unique_temp_dir("pr-cli-commands-extra-restore-uninspectable");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let tarball = tmp_dir.join("backup.tar.xz");
        fs::write(&tarball, b"not-a-real-tarball").expect("write file");

        let err = command_restore(tarball.to_str().expect("tarball path"))
            .expect_err("restore should fail to inspect tarball");
        assert!(err.contains("list tarball"));

        let _ = fs::remove_dir_all(tmp_dir);
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
