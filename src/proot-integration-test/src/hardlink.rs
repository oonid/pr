use super::TestResult;
use std::ffi::CString;
use std::fs;
use std::os::unix::fs::symlink;

const STORE: &str = "/.l2s";
const SYS_RENAMEAT2: libc::c_long = 276;
const RENAME_NOREPLACE: libc::c_uint = 1;

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

fn existing_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let source = format!("{}/{}", dir, basename);
    let target = format!("{}/b", dir);
    fs::write(&source, b"x").map_err(|e| format!("write: {}", e))?;
    fs::write(&target, b"b").map_err(|e| format!("write: {}", e))?;
    let rc = unsafe { libc::link(c(&source).as_ptr(), c(&target).as_ptr()) };
    let err = errno();
    if rc != -1 || err != libc::EEXIST {
        return Err(format!("link: rc={} errno={}, want EEXIST", rc, err));
    }
    let meta = fs::symlink_metadata(&source).map_err(|e| format!("lstat source: {}", e))?;
    if !meta.file_type().is_file() {
        return Err("the source is no longer a regular file".to_string());
    }
    expect_content(&source, "x")?;
    expect_content(&target, "b")?;
    let prefix = format!(".l2s.{}", basename);
    let left: Vec<String> = fs::read_dir(STORE)
        .map_err(|e| format!("read {}: {}", STORE, e))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    if !left.is_empty() {
        return Err(format!("the failed link left {:?} in {}", left, STORE));
    }
    Ok(())
}

pub fn test_link_onto_existing_name_keeps_the_source() -> TestResult {
    let basename = "pithlexist";
    let dir = "/tmp/pit-hl-exist";
    let result = existing_body(basename, dir);
    clean(basename, dir);
    result
}

fn noreplace_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let a = format!("{}/{}", dir, basename);
    let b = format!("{}/b", dir);
    let other = format!("{}/c", dir);
    fs::write(&a, b"data").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&a, &b).map_err(|e| format!("link: {}", e))?;
    fs::write(&other, b"other").map_err(|e| format!("write: {}", e))?;
    let rc = unsafe {
        libc::syscall(
            SYS_RENAMEAT2,
            libc::AT_FDCWD,
            c(&other).as_ptr(),
            libc::AT_FDCWD,
            c(&b).as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    let err = errno();
    if rc != -1 || err != libc::EEXIST {
        return Err(format!("renameat2: rc={} errno={}, want EEXIST", rc, err));
    }
    fs::remove_file(&a).map_err(|e| format!("unlink: {}", e))?;
    expect_content(&b, "data")?;
    expect_content(&other, "other")
}

pub fn test_failed_noreplace_rename_keeps_the_link_count() -> TestResult {
    let basename = "pithlnorep";
    let dir = "/tmp/pit-hl-norep";
    let result = noreplace_body(basename, dir);
    clean(basename, dir);
    result
}

fn rename_over_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let a = format!("{}/{}", dir, basename);
    let b = format!("{}/b", dir);
    let other = format!("{}/c", dir);
    fs::write(&a, b"data").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&a, &b).map_err(|e| format!("link: {}", e))?;
    fs::write(&other, b"other").map_err(|e| format!("write: {}", e))?;
    fs::rename(&other, &b).map_err(|e| format!("rename: {}", e))?;
    expect_content(&a, "data")?;
    expect_content(&b, "other")?;
    let meta = fs::metadata(&a).map_err(|e| format!("stat {}: {}", a, e))?;
    let nlink = std::os::unix::fs::MetadataExt::nlink(&meta);
    if nlink != 1 {
        return Err(format!("st_nlink of {} is {}, want 1", a, nlink));
    }
    Ok(())
}

pub fn test_rename_over_a_hard_link_drops_one_name() -> TestResult {
    let basename = "pithlover";
    let dir = "/tmp/pit-hl-over";
    let result = rename_over_body(basename, dir);
    clean(basename, dir);
    result
}

fn last_name_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let a = format!("{}/{}", dir, basename);
    let b = format!("{}/b", dir);
    fs::write(&a, b"data").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&a, &b).map_err(|e| format!("link: {}", e))?;
    fs::remove_file(&a).map_err(|e| format!("unlink {}: {}", a, e))?;
    fs::remove_file(&b).map_err(|e| format!("unlink {}: {}", b, e))?;
    let prefix = format!(".l2s.{}", basename);
    let left: Vec<String> = fs::read_dir(STORE)
        .map_err(|e| format!("read {}: {}", STORE, e))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    if !left.is_empty() {
        return Err(format!("{:?} remain in {}", left, STORE));
    }
    Ok(())
}

pub fn test_unlink_of_the_last_name_removes_the_data() -> TestResult {
    let basename = "pithllast";
    let dir = "/tmp/pit-hl-last";
    let result = last_name_body(basename, dir);
    clean(basename, dir);
    result
}

fn onto_itself_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let a = format!("{}/{}", dir, basename);
    let b = format!("{}/b", dir);
    fs::write(&a, b"data").map_err(|e| format!("write: {}", e))?;
    fs::hard_link(&a, &b).map_err(|e| format!("link: {}", e))?;
    fs::rename(&b, &b).map_err(|e| format!("rename: {}", e))?;
    fs::remove_file(&a).map_err(|e| format!("unlink: {}", e))?;
    expect_content(&b, "data")
}

pub fn test_rename_onto_itself_keeps_the_link_count() -> TestResult {
    let basename = "pithlself";
    let dir = "/tmp/pit-hl-self";
    let result = onto_itself_body(basename, dir);
    clean(basename, dir);
    result
}

fn symlink_body(basename: &str, dir: &str) -> TestResult {
    setup(basename, dir)?;
    let a = format!("{}/{}", dir, basename);
    let b = format!("{}/b", dir);
    symlink("target-pit", &a).map_err(|e| format!("symlink: {}", e))?;
    let rc = unsafe { libc::link(c(&a).as_ptr(), c(&b).as_ptr()) };
    let err = errno();
    if rc != 0 {
        return Err(format!("link: rc={} errno={}, want 0", rc, err));
    }
    for path in [&b, &a] {
        let got = fs::read_link(path).map_err(|e| format!("readlink {}: {}", path, e))?;
        if got.to_string_lossy() != "target-pit" {
            return Err(format!("{} points to {:?}, want target-pit", path, got));
        }
    }
    let left: Vec<String> = fs::read_dir(STORE)
        .map_err(|e| format!("read {}: {}", STORE, e))?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("target-pit") || name.contains(basename))
        .collect();
    if !left.is_empty() {
        return Err(format!("the link left {:?} in {}", left, STORE));
    }
    Ok(())
}

pub fn test_link_of_a_symlink_copies_it() -> TestResult {
    let basename = "pithlsym";
    let dir = "/tmp/pit-hl-sym";
    let result = symlink_body(basename, dir);
    clean(basename, dir);
    result
}
