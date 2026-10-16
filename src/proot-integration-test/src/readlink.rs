use super::TestResult;
use std::fs;

pub fn probe() -> bool {
    std::path::Path::new("/bin/sh").exists()
}

pub fn test_symlink_resolve() -> TestResult {
    let tmp = "/tmp/pit-readlink-sym";
    let _ = fs::remove_file(tmp);
    std::os::unix::fs::symlink("/bin/sh", tmp).map_err(|e| format!("symlink: {}", e))?;
    let target = fs::read_link(tmp).map_err(|e| format!("readlink: {}", e))?;
    let _ = fs::remove_file(tmp);
    if target.to_str() == Some("/bin/sh") {
        Ok(())
    } else {
        Err(format!("expected /bin/sh, got {:?}", target))
    }
}

pub fn test_realpath_no_l2s() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "realpath /usr/bin/cc 2>&1 || realpath /bin/sh 2>&1"])
        .output()
        .map_err(|e| format!("realpath: {}", e))?;
    let path = std::str::from_utf8(&out.stdout).unwrap_or("").trim();
    if path.contains(".l2s") {
        Err(format!("realpath contains .l2s: {}", path))
    } else if path.is_empty() {
        Err("realpath returned empty".to_string())
    } else {
        Ok(())
    }
}

pub fn test_readlink_einval() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "readlink /usr/bin/gcc 2>&1; echo exit=$?"])
        .output()
        .map_err(|e| format!("readlink: {}", e))?;
    let output = std::str::from_utf8(&out.stdout).unwrap_or("");
    if output.contains("exit=0") && !output.contains(".l2s") {
        Ok(())
    } else if output.contains("Invalid") || output.contains("exit=1") {
        Ok(())
    } else {
        Err(format!("unexpected readlink output: {:?}", output))
    }
}

pub fn test_proc_self_exe() -> TestResult {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {}", e))?;
    let path = exe.to_str().unwrap_or("");
    if path.contains(".l2s") {
        Err(format!("/proc/self/exe contains .l2s: {}", path))
    } else {
        Ok(())
    }
}

pub fn test_lstat_stat() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "stat -c '%s' /bin/sh && stat -L -c '%s' /bin/sh"])
        .output()
        .map_err(|e| format!("stat: {}", e))?;
    let lines: Vec<&str> = std::str::from_utf8(&out.stdout)
        .unwrap_or("")
        .lines()
        .collect();
    if lines.len() >= 2 {
        Ok(())
    } else {
        Err(format!("expected 2 stat lines, got {:?}", lines))
    }
}

pub fn test_readlink_small_buffer() -> TestResult {
    let tmp = "/tmp/pit-readlink-smallbuf";
    let _ = fs::remove_file(tmp);
    let target = "/usr/lib/gcc/aarch64-alpine-linux-musl/14.2.0";
    std::os::unix::fs::symlink(target, tmp).map_err(|e| format!("symlink: {}", e))?;

    let c_src = r#"#include <unistd.h>
#include <stdio.h>
int main(){char b[8];ssize_t n=readlink("/tmp/pit-readlink-smallbuf",b,4);
if(n<0)return 1;b[n]='\0';printf("%s\n",b);return 0;}"#;
    let c_path = "/tmp/pit-rlbuf.c";
    let bin_path = "/tmp/pit-rlbuf";
    fs::write(c_path, c_src).map_err(|e| format!("write c src: {}", e))?;

    let compile = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("cc {} -o {} 2>&1", c_path, bin_path)])
        .output()
        .map_err(|e| format!("compile: {}", e))?;
    if !compile.status.success() {
        let _ = fs::remove_file(tmp);
        let _ = fs::remove_file(c_path);
        return Err(format!(
            "cc failed: {}",
            std::str::from_utf8(&compile.stdout).unwrap_or("?")
        ));
    }

    let run = std::process::Command::new("/bin/sh")
        .args(["-c", &format!("{} 2>&1", bin_path)])
        .output()
        .map_err(|e| format!("run: {}", e))?;

    let _ = fs::remove_file(tmp);
    let _ = fs::remove_file(c_path);
    let _ = fs::remove_file(bin_path);

    if !run.status.success() {
        return Err(format!(
            "readlink test binary failed: {}",
            std::str::from_utf8(&run.stderr).unwrap_or("?")
        ));
    }

    let got = std::str::from_utf8(&run.stdout).unwrap_or("").trim();
    let expected = &target[..4];
    if got == expected && !got.contains(".l2s") {
        Ok(())
    } else {
        Err(format!("expected {:?}, got {:?}", expected, got))
    }
}

// An fd link names the open file even when that file is a fake hard link (an L2S final file).
pub fn test_proc_fd_of_hard_link() -> TestResult {
    use std::os::unix::fs::MetadataExt;
    let a = "/tmp/pit-hl-a";
    let b = "/tmp/pit-hl-b";
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    fs::write(a, b"hardlink").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(a, b).map_err(|e| format!("link: {}", e))?;
    let file = fs::File::open(b).map_err(|e| format!("open: {}", e))?;
    let fd_link = format!(
        "/proc/self/fd/{}",
        std::os::unix::io::AsRawFd::as_raw_fd(&file)
    );
    let result = fs::read_link(&fd_link)
        .map_err(|e| format!("readlink {}: {}", fd_link, e))
        .and_then(|target| {
            let ino = file.metadata().map_err(|e| e.to_string())?.ino();
            let target_ino = fs::metadata(&target)
                .map_err(|e| format!("stat {:?}: {}", target, e))?
                .ino();
            if target_ino == ino {
                Ok(())
            } else {
                Err(format!("{:?} is not the open file", target))
            }
        });
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    result
}
