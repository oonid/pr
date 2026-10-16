use super::TestResult;
use std::path::Path;

fn detect_pm() -> Option<&'static str> {
    for p in &["/sbin/apk", "/usr/sbin/apk", "/usr/bin/apk"] {
        if Path::new(p).exists() {
            return Some("apk");
        }
    }
    for p in &["/usr/bin/apt", "/usr/bin/apt-get"] {
        if Path::new(p).exists() {
            return Some("apt");
        }
    }
    None
}

fn run_sh(cmd: &str) -> Result<String, String> {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", cmd])
        .output()
        .map_err(|e| format!("sh: {}", e))?;
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("");
    let stderr = std::str::from_utf8(&out.stderr).unwrap_or("");
    if !out.status.success() {
        let detail = if stdout.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        Err(format!(
            "exit={}: {}",
            out.status.code().unwrap_or(-1),
            detail
        ))
    } else {
        Ok(stdout.trim().to_string())
    }
}

fn tool_exists(name: &str) -> bool {
    run_sh(&format!("{} --version >/dev/null 2>&1", name)).is_ok()
}

fn all_tools_present() -> bool {
    tool_exists("vim") && tool_exists("cc") && tool_exists("rustc") && tool_exists("cargo")
}

pub fn probe() -> bool {
    true
}

pub fn test_detect_pm() -> TestResult {
    match detect_pm() {
        Some(_pm) => Ok(()),
        None => Err("no package manager found (apk or apt)".to_string()),
    }
}

pub fn test_update_repos() -> TestResult {
    match detect_pm() {
        Some("apk") => run_sh("apk update 2>&1").map(|_| ()),
        Some("apt") => run_sh("apt-get update -qq 2>&1").map(|_| ()),
        _ => Err("no package manager".to_string()),
    }
}

pub fn test_install_tools() -> TestResult {
    if all_tools_present() {
        return Ok(());
    }
    match detect_pm() {
        Some("apk") => {
            run_sh("apk add --no-progress vim gcc rust cargo git 2>&1")?;
        }
        Some("apt") => {
            run_sh("DEBIAN_FRONTEND=noninteractive apt-get install -y -qq vim gcc rustc cargo git 2>&1")?;
        }
        _ => return Err("no package manager".to_string()),
    }
    Ok(())
}

pub fn test_verify_vim() -> TestResult {
    let out = run_sh("vim --version 2>&1 | head -1")?;
    if out.contains("VIM") {
        Ok(())
    } else {
        Err(format!("unexpected: {}", &out[..out.len().min(80)]))
    }
}

pub fn test_verify_gcc() -> TestResult {
    let out = run_sh("cc --version 2>&1 | head -1")?;
    if !out.is_empty() {
        Ok(())
    } else {
        Err("cc --version empty".to_string())
    }
}

pub fn test_verify_rustc() -> TestResult {
    let out = run_sh("rustc --version 2>&1")?;
    if out.contains("rustc") {
        Ok(())
    } else {
        Err(format!("unexpected: {}", out))
    }
}

pub fn test_verify_cargo() -> TestResult {
    let out = run_sh("cargo --version 2>&1")?;
    if out.contains("cargo") {
        Ok(())
    } else {
        Err(format!("unexpected: {}", out))
    }
}

pub fn test_os_release() -> TestResult {
    let content = std::fs::read_to_string("/etc/os-release")
        .map_err(|e| format!("read /etc/os-release: {}", e))?;
    if content.contains("NAME=") && content.contains("ID=") {
        Ok(())
    } else {
        Err(format!("unexpected: {}", &content[..content.len().min(80)]))
    }
}
