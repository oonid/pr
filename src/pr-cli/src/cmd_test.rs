use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use libc;

use crate::shared::*;

const PIT_BIN: &str = "/tmp/pit";

const TEST_BINARY: &[u8] = include_bytes!(
    "../../proot-integration-test/target/aarch64-linux-android/release/proot-integration-test"
);

struct TapResult {
    passed: usize,
    failed: usize,
    skipped: usize,
    failures: Vec<String>,
}

fn deploy_binary(cache_dir: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::io::AsRawFd;
    let bin_path = format!("{}/pit", cache_dir);
    let mut f = fs::File::create(&bin_path).map_err(|e| format!("create {}: {}", bin_path, e))?;
    f.write_all(TEST_BINARY)
        .map_err(|e| format!("write {}: {}", bin_path, e))?;
    // Use fchmod on the open fd — more reliable than path-based chmod on Android.
    unsafe {
        libc::fchmod(f.as_raw_fd(), 0o755);
    }
    Ok(())
}

fn run_proot_command(rootfs: &str, cmd: &str) -> Result<String, String> {
    let proot = get_native_proot();
    let args = build_proot_args(rootfs, false, false, &[]);
    let runtime_env = build_proot_runtime_env();
    let child_env = build_proot_child_env();

    let full_args = {
        let mut a = args;
        a.push("/bin/sh".to_string());
        a.push("-c".to_string());
        a.push(cmd.to_string());
        a
    };

    let mut command = Command::new(&proot);
    command.arg0("proot");
    for (k, v) in &runtime_env {
        command.env(k, v);
    }
    for (k, v) in &child_env {
        command.env(k, v);
    }
    let output = command
        .args(&full_args)
        .output()
        .map_err(|e| format!("proot exec: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let stderr_trim = stderr.trim();
        let stdout_trim = stdout.trim();
        let detail = if stdout_trim.is_empty() {
            stderr_trim.to_string()
        } else {
            format!("{}\n{}", stderr_trim, stdout_trim)
        };
        return Err(format!(
            "exit={}: {}",
            output.status.code().unwrap_or(-1),
            detail
        ));
    }

    Ok(stdout)
}

fn parse_tap(tap_output: &str) -> TapResult {
    let mut result = TapResult {
        passed: 0,
        failed: 0,
        skipped: 0,
        failures: Vec::new(),
    };

    for line in tap_output.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("ok ") {
            if line.contains("# SKIP") {
                result.skipped += 1;
            } else {
                result.passed += 1;
            }
            let name = line
                .trim_start_matches("ok ")
                .split_once(' ')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default();
            println!(
                "  {} {}{}",
                "\x1b[32m✓\x1b[0m",
                name,
                if line.contains("# SKIP") {
                    " \x1b[33m(SKIP)\x1b[0m".to_string()
                } else {
                    "".to_string()
                }
            );
        } else if line.starts_with("not ok ") {
            result.failed += 1;
            let name = line
                .trim_start_matches("not ok ")
                .split_once(' ')
                .map(|(_, n)| n.to_string())
                .unwrap_or_default();
            result.failures.push(name.clone());
            println!("  {} {}", "\x1b[31m✗\x1b[0m", name);
        } else {
            println!("    {}", line);
        }
    }

    result
}

fn run_proot_streaming(rootfs: &str, cmd: &str) -> Result<(), String> {
    let proot = get_native_proot();
    // --link2symlink MUST remain enabled: Android's SELinux (untrusted_app domain)
    // blocks the link() syscall outright (EPERM), so every hard-link dpkg tries to
    // create fails without proot's interception.  The earlier lchown-ENOENT issue
    // was NOT caused by link2symlink itself but by L2S metadata landing in the
    // Android cache dir (a different filesystem), leaving dangling symlinks.
    // That is fixed by pre-creating .l2s inside the rootfs in install_tools().
    let args = build_proot_args(rootfs, true, false, &[]);
    let runtime_env = build_proot_runtime_env();
    let child_env = build_proot_child_env();

    let full_args = {
        let mut a = args;
        a.push("/bin/sh".to_string());
        a.push("-c".to_string());
        a.push(cmd.to_string());
        a
    };

    let mut child = Command::new(&proot);
    child.arg0("proot");
    for (k, v) in &runtime_env {
        child.env(k, v);
    }
    for (k, v) in &child_env {
        child.env(k, v);
    }
    let mut child = child
        .args(&full_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("proot spawn: {}", e))?;

    use std::io::{BufRead, BufReader};
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;

    let stdout_reader = BufReader::new(stdout);
    let stderr_reader = BufReader::new(stderr);

    for line in stdout_reader.lines() {
        let line = line.unwrap_or_default();
        if !line.is_empty() {
            println!("  {}", line);
        }
    }

    for line in stderr_reader.lines() {
        let line = line.unwrap_or_default();
        if !line.is_empty() {
            println!("  {}", line);
        }
    }

    let status = child.wait().map_err(|e| format!("proot wait: {}", e))?;
    if !status.success() {
        return Err(format!("exit={}", status.code().unwrap_or(-1)));
    }

    Ok(())
}

fn detect_package_manager(rootfs: &str) -> Result<&'static str, String> {
    let mut checked = Vec::new();
    for name in &["sbin/apk", "usr/sbin/apk", "usr/bin/apk"] {
        let path = format!("{}/{}", rootfs, name);
        checked.push(path.clone());
        if Path::new(&path).exists() {
            return Ok("apk");
        }
    }
    for name in &["usr/bin/apt", "usr/bin/apt-get"] {
        let path = format!("{}/{}", rootfs, name);
        checked.push(path.clone());
        if Path::new(&path).exists() {
            return Ok("apt");
        }
    }
    Err(format!(
        "no supported package manager found (apk or apt)\n  rootfs={}\n  checked={:?}",
        rootfs, checked
    ))
}

fn install_tools(rootfs: &str, pkg_mgr: &str) -> Result<(), String> {
    // Pre-create .l2s inside the rootfs before launching proot.
    // build_proot_args() sets PROOT_L2S_DIR to <rootfs>/.l2s when this directory
    // exists.  Without it, proot defaults L2S storage to the Android cache dir
    // (a different filesystem), so the content files for L2S symlinks cannot be
    // hard-linked from the rootfs — the symlinks become dangling and every
    // subsequent lchown on a .dpkg-new path fails with ENOENT.
    let _ = fs::create_dir_all(format!("{}/.l2s", rootfs));

    msg_status("Installing tools...");

    let cmd = match pkg_mgr {
        "apk" => {
            // openssh-client provides ssh/scp/ssh-keygen so git can use SSH keys.
            "apk update 2>&1 && \
             apk add --no-progress vim gcc rust cargo git openssh-client 2>&1"
        }
        "apt" => {
            // LC_ALL=C LANG=C: proot child env sets LANG=en_US.UTF-8 but the
            // Debian rootfs has no locale generated yet — perl and dpkg post-install
            // scripts call `locale` and fail if LANG refers to a missing locale.
            // PERL_BADLANG=0: suppresses perl's own locale-failure abort.
            // DEBIAN_FRONTEND=noninteractive: prevents debconf from opening a TTY.
            // --no-install-recommends: avoids packages whose post-install scripts
            // require a full systemd/desktop environment.
            // --force-unsafe-io: skip dpkg's fsync+rename verification pass.
            //
            // openssh-client is installed in a second pass after preparing
            // the environment to work around two Android proot limitations:
            //
            // 1. groupadd lock failure: groupadd uses link() to lock /etc/group.
            //    Android SELinux (untrusted_app) blocks link() → exit 10.
            //    Fix: replace groupadd/groupdel with no-op stubs.
            //
            // 2. chgrp '_ssh' failure: the postinst runs `chgrp _ssh <file>`
            //    after groupadd, but since the stub didn't write to /etc/group
            //    the group lookup fails.
            //    Fix: manually append _ssh / _sshd to /etc/group and /etc/gshadow
            //    before the second apt-get so chgrp finds the group.
            "export LC_ALL=C LANG=C PERL_BADLANG=0 DEBIAN_FRONTEND=noninteractive && \
             apt-get update -q 2>&1 && \
             apt-get install -y -q \
             -o Dpkg::Options::=--force-unsafe-io \
             --no-install-recommends \
             vim gcc rustc cargo git 2>&1 && \
             printf '#!/bin/sh\\nexit 0\\n' > /usr/sbin/groupadd && \
             chmod +x /usr/sbin/groupadd && \
             printf '#!/bin/sh\\nexit 0\\n' > /usr/sbin/groupdel && \
             chmod +x /usr/sbin/groupdel && \
             grep -q '^_ssh:' /etc/group   || echo '_ssh:x:101:'   >> /etc/group && \
             grep -q '^_sshd:' /etc/group  || echo '_sshd:x:102:'  >> /etc/group && \
             grep -q '^_ssh:' /etc/gshadow  || echo '_ssh:!::'  >> /etc/gshadow && \
             grep -q '^_sshd:' /etc/gshadow || echo '_sshd:!::' >> /etc/gshadow && \
             apt-get install -y -q \
             -o Dpkg::Options::=--force-unsafe-io \
             --no-install-recommends \
             openssh-client 2>&1"
        }
        _ => return Err(format!("unknown package manager: {}", pkg_mgr)),
    };

    run_proot_streaming(rootfs, cmd)
}

fn run_test_binary(rootfs: &str, suite: &str) -> Result<TapResult, String> {
    msg_status(&format!("Running test suite: {}...", suite));

    let cmd = format!("{} {} 2>&1", PIT_BIN, suite);
    let output = run_proot_command(rootfs, &cmd)?;

    let result = parse_tap(&output);
    Ok(result)
}

pub fn command_test(distro: &str, suite: Option<&str>, verbose: bool) -> Result<(), String> {
    let Some((rootfs, _source_type)) = resolve_installed_rootfs(distro) else {
        msg_error(&format!("distribution '{}' is not installed.", distro));
        return Err("not installed".to_string());
    };

    if !Path::new(&rootfs).is_dir() {
        msg_error(&format!("distribution '{}' is not installed.", distro));
        return Err("not installed".to_string());
    }

    println!();
    msg_status(&format!("Testing distro: {}", distro));
    println!();

    if verbose {
        println!("  Rootfs: {}", rootfs);
        println!();
    }

    let suites = [
        "distro", "clone", "readlink", "gcc", "rust", "git", "pipe", "general", "ssh", "chroot",
    ];
    let target_suites: Vec<&str> = match suite {
        Some(s) => {
            if suites.contains(&s) {
                vec![s]
            } else {
                return Err(format!(
                    "unknown suite '{}'. Available: {}",
                    s,
                    suites.join(", ")
                ));
            }
        }
        None => suites.to_vec(),
    };

    // Auto-install toolchain when tool suites (gcc, rust, git) are targeted and
    // the toolchain is not yet present.  Uses the distro's native package manager
    // (apk for Alpine, apt for Debian-based).
    let tool_suites = ["gcc", "rust", "git", "ssh"];
    let needs_tools = target_suites.iter().any(|s| tool_suites.contains(s));
    if needs_tools {
        let has_tools = Path::new(&format!("{}/usr/bin/rustc", rootfs)).exists()
            || Path::new(&format!("{}/usr/local/bin/rustc", rootfs)).exists();
        if !has_tools {
            let pkg_mgr = detect_package_manager(&rootfs)?;
            install_tools(&rootfs, pkg_mgr)?;
            println!();
        }
    }

    let cache_dir = std::env::var("PROOT_TMP_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| format!("{}/tmp", get_prefix()));

    msg_status("Deploying test binary...");
    deploy_binary(&cache_dir)?;

    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut total_skipped = 0;
    let mut all_failures: Vec<String> = Vec::new();

    for suite_name in &target_suites {
        match run_test_binary(&rootfs, suite_name) {
            Ok(result) => {
                total_passed += result.passed;
                total_failed += result.failed;
                total_skipped += result.skipped;
                all_failures.extend(result.failures);
            }
            Err(e) => {
                total_failed += 1;
                all_failures.push(format!("{}: {}", suite_name, e));
                println!("  {} {} — {}", "\x1b[31m✗\x1b[0m", suite_name, e);
            }
        }
        println!();
    }

    let total = total_passed + total_failed + total_skipped;
    println!(
        "\x1b[1mResults: {}/{} passed, {} failed, {} skipped\x1b[0m",
        total_passed, total, total_failed, total_skipped
    );

    if !all_failures.is_empty() {
        println!("\x1b[31mFailed:\x1b[0m");
        for f in &all_failures {
            println!("  - {}", f);
        }
        println!();
        return Err("tests failed".to_string());
    }

    println!();
    Ok(())
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
    fn parse_tap_counts_pass_fail_and_skip() {
        let tap =
            "ok 1 distro basics\nok 2 rust compile # SKIP missing rustc\nnot ok 3 git clone\n";
        let result = parse_tap(tap);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.skipped, 1);
        assert_eq!(result.failures, vec!["git clone".to_string()]);
    }

    #[test]
    fn parse_tap_ignores_non_tap_lines_and_empty_input() {
        let tap = "\nTAP version 13\n1..2\n# comment\n";
        let result = parse_tap(tap);
        assert_eq!(result.passed, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.skipped, 0);
        assert!(result.failures.is_empty());
    }

    #[test]
    fn detect_package_manager_finds_apk_then_apt() {
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-pkg");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("usr/bin")).expect("create rootfs dirs");

        fs::write(rootfs.join("usr/bin/apk"), b"").expect("write apk");
        assert_eq!(
            detect_package_manager(rootfs.to_str().expect("rootfs path")).expect("find apk"),
            "apk"
        );

        fs::remove_file(rootfs.join("usr/bin/apk")).expect("remove apk");
        fs::write(rootfs.join("usr/bin/apt-get"), b"").expect("write apt-get");
        assert_eq!(
            detect_package_manager(rootfs.to_str().expect("rootfs path")).expect("find apt"),
            "apt"
        );

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn detect_package_manager_returns_helpful_error_when_missing() {
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-pkg-missing");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&rootfs).expect("create rootfs");

        let err = detect_package_manager(rootfs.to_str().expect("rootfs path"))
            .expect_err("must fail without apk/apt");
        assert!(err.contains("no supported package manager found"));
        assert!(err.contains("checked="));

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn deploy_binary_writes_executable_file() {
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-deploy");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        deploy_binary(tmp_dir.to_str().expect("tmp path")).expect("deploy test binary");
        let deployed = tmp_dir.join("pit");
        assert!(deployed.exists());
        let mode = fs::metadata(&deployed)
            .expect("metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_test_rejects_unknown_suite_early() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-unknown-suite");
        let prefix = tmp_dir.join("usr");
        let legacy_rootfs = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(&legacy_rootfs).expect("create installed rootfs");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_test("debian", Some("unknown-suite"), false)
            .expect_err("must reject unknown suite");
        assert!(err.contains("unknown suite"));

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn parse_tap_handles_entries_without_test_name() {
        let tap = "ok 1\nnot ok 2\n";
        let result = parse_tap(tap);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.failures, vec!["".to_string()]);
    }

    #[test]
    fn detect_package_manager_prefers_sbin_locations_for_apk() {
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-apk-sbin");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("usr/sbin")).expect("create usr/sbin");

        fs::write(rootfs.join("usr/sbin/apk"), b"").expect("write usr/sbin/apk");
        assert_eq!(
            detect_package_manager(rootfs.to_str().expect("rootfs path")).expect("find apk"),
            "apk"
        );

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn deploy_binary_fails_when_cache_dir_is_missing() {
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-deploy-missing");
        let missing_dir = tmp_dir.join("no-such-dir");
        let err = deploy_binary(missing_dir.to_str().expect("missing path"))
            .expect_err("deploy should fail for missing parent dir");
        assert!(err.contains("create"));
    }

    #[test]
    fn install_tools_rejects_unknown_package_manager() {
        let err = install_tools("/tmp/irrelevant", "unknown").expect_err("must fail");
        assert!(err.contains("unknown package manager"));
    }

    #[test]
    fn run_proot_command_and_streaming_report_spawn_failures() {
        let err = run_proot_command("/", "echo hi").expect_err("spawn should fail");
        assert!(err.contains("proot exec"));

        let err = run_proot_streaming("/", "echo hi").expect_err("spawn should fail");
        assert!(err.contains("proot spawn"));
    }

    #[test]
    fn command_test_returns_not_installed_when_rootfs_missing() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-not-installed");
        let prefix = tmp_dir.join("usr");
        fs::create_dir_all(&prefix).expect("create prefix");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_test("debian", None, false).expect_err("must fail");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_test_returns_not_installed_when_rootfs_path_is_file() {
        let _guard = env_lock().lock().expect("lock env");
        let tmp_dir = unique_temp_dir("pr-cli-cmd-test-rootfs-file");
        let prefix = tmp_dir.join("usr");
        let rootfs_file = prefix.join("var/lib/pr/installed-rootfs/debian");
        fs::create_dir_all(rootfs_file.parent().expect("parent")).expect("create parent");
        fs::write(&rootfs_file, b"not a directory").expect("write file");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let err = command_test("debian", None, false).expect_err("must fail");
        assert_eq!(err, "not installed");

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(tmp_dir);
    }
}
