use super::TestResult;
use std::ffi::CString;
use std::fs;
use std::path::Path;

pub fn probe() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn c(path: &str) -> CString {
    CString::new(path).unwrap()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn expect_errno(what: &str, rc: libc::c_int, err: i32, want: i32) -> TestResult {
    if rc == -1 && err == want {
        Ok(())
    } else {
        Err(format!(
            "{}: rc={} errno={}, want errno {}",
            what, rc, err, want
        ))
    }
}

fn in_child(body: impl FnOnce() -> i32) -> Result<i32, String> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!("fork: {}", std::io::Error::last_os_error()));
    }
    if pid == 0 {
        let code = body();
        unsafe { libc::_exit(code) };
    }
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } != pid {
        return Err(format!("waitpid: {}", std::io::Error::last_os_error()));
    }
    if !libc::WIFEXITED(status) {
        return Err(format!("child did not exit: status={:#x}", status));
    }
    Ok(libc::WEXITSTATUS(status))
}

pub fn test_chown_existing_targets() -> TestResult {
    let file = "/tmp/pit-fr-file";
    let link = "/tmp/pit-fr-dangling";
    let _ = fs::remove_file(link);
    fs::write(file, b"x").map_err(|e| format!("write: {}", e))?;
    std::os::unix::fs::symlink("/tmp/pit-fr-nowhere", link)
        .map_err(|e| format!("symlink: {}", e))?;
    let chown_rc = unsafe { libc::chown(c(file).as_ptr(), 0, 0) };
    let chown_err = errno();
    let lchown_rc = unsafe { libc::lchown(c(link).as_ptr(), 0, 0) };
    let lchown_err = errno();
    let _ = fs::remove_file(file);
    let _ = fs::remove_file(link);
    if chown_rc != 0 {
        return Err(format!("chown of an existing file: errno {}", chown_err));
    }
    if lchown_rc != 0 {
        return Err(format!(
            "lchown of a dangling symlink: errno {}",
            lchown_err
        ));
    }
    Ok(())
}

pub fn test_chown_hard_linked_file() -> TestResult {
    let a = "/tmp/pit-fr-hl-a";
    let b = "/tmp/pit-fr-hl-b";
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    fs::write(a, b"x").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(a, b).map_err(|e| format!("link: {}", e))?;
    let rcs = [
        unsafe { libc::chown(c(a).as_ptr(), 0, 0) },
        unsafe { libc::chown(c(b).as_ptr(), 0, 0) },
        unsafe { libc::lchown(c(b).as_ptr(), 0, 0) },
    ];
    let err = errno();
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    if rcs == [0, 0, 0] {
        Ok(())
    } else {
        Err(format!(
            "chown/chown/lchown of a hard link: {:?}, errno {}",
            rcs, err
        ))
    }
}

pub fn test_chown_missing_target_is_enoent() -> TestResult {
    let missing = "/tmp/pit-fr-missing";
    let link = "/tmp/pit-fr-dangling2";
    let _ = fs::remove_file(missing);
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink(missing, link).map_err(|e| format!("symlink: {}", e))?;
    let rc = unsafe { libc::chown(c(missing).as_ptr(), 0, 0) };
    let err = errno();
    let follow_rc = unsafe { libc::chown(c(link).as_ptr(), 0, 0) };
    let follow_err = errno();
    let _ = fs::remove_file(link);
    expect_errno("chown of a missing path", rc, err, libc::ENOENT)?;
    expect_errno(
        "chown through a dangling symlink",
        follow_rc,
        follow_err,
        libc::ENOENT,
    )
}

pub fn test_fchown_closed_fd_is_ebadf() -> TestResult {
    let file = "/tmp/pit-fr-fd";
    fs::write(file, b"x").map_err(|e| format!("write: {}", e))?;
    let fd = unsafe { libc::open(c(file).as_ptr(), libc::O_RDONLY) };
    let _ = fs::remove_file(file);
    if fd < 0 {
        return Err(format!("open: errno {}", errno()));
    }
    unsafe { libc::close(fd) };
    let rc = unsafe { libc::fchown(fd, 0, 0) };
    expect_errno("fchown of a closed fd", rc, errno(), libc::EBADF)?;
    let rc = unsafe { libc::fchown(libc::AT_FDCWD, 0, 0) };
    expect_errno("fchown(AT_FDCWD)", rc, errno(), libc::EBADF)
}

pub fn test_owner_follows_effective_ids() -> TestResult {
    let file = "/var/tmp/pit-fr-owner";
    fs::write(file, b"x").map_err(|e| format!("write: {}", e))?;
    let path = c(file);
    let code = in_child(|| unsafe {
        if libc::setegid(1000) != 0 || libc::seteuid(1000) != 0 {
            return 10;
        }
        let mut st: libc::stat = std::mem::zeroed();
        if libc::stat(path.as_ptr(), &mut st) != 0 {
            return 11;
        }
        let mut stx: libc::statx = std::mem::zeroed();
        let wanted = libc::STATX_UID | libc::STATX_GID;
        if libc::syscall(
            libc::SYS_statx,
            libc::AT_FDCWD,
            path.as_ptr(),
            0,
            wanted,
            &mut stx as *mut libc::statx,
        ) != 0
        {
            return 12;
        }
        if (st.st_uid, st.st_gid) != (1000, 1000) {
            return 20;
        }
        if (stx.stx_uid, stx.stx_gid) != (1000, 1000) {
            return 21;
        }
        0
    });
    let _ = fs::remove_file(file);
    match code? {
        0 => Ok(()),
        10 => Err("setegid/seteuid(1000) failed".to_string()),
        11 | 12 => Err("stat/statx failed after dropping the effective ids".to_string()),
        20 => Err("stat shows the saved ids, not the effective ids 1000:1000".to_string()),
        21 => Err("statx shows the saved ids, not the effective ids 1000:1000".to_string()),
        other => Err(format!("child exit {}", other)),
    }
}

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_TRAP: u32 = 0x0003_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;

fn trap_linkat() -> bool {
    let filter = [
        libc::sock_filter {
            code: BPF_LD_W_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: BPF_JMP_JEQ_K,
            jt: 0,
            jf: 1,
            k: libc::SYS_linkat as u32,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_TRAP,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
    ];
    let prog = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr() as *mut libc::sock_filter,
    };
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0
            && libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &prog as *const libc::sock_fprog,
            ) == 0
    }
}

pub fn test_blocked_linkat_keeps_the_source() -> TestResult {
    let a = "/tmp/pit-fr-link-a";
    let b = "/tmp/pit-fr-link-b";
    let _ = fs::remove_file(b);
    fs::write(a, b"source").map_err(|e| format!("write: {}", e))?;
    let (pa, pb) = (c(a), c(b));
    let code = in_child(|| unsafe {
        if !trap_linkat() {
            return 10;
        }
        let rc = libc::syscall(
            libc::SYS_linkat,
            libc::AT_FDCWD,
            pa.as_ptr(),
            libc::AT_FDCWD,
            pb.as_ptr(),
            0,
        );
        if libc::access(pa.as_ptr(), libc::F_OK) != 0 {
            return 20;
        }
        if rc == 0 {
            return 0;
        }
        if errno() == libc::EPERM {
            0
        } else {
            30
        }
    });
    let source_left = Path::new(a).exists();
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    match code? {
        0 if source_left => Ok(()),
        0 => Err("source vanished after the child exited".to_string()),
        10 => Err("could not install the seccomp filter".to_string()),
        20 => Err("linkat removed the source name".to_string()),
        30 => Err("blocked linkat failed with an errno other than EPERM".to_string()),
        other => Err(format!("child exit {}", other)),
    }
}
