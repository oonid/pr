use super::TestResult;
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::PermissionsExt;

const SYS_RENAMEAT2: libc::c_long = 276;
const SYS_OPENAT2: libc::c_long = 437;
const SYS_FACCESSAT2: libc::c_long = 439;
const SYS_EPOLL_PWAIT2: libc::c_long = 441;
const SYS_FCHMODAT2: libc::c_long = 452;
const SYS_SETXATTRAT: libc::c_long = 463;

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

#[repr(C)]
struct XattrArgs {
    value: u64,
    size: u32,
    flags: u32,
}

pub fn probe() -> bool {
    true
}

fn c(path: &str) -> CString {
    CString::new(path).unwrap()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn expect_enosys(what: &str, rc: libc::c_long, err: i32) -> TestResult {
    if rc == -1 && err == libc::ENOSYS {
        Ok(())
    } else {
        Err(format!(
            "{}: rc={} errno={}, want -1 with ENOSYS",
            what, rc, err
        ))
    }
}

pub fn test_openat2_is_enosys() -> TestResult {
    let file = "/tmp/pit-sc-openat2";
    let _ = fs::remove_file(file);
    fs::write(file, b"data").map_err(|e| format!("write: {}", e))?;
    let path = c(file);
    let how = OpenHow {
        flags: libc::O_RDONLY as u64,
        mode: 0,
        resolve: 0,
    };
    let mut failures = Vec::new();
    for i in 0..20 {
        let rc = unsafe {
            libc::syscall(
                SYS_OPENAT2,
                libc::AT_FDCWD,
                path.as_ptr(),
                &how as *const OpenHow,
                std::mem::size_of::<OpenHow>(),
            )
        };
        let err = errno();
        if rc >= 0 {
            unsafe { libc::close(rc as libc::c_int) };
            failures.push(format!("call {}: returned fd {}", i, rc));
        } else if let Err(e) = expect_enosys(&format!("call {}", i), rc, err) {
            failures.push(e);
        }
    }
    let len = fs::metadata(file).map(|m| m.len());
    let _ = fs::remove_file(file);
    if !failures.is_empty() {
        return Err(format!("openat2: {}", failures.join("; ")));
    }
    match len {
        Ok(4) => Ok(()),
        Ok(n) => Err(format!("file is {} bytes after openat2, want 4", n)),
        Err(e) => Err(format!("stat after openat2: {}", e)),
    }
}

pub fn test_fchmodat2_is_enosys() -> TestResult {
    let file = "/tmp/pit-sc-fchmodat2";
    let _ = fs::remove_file(file);
    fs::write(file, b"x").map_err(|e| format!("write: {}", e))?;
    fs::set_permissions(file, fs::Permissions::from_mode(0o644))
        .map_err(|e| format!("chmod: {}", e))?;
    let path = c(file);
    let rc = unsafe {
        libc::syscall(
            SYS_FCHMODAT2,
            libc::AT_FDCWD,
            path.as_ptr(),
            0o600 as libc::c_uint,
            0 as libc::c_int,
        )
    };
    let err = errno();
    let mode = fs::metadata(file).map(|m| m.permissions().mode() & 0o7777);
    let _ = fs::remove_file(file);
    expect_enosys("fchmodat2", rc, err)?;
    match mode {
        Ok(0o644) => Ok(()),
        Ok(m) => Err(format!("mode is {:o} after fchmodat2, want 644", m)),
        Err(e) => Err(format!("stat after fchmodat2: {}", e)),
    }
}

pub fn test_setxattrat_is_enosys() -> TestResult {
    let file = "/tmp/pit-sc-setxattrat";
    let _ = fs::remove_file(file);
    fs::write(file, b"x").map_err(|e| format!("write: {}", e))?;
    let path = c(file);
    let name = c("user.pit");
    let value = b"v";
    let args = XattrArgs {
        value: value.as_ptr() as u64,
        size: value.len() as u32,
        flags: 0,
    };
    let rc = unsafe {
        libc::syscall(
            SYS_SETXATTRAT,
            libc::AT_FDCWD,
            path.as_ptr(),
            0 as libc::c_uint,
            name.as_ptr(),
            &args as *const XattrArgs,
            std::mem::size_of::<XattrArgs>(),
        )
    };
    let err = errno();
    let _ = fs::remove_file(file);
    expect_enosys("setxattrat", rc, err)
}

extern "C" {
    fn __system_property_get(name: *const libc::c_char, value: *mut libc::c_char) -> libc::c_int;
}

fn sdk_level() -> i32 {
    let name = b"ro.build.version.sdk\0";
    let mut value = [0 as libc::c_char; 92];
    let len =
        unsafe { __system_property_get(name.as_ptr() as *const libc::c_char, value.as_mut_ptr()) };
    if len <= 0 {
        return 0;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(value.as_ptr()) };
    text.to_string_lossy().trim().parse().unwrap_or(0)
}

pub fn test_path_free_syscall_above_table() -> TestResult {
    if sdk_level() < 35 {
        return Ok(());
    }
    let epfd = unsafe { libc::epoll_create1(0) };
    if epfd < 0 {
        return Err(format!("epoll_create1: errno {}", errno()));
    }
    let mut events: [libc::epoll_event; 1] = unsafe { std::mem::zeroed() };
    let timeout = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe {
        libc::syscall(
            SYS_EPOLL_PWAIT2,
            epfd,
            events.as_mut_ptr(),
            1 as libc::c_int,
            &timeout as *const libc::timespec,
            std::ptr::null::<libc::c_void>(),
            8 as libc::size_t,
        )
    };
    let err = errno();
    unsafe { libc::close(epfd) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("epoll_pwait2: rc={} errno={}, want 0", rc, err))
    }
}

pub fn test_renameat2_noreplace() -> TestResult {
    let a = "/tmp/pit-sc-rn-a";
    let b = "/tmp/pit-sc-rn-b";
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    fs::write(a, b"A").map_err(|e| format!("write a: {}", e))?;
    fs::write(b, b"B").map_err(|e| format!("write b: {}", e))?;
    let pa = c(a);
    let pb = c(b);
    let rc = unsafe {
        libc::syscall(
            SYS_RENAMEAT2,
            libc::AT_FDCWD,
            pa.as_ptr(),
            libc::AT_FDCWD,
            pb.as_ptr(),
            1 as libc::c_uint,
        )
    };
    let err = errno();
    let content = fs::read_to_string(b);
    let _ = fs::remove_file(a);
    let _ = fs::remove_file(b);
    if !(rc == -1 && (err == libc::EEXIST || err == libc::ENOSYS)) {
        return Err(format!(
            "renameat2 NOREPLACE: rc={} errno={}, want -1 with EEXIST or ENOSYS",
            rc, err
        ));
    }
    match content {
        Ok(ref s) if s == "B" => Ok(()),
        Ok(s) => Err(format!("b reads {:?} after renameat2, want \"B\"", s)),
        Err(e) => Err(format!("read b after renameat2: {}", e)),
    }
}

pub fn test_faccessat2_nofollow_dangling() -> TestResult {
    let link = "/tmp/pit-sc-dangling";
    let target = "/tmp/pit-sc-nowhere";
    let _ = fs::remove_file(link);
    let _ = fs::remove_file(target);
    std::os::unix::fs::symlink(target, link).map_err(|e| format!("symlink: {}", e))?;
    let path = c(link);
    let rc = unsafe {
        libc::syscall(
            SYS_FACCESSAT2,
            libc::AT_FDCWD,
            path.as_ptr(),
            libc::F_OK,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    let err = errno();
    let _ = fs::remove_file(link);
    if rc == 0 || (rc == -1 && err == libc::ENOSYS) {
        Ok(())
    } else {
        Err(format!(
            "faccessat2 NOFOLLOW on a dangling symlink: rc={} errno={}, want 0 or ENOSYS",
            rc, err
        ))
    }
}
