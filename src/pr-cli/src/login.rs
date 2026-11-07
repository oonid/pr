use std::fs;
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
    home: String,
}

fn parse_passwd_entry(line: &str) -> Option<PasswdEntry> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < 7 {
        return None;
    }
    Some(PasswdEntry {
        home: fields[5].to_string(),
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
    let mut lines: Vec<String> = Vec::new();
    let Ok(existing) = read_rootfs_file(Path::new(rootfs), "etc/environment") else {
        return;
    };
    let app_bin = std::path::PathBuf::from(get_prefix()).join("bin");
    let mut app_bins = vec![app_bin.clone()];
    app_bins.extend(path_alias(&app_bin));
    if let Some(content) = existing {
        for line in content.lines() {
            let key = line.split('=').next().unwrap_or("");
            if let Some(path) = line.strip_prefix("PATH=") {
                let kept: Vec<&str> = path
                    .split(':')
                    .filter(|dir| !app_bins.iter().any(|bin| Path::new(dir) == bin))
                    .collect();
                lines.push(format!("PATH={}", kept.join(":")));
            } else if !key.is_empty()
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

    let _ = write_rootfs_file(
        Path::new(rootfs),
        "etc/environment",
        &(lines.join("\n") + "\n"),
        Some(0o644),
    );
}

fn refresh_resolv_conf(rootfs: &Path) {
    let Ok(path) = crate::oci::resolve_in_root(rootfs, Path::new("etc/resolv.conf"), false) else {
        return;
    };
    let resolv_conf_path = path.to_string_lossy().into_owned();
    let existing = read_existing_resolv_conf(&resolv_conf_path);
    if resolv_conf_is_managed(existing.as_deref()) {
        let _ = write_resolv_conf(
            &resolv_conf_path,
            &resolv_conf_contents(&network_dns_servers()),
        );
    }
}

fn environment_entries(content: &str) -> Vec<(String, String)> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let mut chars = key.chars();
            let first = chars.next()?;
            let valid = (first.is_ascii_alphabetic() || first == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
            valid.then(|| (key.to_string(), unquote(value).to_string()))
        })
        .collect()
}

fn unquote(value: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|q| value.strip_prefix(*q)?.strip_suffix(*q))
        .unwrap_or(value)
}

fn guest_command(extra_args: &[String]) -> Vec<String> {
    let mut cmd_args = vec!["/bin/sh".to_string(), "-l".to_string()];
    match extra_args {
        [] => {}
        [command] => cmd_args.extend(["-c".to_string(), command.clone()]),
        words => {
            let quoted: Vec<String> = words
                .iter()
                .map(|word| format!("'{}'", word.replace('\'', "'\\''")))
                .collect();
            cmd_args.extend(["-c".to_string(), quoted.join(" ")]);
        }
    }
    cmd_args
}

fn login_process_env(
    inherited: impl Iterator<Item = (String, String)>,
    runtime: Vec<(String, String)>,
    guest: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let proot_settings = inherited.filter(|(k, _)| k.starts_with("PROOT_"));
    for (key, value) in proot_settings.chain(runtime).chain(guest) {
        env.retain(|(k, _)| *k != key);
        env.push((key, value));
    }
    env
}

fn login_command(proot: &Path, args: &[String], env: Vec<(String, String)>) -> Command {
    let mut cmd = Command::new(proot);
    cmd.arg0("proot");
    cmd.env_clear();
    cmd.envs(env);
    cmd.args(args);
    cmd
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
        if let Some((_, _, state)) = pending_container(distro_name) {
            msg_error(&match state {
                crate::install_marker::PendingInstall::InProgress => {
                    format!("another pr-cli process is changing '{}'; try again once it finishes.", distro_name)
                }
                crate::install_marker::PendingInstall::Interrupted => format!(
                    "an install or removal of '{}' did not finish. Install it again, or remove it with: pr-cli remove {}",
                    distro_name, distro_name
                ),
            });
            println!();
            return Err("not fully installed".to_string());
        }
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

    refresh_resolv_conf(Path::new(&rootfs));

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

    let env_file = format!("{}/etc/environment", rootfs);
    if let Ok(content) = fs::read_to_string(&env_file) {
        for (key, value) in environment_entries(&content) {
            if !login_env
                .iter()
                .any(|e| e.split('=').next() == Some(key.as_str()))
            {
                login_env.push(format!("{}={}", key, value));
            }
        }
    }

    let cmd_args = guest_command(extra_args);

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

    let runtime_env: Vec<(String, String)> = build_proot_runtime_env()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let mut cmd = login_command(
        Path::new(&proot),
        &full_args,
        login_process_env(std::env::vars(), runtime_env, child_env),
    );

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

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_login_without_a_command_starts_a_login_shell() {
        assert_eq!(guest_command(&[]), strings(&["/bin/sh", "-l"]));
    }

    #[test]
    fn a_single_command_runs_as_a_shell_command() {
        assert_eq!(
            guest_command(&strings(&["cd /root && exec hermes gateway run"])),
            strings(&["/bin/sh", "-l", "-c", "cd /root && exec hermes gateway run"])
        );
    }

    #[test]
    fn several_arguments_reach_the_program_as_separate_words() {
        assert_eq!(
            guest_command(&strings(&["touch", "a b", "it's", "$HOME;x"])),
            strings(&["/bin/sh", "-l", "-c", "'touch' 'a b' 'it'\\''s' '$HOME;x'"])
        );
    }

    #[test]
    fn environment_values_lose_matching_quotes() {
        let content = "PATH=\"/usr/bin:/bin\"\nLANG='C.UTF-8'\n# NOTE=comment\n\nexport EDITOR=vi\nNOEQUALS\nBAD KEY=x\nEMPTY=\nMIXED=\"a'\n";
        let expected: Vec<(String, String)> = [
            ("PATH", "/usr/bin:/bin"),
            ("LANG", "C.UTF-8"),
            ("EDITOR", "vi"),
            ("EMPTY", ""),
            ("MIXED", "\"a'"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(environment_entries(content), expected);
    }

    #[test]
    fn the_guest_gets_only_proot_settings_from_the_app_environment() {
        let inherited = [
            ("APP_PREFIX", "/data/user/0/app/files/usr"),
            ("HOME", "/data/user/0/app/files/home"),
            ("PROOT_VERBOSE", "1"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let env = login_process_env(
            inherited,
            vec![("PROOT_TMP_DIR".to_string(), "/cache".to_string())],
            vec![("HOME".to_string(), "/root".to_string())],
        );
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"APP_PREFIX"), "{:?}", env);
        assert!(env.contains(&("PROOT_VERBOSE".to_string(), "1".to_string())));
        assert!(env.contains(&("PROOT_TMP_DIR".to_string(), "/cache".to_string())));
        assert_eq!(
            env.iter().filter(|(k, _)| k == "HOME").count(),
            1,
            "{:?}",
            env
        );
        assert!(env.contains(&("HOME".to_string(), "/root".to_string())));
    }

    #[test]
    fn the_login_process_starts_with_only_the_given_environment() {
        let _guard = env_lock().lock().expect("lock env");
        std::env::set_var("PR_CLI_TEST_LEAK", "leaked");
        let output = login_command(
            Path::new("/usr/bin/env"),
            &[],
            vec![("ONLY".to_string(), "this".to_string())],
        )
        .output()
        .expect("run env");
        std::env::remove_var("PR_CLI_TEST_LEAK");
        let printed = String::from_utf8_lossy(&output.stdout);
        assert_eq!(printed.trim(), "ONLY=this");
    }

    #[test]
    fn parse_passwd_entry_accepts_valid_line() {
        let line = "alice:x:1000:1000:Alice:/home/alice:/bin/sh";
        let entry = parse_passwd_entry(line).expect("parse passwd line");
        assert_eq!(entry.home, "/home/alice");
    }

    #[test]
    fn parse_passwd_entry_rejects_invalid_line() {
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
    fn update_etc_environment_drops_the_app_bin_from_path() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp_dir = unique_temp_dir("pr-cli-login-env-path");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        let prefix = "/data/data/id.or.oo.pr/files/usr";
        std::env::set_var("APP_PREFIX", prefix);
        fs::write(
            rootfs.join("etc/environment"),
            format!("PATH=/usr/bin:/bin:{}/bin\nKEEP=1\n", prefix),
        )
        .expect("seed environment");

        update_etc_environment(rootfs.to_str().expect("path"));

        std::env::remove_var("APP_PREFIX");
        let content = fs::read_to_string(rootfs.join("etc/environment")).expect("read");
        assert!(content.contains("PATH=/usr/bin:/bin\n"), "{}", content);
        assert!(content.contains("KEEP=1"));
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn update_etc_environment_drops_the_app_bin_under_its_other_name() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp_dir = unique_temp_dir("pr-cli-login-env-path-alias");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        std::env::set_var("APP_PREFIX", "/data/data/id.or.oo.pr/files/usr");
        fs::write(
            rootfs.join("etc/environment"),
            "PATH=/usr/bin:/data/user/0/id.or.oo.pr/files/usr/bin:/bin\n",
        )
        .expect("seed environment");

        update_etc_environment(rootfs.to_str().expect("path"));

        std::env::remove_var("APP_PREFIX");
        let content = fs::read_to_string(rootfs.join("etc/environment")).expect("read");
        assert!(content.contains("PATH=/usr/bin:/bin\n"), "{}", content);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn update_etc_environment_drops_the_app_bin_of_a_custom_prefix() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp_dir = unique_temp_dir("pr-cli-login-env-path-custom");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        std::env::set_var("APP_PREFIX", "/data/data/id.or.oo.pr/custom/usr");
        fs::write(
            rootfs.join("etc/environment"),
            "PATH=/usr/bin:/data/user/0/id.or.oo.pr/custom/usr/bin:/bin\n",
        )
        .expect("seed environment");

        update_etc_environment(rootfs.to_str().expect("path"));

        std::env::remove_var("APP_PREFIX");
        let content = fs::read_to_string(rootfs.join("etc/environment")).expect("read");
        assert!(content.contains("PATH=/usr/bin:/bin\n"), "{}", content);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn refresh_resolv_conf_does_not_write_through_an_etc_symlink() {
        let tmp_dir = unique_temp_dir("pr-cli-login-resolv-symlink");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        let managed = "nameserver 8.8.8.8\nnameserver 8.8.4.4\n";
        fs::write(outside.join("resolv.conf"), managed).expect("seed host file");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&rootfs).expect("create rootfs");
        std::os::unix::fs::symlink(&outside, rootfs.join("etc")).expect("symlink etc");

        refresh_resolv_conf(&rootfs);

        assert_eq!(
            fs::read_to_string(outside.join("resolv.conf")).unwrap(),
            managed
        );
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn update_etc_environment_does_not_write_through_a_symlink() {
        let tmp_dir = unique_temp_dir("pr-cli-login-env-symlink");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        fs::write(outside.join("environment"), "HOST=1\n").expect("seed host file");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        std::os::unix::fs::symlink(outside.join("environment"), rootfs.join("etc/environment"))
            .expect("symlink environment");

        update_etc_environment(rootfs.to_str().expect("path"));

        assert_eq!(
            fs::read_to_string(outside.join("environment")).unwrap(),
            "HOST=1\n"
        );
        let inside = rootfs.join("etc/environment");
        assert!(fs::symlink_metadata(&inside).unwrap().is_file());
        assert!(!fs::read_to_string(&inside).unwrap().contains("HOST=1"));
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
    fn command_login_refuses_an_interrupted_install() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-login-interrupted");
        let prefix = tmp_dir.join("usr");
        let rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(rootfs.join("etc")).expect("create rootfs");
        fs::write(rootfs.join("etc/passwd"), "root:x:0:0:root:/root:/bin/sh\n").expect("passwd");
        drop(
            crate::install_marker::InstallMarker::claim(
                &prefix.join("var/lib/pr/installed-rootfs"),
                "debian",
            )
            .expect("mark"),
        );
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_login("debian", "root", false, false, &[], &[])
            .expect_err("must refuse an interrupted install");

        std::env::remove_var("APP_PREFIX");
        assert_eq!(err, "not fully installed");
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
