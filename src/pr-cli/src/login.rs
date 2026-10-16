use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::shared::*;

#[cfg(target_os = "android")]
fn android_log(msg: &str) {
    unsafe {
        let c_msg = std::ffi::CString::new(msg).unwrap_or_default();
        let tag = b"PR\0".as_ptr() as *const i8;
        __android_log_write(4, tag, c_msg.as_ptr() as *const i8);
    }
}

#[cfg(target_os = "android")]
extern "C" {
    fn __android_log_write(prio: i32, tag: *const i8, text: *const i8) -> i32;
}

#[cfg(not(target_os = "android"))]
fn android_log(_msg: &str) {}

struct PasswdEntry {
    uid: u32,
    gid: u32,
    home: String,
    shell: String,
}

fn parse_passwd_entry(line: &str) -> Option<PasswdEntry> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 7 {
        return None;
    }
    Some(PasswdEntry {
        uid: fields[2].parse().ok()?,
        gid: fields[3].parse().ok()?,
        home: fields[5].to_string(),
        shell: fields[6].to_string(),
    })
}

fn find_user_in_passwd(passwd_path: &str, user: &str) -> Result<PasswdEntry, String> {
    let content = fs::read_to_string(passwd_path).map_err(|e| format!("read passwd: {}", e))?;
    for line in content.lines() {
        if let Some(entry) = parse_passwd_entry(line) {
            if line.starts_with(&format!("{}:", user)) {
                return Ok(entry);
            }
        }
    }
    Err(format!("no user '{}' defined in /etc/passwd", user))
}

fn update_etc_environment(rootfs: &str) {
    let env_path = format!("{}/etc/environment", rootfs);
    let _ = fs::set_permissions(&env_path, fs::Permissions::from_mode(0o644));

    let mut lines: Vec<String> = Vec::new();
    if let Ok(content) = fs::read_to_string(&env_path) {
        for line in content.lines() {
            let key = line.split('=').next().unwrap_or("");
            if !key.is_empty()
                && !key.starts_with("ANDROID_")
                && key != "BOOTCLASSPATH"
                && key != "DEX2OATBOOTCLASSPATH"
            {
                lines.push(line.to_string());
            }
        }
    }

    for var in &[
        "ANDROID_ART_ROOT",
        "ANDROID_DATA",
        "ANDROID_I18N_ROOT",
        "ANDROID_ROOT",
        "ANDROID_RUNTIME_ROOT",
        "ANDROID_TZDATA_ROOT",
        "BOOTCLASSPATH",
        "DEX2OATBOOTCLASSPATH",
    ] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                lines.push(format!("{}={}", var, val));
            }
        }
    }

    let _ = fs::write(&env_path, lines.join("\n") + "\n");
}

pub fn command_login(
    distro_name: &str,
    user: &str,
    isolated: bool,
    no_link2symlink: bool,
    custom_bind: &[String],
    extra_args: &[String],
) -> Result<(), String> {
    let Some((rootfs, _source_type)) = resolve_installed_rootfs(distro_name) else {
        println!();
        msg_error(&format!("distribution '{}' is not installed.", distro_name));
        println!();
        return Err("not installed".to_string());
    };

    if !Path::new(&rootfs).is_dir() {
        return Err("not installed".to_string());
    }

    let passwd_path = format!("{}/etc/passwd", rootfs);
    if !Path::new(&passwd_path).exists() {
        msg_error("the selected distribution doesn't have /etc/passwd.");
        return Err("no /etc/passwd".to_string());
    }

    // Follow the network the device is on now; a resolv.conf the user wrote is left alone.
    let resolv_conf_path = format!("{}/etc/resolv.conf", rootfs);
    let existing = std::fs::read_to_string(&resolv_conf_path).ok();
    if resolv_conf_is_managed(existing.as_deref()) {
        let _ = write_resolv_conf(
            &resolv_conf_path,
            &resolv_conf_contents(&network_dns_servers()),
        );
    }

    let entry = find_user_in_passwd(&passwd_path, user)?;

    update_etc_environment(&rootfs);

    let args = build_proot_args(&rootfs, isolated, no_link2symlink, custom_bind);

    let mut login_env: Vec<String> = vec![format!("PATH={}", get_default_path_env())];

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
                login_env.push(format!("{}={}", var, val));
            }
        }
    }

    // Read /etc/environment from rootfs
    let env_file = format!("{}/etc/environment", rootfs);
    if let Ok(content) = fs::read_to_string(&env_file) {
        for line in content.lines() {
            if line.contains('=') && !line.is_empty() {
                let clean = line.trim_matches('\'').trim_matches('"');
                if clean.contains('=')
                    && !login_env
                        .iter()
                        .any(|e| e.split('=').next() == clean.split('=').next())
                {
                    login_env.push(clean.to_string());
                }
            }
        }
    }

    // Build command
    let mut cmd_args: Vec<String> = Vec::new();
    cmd_args.push("/bin/sh".to_string());
    cmd_args.push("-l".to_string());

    if !extra_args.is_empty() {
        cmd_args.push("-c".to_string());
        cmd_args.push(extra_args.join(" "));
    }

    // The proot binary
    let proot = get_native_proot();

    let mut full_args = Vec::new();
    full_args.extend(args);
    full_args.extend(cmd_args);

    android_log(&format!("exec: proot {} (cwd=/)", full_args.join(" ")));

    let mut child_env: Vec<(String, String)> = Vec::new();
    for line in &login_env {
        if let Some((k, v)) = line.split_once('=') {
            child_env.push((k.to_string(), v.to_string()));
        }
    }
    child_env.push((
        "COLORTERM".to_string(),
        std::env::var("COLORTERM").unwrap_or_default(),
    ));
    child_env.push(("HOME".to_string(), entry.home.clone()));
    child_env.push(("USER".to_string(), user.to_string()));
    child_env.push((
        "TERM".to_string(),
        std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()),
    ));
    child_env.push(("LANG".to_string(), "en_US.UTF-8".to_string()));
    child_env.push(("TMPDIR".to_string(), "/tmp".to_string()));

    let mut cmd = Command::new(&proot);
    cmd.arg0("proot");

    for (k, v) in &build_proot_runtime_env() {
        cmd.env(k, v);
    }
    for (k, v) in &child_env {
        cmd.env(k, v);
    }

    cmd.args(&full_args);

    let err = cmd.exec();

    Err(format!("exec proot failed: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn parse_passwd_entry_accepts_valid_line() {
        let line = "alice:x:1000:1000:Alice:/home/alice:/bin/sh";
        let entry = parse_passwd_entry(line).expect("parse passwd line");
        assert_eq!(entry.uid, 1000);
        assert_eq!(entry.gid, 1000);
        assert_eq!(entry.home, "/home/alice");
        assert_eq!(entry.shell, "/bin/sh");
    }

    #[test]
    fn parse_passwd_entry_rejects_invalid_line() {
        assert!(parse_passwd_entry("alice:x:baduid:1000:Alice:/home/alice:/bin/sh").is_none());
        assert!(parse_passwd_entry("too:few:fields").is_none());
    }

    #[test]
    fn find_user_in_passwd_finds_expected_user() {
        let tmp_dir = unique_temp_dir("pr-cli-login-passwd");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let passwd = tmp_dir.join("passwd");
        fs::write(
            &passwd,
            "root:x:0:0:root:/root:/bin/sh\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .expect("write passwd");

        let entry = find_user_in_passwd(passwd.to_str().expect("passwd path"), "alice")
            .expect("find alice entry");
        assert_eq!(entry.home, "/home/alice");

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn find_user_in_passwd_returns_error_when_missing() {
        let tmp_dir = unique_temp_dir("pr-cli-login-passwd-missing");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");
        let passwd = tmp_dir.join("passwd");
        fs::write(&passwd, "root:x:0:0:root:/root:/bin/sh\n").expect("write passwd");

        let err = match find_user_in_passwd(passwd.to_str().expect("passwd path"), "alice") {
            Ok(_) => panic!("must fail for missing user"),
            Err(err) => err,
        };
        assert!(err.contains("no user 'alice' defined in /etc/passwd"));

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn update_etc_environment_filters_and_appends_android_vars() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-login-env");
        let rootfs = tmp_dir.join("rootfs");
        let etc = rootfs.join("etc");
        fs::create_dir_all(&etc).expect("create etc");
        fs::write(
            etc.join("environment"),
            "KEEP_ME=1\nANDROID_ROOT=old\nBOOTCLASSPATH=old\n",
        )
        .expect("seed environment");
        std::env::set_var("ANDROID_ROOT", "/system");
        std::env::set_var("BOOTCLASSPATH", "/apex/jars");

        update_etc_environment(rootfs.to_str().expect("rootfs path"));
        let content = fs::read_to_string(etc.join("environment")).expect("read environment");
        assert!(content.contains("KEEP_ME=1"));
        assert!(content.contains("ANDROID_ROOT=/system"));
        assert!(content.contains("BOOTCLASSPATH=/apex/jars"));
        assert!(!content.contains("ANDROID_ROOT=old"));
        assert!(!content.contains("BOOTCLASSPATH=old"));

        std::env::remove_var("ANDROID_ROOT");
        std::env::remove_var("BOOTCLASSPATH");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_login_returns_not_installed_when_rootfs_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-login-not-installed");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_login("debian", "root", false, false, &[], &[])
            .expect_err("must fail when distro not installed");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_login_requires_passwd_file() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-login-no-passwd");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(&rootfs).expect("create rootfs");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_login("debian", "root", false, false, &[], &[])
            .expect_err("must fail when /etc/passwd is missing");
        assert_eq!(err, "no /etc/passwd");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_login_returns_user_lookup_error_before_exec() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-login-missing-user");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        let etc = rootfs.join("etc");
        fs::create_dir_all(&etc).expect("create etc");
        fs::write(etc.join("passwd"), "root:x:0:0:root:/root:/bin/sh\n").expect("write passwd");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_login("debian", "alice", false, false, &[], &[])
            .expect_err("must fail when user does not exist");
        assert!(err.contains("no user 'alice' defined in /etc/passwd"));

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }
}
