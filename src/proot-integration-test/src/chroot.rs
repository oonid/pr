use super::TestResult;
use std::ffi::CString;
use std::fs;

pub fn probe() -> bool {
    true
}

fn in_child(dir: &str) -> Result<i32, String> {
    let dir = CString::new(dir).unwrap();
    let marker = CString::new("/pit-chroot-marker").unwrap();
    let root = CString::new("/").unwrap();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(format!("fork: {}", std::io::Error::last_os_error()));
    }
    if pid == 0 {
        unsafe {
            if libc::chroot(dir.as_ptr()) != 0 {
                libc::_exit(
                    std::io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(255),
                );
            }
            if libc::chdir(root.as_ptr()) != 0 || libc::access(marker.as_ptr(), libc::F_OK) != 0 {
                libc::_exit(200);
            }
            libc::_exit(0);
        }
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

pub fn test_chroot_into_directory() -> TestResult {
    let dir = "/tmp/pit-chroot";
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| format!("mkdir: {}", e))?;
    fs::write(format!("{}/pit-chroot-marker", dir), "").map_err(|e| format!("write: {}", e))?;
    let code = in_child(dir);
    let _ = fs::remove_dir_all(dir);
    match code? {
        0 => Ok(()),
        200 => Err("chroot succeeded but the new root is not the target".to_string()),
        errno => Err(format!("chroot failed: errno {}", errno)),
    }
}

pub fn test_chroot_missing_target() -> TestResult {
    let dir = "/tmp/pit-chroot-missing";
    let _ = fs::remove_dir_all(dir);
    match in_child(dir)? {
        code if code == libc::ENOENT => Ok(()),
        0 | 200 => Err("chroot into a missing directory succeeded".to_string()),
        errno => Err(format!("expected ENOENT, got errno {}", errno)),
    }
}
