use super::TestResult;
use std::fs;

pub fn probe() -> bool {
    true
}

pub fn test_file_io() -> TestResult {
    let dir = "/tmp/pit-fileio";
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| format!("mkdir: {}", e))?;

    let file = format!("{}/test.txt", dir);
    fs::write(&file, "hello").map_err(|e| format!("write: {}", e))?;

    let content = fs::read_to_string(&file).map_err(|e| format!("read: {}", e))?;
    if content != "hello" {
        return Err(format!("content mismatch: {:?}", content));
    }

    let renamed = format!("{}/renamed.txt", dir);
    fs::rename(&file, &renamed).map_err(|e| format!("rename: {}", e))?;

    fs::remove_dir_all(dir).map_err(|e| format!("rm: {}", e))?;
    Ok(())
}

pub fn test_symlink_ops() -> TestResult {
    let link = "/tmp/pit-symlink";
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink("/bin/sh", link).map_err(|e| format!("symlink: {}", e))?;
    let target = fs::read_link(link).map_err(|e| format!("readlink: {}", e))?;
    fs::remove_file(link).map_err(|e| format!("remove: {}", e))?;
    if target.to_str() == Some("/bin/sh") {
        Ok(())
    } else {
        Err(format!("expected /bin/sh, got {:?}", target))
    }
}

pub fn test_pipe() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "echo foo | cat | wc -c"])
        .output()
        .map_err(|e| format!("pipe: {}", e))?;
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("").trim();
    let count: usize = stdout.parse().unwrap_or(0);
    if count == 4 {
        Ok(())
    } else {
        Err(format!("expected 4 bytes, got {}", count))
    }
}

pub fn test_signal() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "trap 'echo caught' INT; kill -INT $$; echo not-caught",
        ])
        .output()
        .map_err(|e| format!("signal: {}", e))?;
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("");
    if stdout.contains("caught") || stdout.contains("not-caught") {
        Ok(())
    } else {
        Err(format!("unexpected signal output: {:?}", stdout))
    }
}

pub fn test_touch_creates_file() -> TestResult {
    let file = "/tmp/pit-touch";
    let _ = fs::remove_file(file);
    let missing = std::ffi::CString::new(file).unwrap();
    let rc = unsafe { libc::utimensat(libc::AT_FDCWD, missing.as_ptr(), std::ptr::null(), 0) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    if rc != -1 || errno != Some(libc::ENOENT) {
        return Err(format!(
            "utimensat on missing file: rc={} errno={:?}",
            rc, errno
        ));
    }
    let out = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "touch /tmp/pit-touch && test -f /tmp/pit-touch && touch /tmp/pit-touch && echo ok",
        ])
        .output()
        .map_err(|e| format!("touch: {}", e))?;
    let _ = fs::remove_file(file);
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("").trim();
    if stdout == "ok" {
        Ok(())
    } else {
        Err(format!("touch did not create the file: {:?}", stdout))
    }
}

// The loader rewrites auxv on the initial stack (AT_EXECFN, AT_PHDR, AT_ENTRY,
// AT_BASE, fake-root ids); getauxval() reads that copy. The kernel's own views
// must agree, or programs that read them (uutils coreutils via rustix) see the
// loader instead of themselves.
fn check_auxv_pairs(source: &str, bytes: &[u8]) -> TestResult {
    let mut mismatches = Vec::new();
    for pair in bytes.chunks_exact(16) {
        let kind = u64::from_ne_bytes(pair[..8].try_into().unwrap());
        let value = u64::from_ne_bytes(pair[8..].try_into().unwrap());
        if kind == 0 {
            return if mismatches.is_empty() {
                Ok(())
            } else {
                Err(format!("{}: {}", source, mismatches.join(", ")))
            };
        }
        let stack = unsafe { libc::getauxval(kind as libc::c_ulong) } as u64;
        if stack != value {
            mismatches.push(format!("type {} is {:#x} not {:#x}", kind, value, stack));
        }
    }
    Err(format!("{}: no AT_NULL terminator", source))
}

pub fn test_auxv_matches_stack() -> TestResult {
    let raw = fs::read("/proc/self/auxv").map_err(|e| format!("/proc/self/auxv: {}", e))?;
    check_auxv_pairs("/proc/self/auxv", &raw)?;

    const PR_GET_AUXV: libc::c_int = 0x4155_5856;
    let mut buf = vec![0u8; 4096];
    let rc = unsafe { libc::prctl(PR_GET_AUXV, buf.as_mut_ptr(), buf.len(), 0, 0) };
    if rc < 0 {
        // Kernels before 6.4 have no PR_GET_AUXV.
        let errno = std::io::Error::last_os_error().raw_os_error();
        return if errno == Some(libc::EINVAL) {
            Ok(())
        } else {
            Err(format!("PR_GET_AUXV: errno={:?}", errno))
        };
    }
    let len = (rc as usize).min(buf.len());
    check_auxv_pairs("PR_GET_AUXV", &buf[..len])
}

pub fn test_env() -> TestResult {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", "echo $HOME"])
        .env("HOME", "/root")
        .output()
        .map_err(|e| format!("env: {}", e))?;
    let stdout = std::str::from_utf8(&out.stdout).unwrap_or("").trim();
    if stdout == "/root" {
        Ok(())
    } else {
        Err(format!("expected /root, got {:?}", stdout))
    }
}
