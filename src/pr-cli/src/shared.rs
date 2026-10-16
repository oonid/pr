pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const DEFAULT_FAKE_KERNEL_RELEASE: &str = "6.17.0-pr";
pub const DEFAULT_FAKE_KERNEL_VERSION: &str =
    "#1 SMP PREEMPT_DYNAMIC Fri, 10 Oct 2025 00:00:00 +0000";
pub const DEFAULT_PRIMARY_NAMESERVER: &str = "8.8.8.8";
pub const DEFAULT_SECONDARY_NAMESERVER: &str = "8.8.4.4";

/// First line of a resolv.conf that pr-cli owns and rewrites at each login.
pub const RESOLV_CONF_MARKER: &str =
    "# Managed by pr-cli: refreshed at login from the active network's DNS. Delete this line to keep your own servers.\n";

/// What pr-cli wrote before network DNS, so existing installs are refreshed too.
const LEGACY_RESOLV_CONFS: [&str; 2] = [
    "nameserver 8.8.8.8\nnameserver 8.8.4.4\n",
    "nameserver 8.8.8.8\nnameserver 1.1.1.1\n",
];

/// DNS servers of the active Android network, passed by the app as `APP_DNS_SERVERS` (comma-separated).
pub fn network_dns_servers() -> Vec<String> {
    std::env::var("APP_DNS_SERVERS")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// At most three nameservers (the musl and glibc limit): up to two from the network, so some
/// networks that block public DNS still resolve, then the public servers as fallback.
pub fn resolv_conf_contents(network: &[String]) -> String {
    let mut servers: Vec<String> = Vec::new();
    for raw in network {
        // A scoped or link-local address (fe80::1%wlan0) cannot be used from resolv.conf.
        let ip = match raw.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V6(v6)) if (v6.segments()[0] & 0xffc0) == 0xfe80 => continue,
            Ok(ip) => ip.to_string(),
            Err(_) => continue,
        };
        if !servers.contains(&ip) && servers.len() < 2 {
            servers.push(ip);
        }
    }
    for fallback in [DEFAULT_PRIMARY_NAMESERVER, DEFAULT_SECONDARY_NAMESERVER] {
        if servers.len() < 3 && !servers.iter().any(|s| s == fallback) {
            servers.push(fallback.to_string());
        }
    }
    let mut contents = String::from(RESOLV_CONF_MARKER);
    for server in servers {
        contents.push_str(&format!("nameserver {}\n", server));
    }
    contents
}

/// True when login may rewrite resolv.conf: missing, written by pr-cli, or an old pr-cli default.
pub fn resolv_conf_is_managed(existing: Option<&str>) -> bool {
    match existing {
        None => true,
        Some(contents) => {
            contents.starts_with(RESOLV_CONF_MARKER) || LEGACY_RESOLV_CONFS.contains(&contents)
        }
    }
}
pub const DEFAULT_PATH_ENV_SUFFIX: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games";

pub fn global_test_env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstalledSourceType {
    Legacy,
    Oci,
}

pub fn get_prefix() -> String {
    std::env::var("APP_PREFIX").unwrap_or_else(|_| "/data/data/id.or.oo.pr/files/usr".to_string())
}

/// The other path of the same primary-user app data directory. L2S links store absolute host
/// paths, so links made from the app (`/data/user/0/<pkg>`) and from `adb run-as`
/// (`/data/data/<pkg>`) only resolve when both paths are bound.
pub fn app_dir_alias(app_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let pkg = app_dir.file_name()?;
    let parent = app_dir.parent()?;
    if parent == std::path::Path::new("/data/data") {
        Some(std::path::Path::new("/data/user/0").join(pkg))
    } else if parent == std::path::Path::new("/data/user/0") {
        Some(std::path::Path::new("/data/data").join(pkg))
    } else {
        None
    }
}

pub fn get_bin_dir() -> String {
    format!("{}/bin", get_prefix())
}

pub fn get_plugins_dir() -> String {
    format!("{}/etc/pr", get_prefix())
}

pub fn get_installed_rootfs_dir() -> String {
    format!("{}/var/lib/pr/installed-rootfs", get_prefix())
}

fn get_oci_containers_dir_for_prefix(prefix: &str) -> String {
    format!("{}/var/lib/pr/containers", prefix)
}

fn get_oci_container_dir_for_prefix(prefix: &str, name: &str) -> String {
    format!("{}/{}", get_oci_containers_dir_for_prefix(prefix), name)
}

fn get_oci_container_rootfs_dir_for_prefix(prefix: &str, name: &str) -> String {
    format!("{}/rootfs", get_oci_container_dir_for_prefix(prefix, name))
}

fn get_oci_container_manifest_path_for_prefix(prefix: &str, name: &str) -> String {
    format!(
        "{}/manifest.json",
        get_oci_container_dir_for_prefix(prefix, name)
    )
}

pub fn get_oci_containers_dir() -> String {
    get_oci_containers_dir_for_prefix(&get_prefix())
}

pub fn get_oci_container_dir(name: &str) -> String {
    get_oci_container_dir_for_prefix(&get_prefix(), name)
}

pub fn get_oci_container_rootfs_dir(name: &str) -> String {
    get_oci_container_rootfs_dir_for_prefix(&get_prefix(), name)
}

pub fn get_oci_container_manifest_path(name: &str) -> String {
    get_oci_container_manifest_path_for_prefix(&get_prefix(), name)
}

pub fn resolve_installed_rootfs(name: &str) -> Option<(String, InstalledSourceType)> {
    let legacy_rootfs = format!("{}/{}", get_installed_rootfs_dir(), name);
    if std::path::Path::new(&legacy_rootfs).is_dir() {
        return Some((legacy_rootfs, InstalledSourceType::Legacy));
    }

    let oci_rootfs = get_oci_container_rootfs_dir(name);
    if std::path::Path::new(&oci_rootfs).is_dir() {
        return Some((oci_rootfs, InstalledSourceType::Oci));
    }

    None
}

pub fn get_download_cache_dir() -> String {
    format!("{}/var/lib/pr/dlcache", get_prefix())
}

pub fn get_default_path_env() -> String {
    format!("{}:{}", DEFAULT_PATH_ENV_SUFFIX, get_prefix())
}

pub fn get_native_lib_dir() -> String {
    std::fs::read_link("/proc/self/exe")
        .ok()
        .and_then(|p| p.parent().map(|p| p.display().to_string()))
        .unwrap_or_else(|| get_bin_dir())
}

pub fn get_native_busybox() -> String {
    format!("{}/libbusybox.so", get_native_lib_dir())
}

pub fn get_native_proot() -> String {
    format!("{}/libproot.so", get_native_lib_dir())
}

pub fn get_native_loader() -> String {
    format!("{}/libproot-loader.so", get_native_lib_dir())
}

pub fn msg_status(text: &str) {
    println!(
        "{}\x1b[1;34m[\x1b[32m*\x1b[1;34m\x1b[36m {}\x1b[0m",
        "", text
    );
}

pub fn msg_error(text: &str) {
    println!(
        "{}\x1b[1;34m[\x1b[31m!\x1b[1;34m\x1b[36m {}\x1b[0m",
        "", text
    );
}

pub fn can_read_dir(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let mode = meta.permissions().mode();
    (mode & 0o5) != 0
}

pub fn can_read_file(path: &str) -> bool {
    std::fs::read(path).is_ok()
}

pub fn can_list_dir(path: &str) -> bool {
    std::fs::read_dir(path).is_ok()
}

pub fn build_proot_args(
    rootfs: &str,
    isolated: bool,
    no_link2symlink: bool,
    custom_bind: &[String],
) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let prefix = get_prefix();
    let cache_dir = std::env::var("PROOT_TMP_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| format!("{}/tmp", prefix));

    let l2s_dir = format!("{}/.l2s", rootfs);
    if Path::new(&l2s_dir).is_dir() {
        std::env::set_var("PROOT_L2S_DIR", &l2s_dir);
    }

    let mut args: Vec<String> = Vec::new();

    for bnd in custom_bind {
        args.push(format!("--bind={}", bnd));
    }

    if !isolated {
        for data_dir in &[
            "/data/app",
            "/data/dalvik-cache",
            "/data/misc/apexdata/com.android.art/dalvik-cache",
        ] {
            if Path::new(data_dir).is_dir() && can_read_dir(data_dir) {
                args.push(format!("--bind={}", data_dir));
            }
        }

        let app_dir = Path::new(&prefix).parent().unwrap().parent().unwrap();

        let apps_dir = app_dir.join("files/apps");
        if apps_dir.is_dir() {
            args.push(format!("--bind={}", apps_dir.display()));
        }

        args.push(format!("--bind={}/cache", app_dir.display()));
        args.push(format!("--bind={}", app_dir.display()));
        if let Some(alias) = app_dir_alias(app_dir).filter(|a| a.is_dir()) {
            args.push(format!("--bind={}", alias.display()));
        }

        if can_list_dir("/storage") {
            args.push("--bind=/storage".to_string());
            args.push("--bind=/storage/emulated/0:/sdcard".to_string());
        } else {
            let storage_path = if can_list_dir("/storage/self/primary/") {
                Some("/storage/self/primary")
            } else if can_list_dir("/storage/emulated/0/") {
                Some("/storage/emulated/0")
            } else if can_list_dir("/sdcard/") {
                Some("/sdcard")
            } else {
                None
            };

            if let Some(sp) = storage_path {
                args.push(format!("--bind={}:/sdcard", sp));
                args.push(format!("--bind={}:/storage/emulated/0", sp));
                args.push(format!("--bind={}:/storage/self/primary", sp));
            }
        }

        for system_mnt in &[
            "/apex",
            "/odm",
            "/product",
            "/system",
            "/system_ext",
            "/vendor",
            "/linkerconfig/ld.config.txt",
            "/linkerconfig/com.android.art/ld.config.txt",
            "/plat_property_contexts",
            "/property_contexts",
        ] {
            let p = Path::new(system_mnt);
            if !p.exists() {
                continue;
            }
            let real = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            let real_str = real.to_string_lossy();
            if real.is_dir() {
                if can_read_dir(&real_str) {
                    args.push(format!("--bind={}", real_str));
                }
            } else if real.is_file() {
                if can_read_file(&real_str) {
                    args.push(format!("--bind={}", real_str));
                }
            }
        }

        args.push(format!("--bind={}", prefix));
    }

    let tmp_dir = format!("{}/tmp", rootfs);
    if !Path::new(&tmp_dir).is_dir() {
        let _ = std::fs::create_dir_all(&tmp_dir);
        let _ = std::fs::set_permissions(&tmp_dir, std::fs::Permissions::from_mode(0o1777));
    }
    args.push(format!("--bind={}/tmp:/dev/shm", rootfs));

    args.push("--bind=/dev".to_string());
    args.push("--bind=/proc".to_string());
    args.push("--bind=/sys".to_string());

    args.push("--bind=/proc/self/fd:/dev/fd".to_string());
    args.push("--bind=/dev/urandom:/dev/random".to_string());

    for (fake, real) in &[
        ("proc/.loadavg", "/proc/loadavg"),
        ("proc/.stat", "/proc/stat"),
        ("proc/.uptime", "/proc/uptime"),
        ("proc/.version", "/proc/version"),
        ("proc/.vmstat", "/proc/vmstat"),
    ] {
        if !can_read_file(real) {
            args.push(format!("--bind={}/{}:{}", rootfs, fake, real));
        }
    }

    if Path::new("/sys/fs/selinux").exists() {
        args.push(format!("--bind={}/sys/.empty:/sys/fs/selinux", rootfs));
    }

    args.push(format!("--bind={}:{}", cache_dir, "/tmp"));

    args.push("-L".to_string());

    let kernel_release = std::env::var("PROOT_DISTRO_KERNEL_RELEASE")
        .unwrap_or_else(|_| DEFAULT_FAKE_KERNEL_RELEASE.to_string());
    args.push(format!("--kernel-release={}", kernel_release,));

    if !no_link2symlink {
        args.push("--link2symlink".to_string());
    }

    args.push("--kill-on-exit".to_string());

    args.push("--change-id=0:0".to_string());

    args.push(format!("--rootfs={}", rootfs));
    args.push("--cwd=/".to_string());

    args
}

pub fn build_proot_runtime_env() -> Vec<(&'static str, String)> {
    let cache_dir = std::env::var("PROOT_TMP_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| format!("{}/tmp", get_prefix()));

    vec![
        ("PROOT_NO_SECCOMP", "1".to_string()),
        (
            "PROOT_L2S_DIR",
            std::env::var("PROOT_L2S_DIR").unwrap_or_default(),
        ),
        ("PROOT_TMP_DIR", cache_dir.clone()),
        ("TMPDIR", cache_dir),
        ("PROOT_LOADER", get_native_loader()),
    ]
}

pub fn build_proot_child_env() -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("PATH".to_string(), get_default_path_env()),
        ("HOME".to_string(), "/root".to_string()),
        ("USER".to_string(), "root".to_string()),
        (
            "TERM".to_string(),
            std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()),
        ),
        ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ("TMPDIR".to_string(), "/tmp".to_string()),
    ];

    for var in &[
        "ANDROID_ART_ROOT",
        "ANDROID_DATA",
        "ANDROID_I18N_ROOT",
        "ANDROID_ROOT",
        "ANDROID_RUNTIME_ROOT",
        "ANDROID_TZDATA_ROOT",
        "BOOTCLASSPATH",
        "DEX2OATBOOTCLASSPATH",
        "EXTERNAL_STORAGE",
    ] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                env.push((var.to_string(), val));
            }
        }
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn nameservers(contents: &str) -> Vec<&str> {
        contents
            .lines()
            .filter_map(|l| l.strip_prefix("nameserver "))
            .collect()
    }

    #[test]
    fn resolv_conf_puts_network_dns_first_then_public_fallback() {
        let contents = resolv_conf_contents(&["192.168.1.1".to_string()]);
        assert!(contents.starts_with(RESOLV_CONF_MARKER));
        assert_eq!(
            nameservers(&contents),
            ["192.168.1.1", "8.8.8.8", "8.8.4.4"]
        );
    }

    #[test]
    fn resolv_conf_skips_invalid_duplicate_and_scoped_servers_and_caps_at_three() {
        let network = [
            "2400:9800:2:2::245",
            "not-an-ip",
            "fe80::1%wlan0",
            "2400:9800:2:2::245",
            "2400:9800:2:2::246",
            "10.0.0.1",
        ]
        .map(String::from);
        let contents = resolv_conf_contents(&network);
        assert_eq!(
            nameservers(&contents),
            ["2400:9800:2:2::245", "2400:9800:2:2::246", "8.8.8.8"]
        );
    }

    #[test]
    fn resolv_conf_without_network_dns_uses_public_servers() {
        assert_eq!(
            nameservers(&resolv_conf_contents(&[])),
            ["8.8.8.8", "8.8.4.4"]
        );
    }

    #[test]
    fn login_refreshes_only_pr_managed_resolv_conf() {
        assert!(resolv_conf_is_managed(None));
        assert!(resolv_conf_is_managed(Some(&resolv_conf_contents(&[]))));
        assert!(resolv_conf_is_managed(Some(
            "nameserver 8.8.8.8\nnameserver 8.8.4.4\n"
        )));
        assert!(resolv_conf_is_managed(Some(
            "nameserver 8.8.8.8\nnameserver 1.1.1.1\n"
        )));
        assert!(!resolv_conf_is_managed(Some("nameserver 9.9.9.9\n")));
    }

    #[test]
    fn app_dir_alias_maps_data_data_to_user_0() {
        assert_eq!(
            app_dir_alias(Path::new("/data/data/id.or.oo.pr")),
            Some(PathBuf::from("/data/user/0/id.or.oo.pr"))
        );
    }

    #[test]
    fn app_dir_alias_maps_user_0_to_data_data() {
        assert_eq!(
            app_dir_alias(Path::new("/data/user/0/id.or.oo.pr")),
            Some(PathBuf::from("/data/data/id.or.oo.pr"))
        );
    }

    #[test]
    fn app_dir_alias_is_none_for_secondary_users_and_other_paths() {
        assert_eq!(app_dir_alias(Path::new("/data/user/10/id.or.oo.pr")), None);
        assert_eq!(app_dir_alias(Path::new("/tmp/pr-test/app")), None);
    }

    fn env_lock() -> &'static std::sync::Mutex<()> {
        super::global_test_env_lock()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time ok")
            .as_nanos();
        std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos))
    }

    #[test]
    fn oci_container_paths_follow_expected_layout() {
        let prefix = "/data/data/id.or.oo.pr/files/usr";
        let name = "debian";

        assert_eq!(
            get_oci_containers_dir_for_prefix(prefix),
            "/data/data/id.or.oo.pr/files/usr/var/lib/pr/containers"
        );
        assert_eq!(
            get_oci_container_dir_for_prefix(prefix, name),
            "/data/data/id.or.oo.pr/files/usr/var/lib/pr/containers/debian"
        );
        assert_eq!(
            get_oci_container_rootfs_dir_for_prefix(prefix, name),
            "/data/data/id.or.oo.pr/files/usr/var/lib/pr/containers/debian/rootfs"
        );
        assert_eq!(
            get_oci_container_manifest_path_for_prefix(prefix, name),
            "/data/data/id.or.oo.pr/files/usr/var/lib/pr/containers/debian/manifest.json"
        );
    }

    #[test]
    fn resolve_installed_rootfs_prefers_legacy_when_both_exist() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-rootfs");
        let prefix = base.join("usr");
        let legacy = prefix.join("var/lib/pr/installed-rootfs/debian");
        let oci_rootfs = prefix.join("var/lib/pr/containers/debian/rootfs");
        fs::create_dir_all(&legacy).expect("create legacy rootfs");
        fs::create_dir_all(&oci_rootfs).expect("create oci rootfs");

        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
        let resolved = resolve_installed_rootfs("debian").expect("resolve installed rootfs");
        assert_eq!(resolved.1, InstalledSourceType::Legacy);
        assert_eq!(resolved.0, legacy.to_string_lossy());

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn build_proot_runtime_env_prefers_tmpdir_when_set() {
        let _guard = env_lock().lock().expect("lock env");
        std::env::set_var("TMPDIR", "/tmp/custom-cache");
        std::env::set_var("PROOT_L2S_DIR", "/tmp/custom-l2s");

        let env = build_proot_runtime_env();
        let map: std::collections::BTreeMap<&str, String> = env.into_iter().collect();
        assert_eq!(
            map.get("PROOT_TMP_DIR").map(String::as_str),
            Some("/tmp/custom-cache")
        );
        assert_eq!(
            map.get("TMPDIR").map(String::as_str),
            Some("/tmp/custom-cache")
        );
        assert_eq!(
            map.get("PROOT_L2S_DIR").map(String::as_str),
            Some("/tmp/custom-l2s")
        );

        std::env::remove_var("TMPDIR");
        std::env::remove_var("PROOT_L2S_DIR");
    }

    #[test]
    fn build_proot_child_env_includes_non_empty_android_vars() {
        let _guard = env_lock().lock().expect("lock env");
        std::env::set_var("ANDROID_ROOT", "/system");
        std::env::set_var("EXTERNAL_STORAGE", "/sdcard");

        let env = build_proot_child_env();
        let map: std::collections::BTreeMap<String, String> = env.into_iter().collect();
        assert_eq!(map.get("ANDROID_ROOT").map(String::as_str), Some("/system"));
        assert_eq!(
            map.get("EXTERNAL_STORAGE").map(String::as_str),
            Some("/sdcard")
        );
        assert_eq!(map.get("LANG").map(String::as_str), Some("en_US.UTF-8"));

        std::env::remove_var("ANDROID_ROOT");
        std::env::remove_var("EXTERNAL_STORAGE");
    }

    #[test]
    fn build_proot_args_includes_custom_bind_and_rootfs_flags() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-proot-args");
        let rootfs = base.join("rootfs");
        fs::create_dir_all(rootfs.join(".l2s")).expect("create .l2s");

        let args = build_proot_args(
            rootfs.to_str().expect("rootfs path"),
            true,
            false,
            &[String::from("/host:/guest")],
        );

        assert!(args.contains(&String::from("--bind=/host:/guest")));
        assert!(args.contains(&format!("--rootfs={}", rootfs.to_string_lossy())));
        assert!(args.contains(&String::from("--change-id=0:0")));
        assert!(args.contains(&String::from("--link2symlink")));
        assert!(args.iter().any(|a| a.starts_with("--kernel-release=")));

        let _ = fs::remove_dir_all(base);
        std::env::remove_var("PROOT_L2S_DIR");
    }

    #[test]
    fn prefix_helpers_follow_app_prefix() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-prefix");
        let prefix = base.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let prefix_str = prefix.to_string_lossy().to_string();
        assert_eq!(get_prefix(), prefix_str);
        assert_eq!(get_bin_dir(), format!("{}/bin", prefix_str));
        assert_eq!(get_plugins_dir(), format!("{}/etc/pr", prefix_str));
        assert_eq!(
            get_installed_rootfs_dir(),
            format!("{}/var/lib/pr/installed-rootfs", prefix_str)
        );
        assert_eq!(
            get_download_cache_dir(),
            format!("{}/var/lib/pr/dlcache", prefix_str)
        );
        assert_eq!(
            get_default_path_env(),
            format!("{}:{}", DEFAULT_PATH_ENV_SUFFIX, prefix_str)
        );
        assert_eq!(
            get_oci_containers_dir(),
            format!("{}/var/lib/pr/containers", prefix_str)
        );
        assert_eq!(
            get_oci_container_dir("debian"),
            format!("{}/var/lib/pr/containers/debian", prefix_str)
        );
        assert_eq!(
            get_oci_container_rootfs_dir("debian"),
            format!("{}/var/lib/pr/containers/debian/rootfs", prefix_str)
        );
        assert_eq!(
            get_oci_container_manifest_path("debian"),
            format!("{}/var/lib/pr/containers/debian/manifest.json", prefix_str)
        );

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn native_binary_paths_are_based_on_native_lib_dir() {
        let native_lib_dir = get_native_lib_dir();
        assert_eq!(
            get_native_busybox(),
            format!("{}/libbusybox.so", native_lib_dir)
        );
        assert_eq!(
            get_native_proot(),
            format!("{}/libproot.so", native_lib_dir)
        );
        assert_eq!(
            get_native_loader(),
            format!("{}/libproot-loader.so", native_lib_dir)
        );
    }

    #[test]
    fn resolve_installed_rootfs_prefers_oci_when_legacy_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-rootfs-oci");
        let prefix = base.join("usr");
        let oci_rootfs = prefix.join("var/lib/pr/containers/debian/rootfs");
        fs::create_dir_all(&oci_rootfs).expect("create oci rootfs");

        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
        let resolved = resolve_installed_rootfs("debian").expect("resolve installed rootfs");
        assert_eq!(resolved.1, InstalledSourceType::Oci);
        assert_eq!(resolved.0, oci_rootfs.to_string_lossy());

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn resolve_installed_rootfs_returns_none_when_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-rootfs-none");
        let prefix = base.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");

        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
        assert!(resolve_installed_rootfs("debian").is_none());

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn read_helpers_report_expected_access() {
        use std::os::unix::fs::PermissionsExt;

        let base = unique_temp_dir("pr-cli-shared-read-access");
        let dir = base.join("readable-dir");
        let restricted_dir = base.join("restricted-dir");
        let file = base.join("readable.txt");
        fs::create_dir_all(&dir).expect("create dir");
        fs::create_dir_all(&restricted_dir).expect("create restricted dir");
        fs::write(&file, "hello").expect("write file");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod dir");
        fs::set_permissions(&restricted_dir, fs::Permissions::from_mode(0o700))
            .expect("chmod restricted dir");

        assert!(can_read_dir(dir.to_str().expect("dir path")));
        assert!(!can_read_dir(
            restricted_dir.to_str().expect("restricted dir path")
        ));
        assert!(can_list_dir(dir.to_str().expect("dir path")));
        assert!(can_read_file(file.to_str().expect("file path")));
        assert!(!can_list_dir(
            base.join("missing").to_str().expect("missing path")
        ));
        assert!(!can_read_file(
            base.join("missing.txt")
                .to_str()
                .expect("missing file path")
        ));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn build_proot_args_non_isolated_includes_prefix_and_mounts() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-shared-proot-args-nonisolated");
        let prefix = base.join("usr");
        let rootfs = base.join("rootfs");
        fs::create_dir_all(&prefix).expect("create prefix");
        fs::create_dir_all(rootfs.join("tmp")).expect("create rootfs tmp");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
        std::env::set_var("TMPDIR", base.join("cache").to_string_lossy().to_string());

        let args = build_proot_args(rootfs.to_str().expect("rootfs path"), false, true, &[]);
        let prefix_str = prefix.to_string_lossy().to_string();

        assert!(args.contains(&format!("--bind={}", prefix_str)));
        assert!(args.contains(&String::from("--bind=/dev")));
        assert!(args.contains(&String::from("--bind=/proc")));
        assert!(args.contains(&String::from("--bind=/sys")));
        assert!(args.contains(&String::from("--bind=/proc/self/fd:/dev/fd")));
        assert!(args.contains(&String::from("--bind=/dev/urandom:/dev/random")));
        assert!(args.contains(&format!(
            "--bind={}:/tmp",
            base.join("cache").to_string_lossy()
        )));
        assert!(!args.contains(&String::from("--link2symlink")));

        std::env::remove_var("APP_PREFIX");
        std::env::remove_var("TMPDIR");
        let _ = fs::remove_dir_all(base);
    }
}
