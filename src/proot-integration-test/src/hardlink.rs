use super::TestResult;
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::symlink;

const STORE: &str = "/.l2s";

pub fn probe() -> bool {
    true
}

fn c(path: &str) -> CString {
    CString::new(path).unwrap()
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn clean(basename: &str, dir: &str) {
    let prefix = format!(".l2s.{}", basename);
    if let Ok(entries) = fs::read_dir(STORE) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let _ = fs::remove_dir_all(dir);
}

fn read(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read {}: {}", path, e))
}

fn expect_content(path: &str, want: &str) -> TestResult {
    let got = read(path)?;
    if got == want {
        Ok(())
    } else {
        Err(format!("{} reads {:?}, want {:?}", path, got, want))
    }
}

fn setup(basename: &str, dir: &str) -> TestResult {
    clean(basename, dir);
    fs::create_dir_all(STORE).map_err(|e| format!("mkdir {}: {}", STORE, e))?;
    fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {}", dir, e))
}

fn full_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    for n in 1..=9999 {
        symlink(
            "/tmp/pit-hl-nowhere",
            format!("{}/.l2s.{}{:04}", STORE, basename, n),
        )
        .map_err(|e| format!("plant {}: {}", n, e))?;
    }
    let source = format!("{}/{}", dir, basename);
    let target = format!("{}/b", dir);
    fs::write(&source, b"x").map_err(|e| format!("write: {}", e))?;
    let rc = unsafe { libc::link(c(&source).as_ptr(), c(&target).as_ptr()) };
    let err = errno();
    if rc != -1 || err != libc::EMLINK {
        return Err(format!("link: rc={} errno={}, want EMLINK", rc, err));
    }
    let meta = fs::symlink_metadata(&source).map_err(|e| format!("lstat source: {}", e))?;
    if !meta.file_type().is_file() {
        return Err("the source is no longer a regular file".to_string());
    }
    expect_content(&source, "x")?;
    if fs::symlink_metadata(&target).is_ok() {
        return Err("the failed link left a name behind".to_string());
    }
    Ok(())
}

pub fn test_full_basename_is_emlink() -> TestResult {
    let basename = "pithlfull";
    let dir = "/tmp/pit-hl-full";
    let result = full_body(basename, dir);
    clean(basename, dir);
    result
}

fn stale_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let planted = format!("{}/.l2s.{}0001.0002", STORE, basename);
    fs::write(&planted, b"other").map_err(|e| format!("plant: {}", e))?;
    let source = format!("{}/{}", dir, basename);
    let target = format!("{}/b", dir);
    fs::write(&source, b"mine").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&source, &target).map_err(|e| format!("link: {}", e))?;
    expect_content(&source, "mine")?;
    expect_content(&target, "mine")?;
    expect_content(&planted, "other")
}

pub fn test_stale_data_name_is_never_overwritten() -> TestResult {
    let basename = "pithlstale";
    let dir = "/tmp/pit-hl-stale";
    let result = stale_body(basename, dir);
    clean(basename, dir);
    result
}

fn thousandth_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    for n in 1..1000 {
        let intermediate = format!("{}/.l2s.{}{:04}", STORE, basename, n);
        let data = format!("{}.0002", intermediate);
        fs::write(&data, format!("{:04}", n)).map_err(|e| format!("plant data {}: {}", n, e))?;
        symlink(&data, &intermediate).map_err(|e| format!("plant {}: {}", n, e))?;
    }
    let source = format!("{}/{}", dir, basename);
    let target = format!("{}/b", dir);
    fs::write(&source, b"new").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&source, &target).map_err(|e| format!("link: {}", e))?;
    expect_content(&source, "new")?;
    expect_content(&target, "new")?;
    expect_content(&format!("{}/.l2s.{}0999.0002", STORE, basename), "0999")
}

pub fn test_thousandth_group_gets_its_own_data() -> TestResult {
    let basename = "pithlk";
    let dir = "/tmp/pit-hl-k";
    let result = thousandth_body(basename, dir);
    clean(basename, dir);
    result
}
