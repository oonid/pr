use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub const INSTALL_MARKER_SUFFIX: &str = ".pr-cli-installing";
pub const BEING_INSTALLED: &str = "being installed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PendingInstall {
    InProgress,
    Interrupted,
}

pub fn marker_path(parent: &Path, name: &str) -> PathBuf {
    parent.join(format!(".{}{}", name, INSTALL_MARKER_SUFFIX))
}

pub struct InstallMarker {
    path: PathBuf,
    _file: File,
    took_over: bool,
}

impl InstallMarker {
    pub fn claim(parent: &Path, name: &str) -> Result<Self, String> {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {}", parent.display(), e))?;
        let path = marker_path(parent, name);
        let mut opened: Option<(File, bool)> = None;
        for _ in 0..CLAIM_ATTEMPTS {
            if opened.is_none() {
                opened = open_marker(&path)?;
            }
            if let Some((file, _)) = &opened {
                if lock(file, libc::LOCK_EX) {
                    if still_at(file, &path) {
                        let (file, took_over) = opened.take().expect("opened marker");
                        return Ok(Self {
                            path,
                            _file: file,
                            took_over,
                        });
                    }
                    opened = None;
                    continue;
                }
            }
            std::thread::sleep(CLAIM_RETRY_DELAY);
        }
        Err(BEING_INSTALLED.to_string())
    }

    pub fn claim_reporting(parent: &Path, name: &str) -> Result<Self, String> {
        Self::claim(parent, name).inspect_err(|e| {
            if e == BEING_INSTALLED {
                crate::shared::msg_error(&format!(
                    "another pr-cli process is changing '{}'; try again once it finishes.",
                    name
                ));
            }
        })
    }

    pub fn took_over(&self) -> bool {
        self.took_over
    }

    pub fn finish(self) -> Result<(), String> {
        fs::remove_file(&self.path).map_err(|e| format!("remove {}: {}", self.path.display(), e))
    }
}

const CLAIM_ATTEMPTS: u32 = 10;
const CLAIM_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

fn open_marker(path: &Path) -> Result<Option<(File, bool)>, String> {
    let open = |create_new: bool| {
        OpenOptions::new()
            .write(true)
            .create_new(create_new)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
    };
    match open(true) {
        Ok(file) => Ok(Some((file, false))),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => match open(false) {
            Ok(file) => Ok(Some((file, true))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("open {}: {}", path.display(), e)),
        },
        Err(e) => Err(format!("create {}: {}", path.display(), e)),
    }
}

pub fn has_marker(parent: &Path, name: &str) -> bool {
    fs::symlink_metadata(marker_path(parent, name)).is_ok()
}

pub fn pending_install(parent: &Path, name: &str) -> Option<PendingInstall> {
    let path = marker_path(parent, name);
    fs::symlink_metadata(&path).ok()?;
    let held = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => !lock(&file, libc::LOCK_SH),
        Err(e) if e.kind() == ErrorKind::NotFound => return None,
        Err(_) => false,
    };
    Some(if held {
        PendingInstall::InProgress
    } else {
        PendingInstall::Interrupted
    })
}

pub fn marked_names(parent: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let name = file_name.to_str()?;
            let alias = name
                .strip_prefix('.')?
                .strip_suffix(INSTALL_MARKER_SUFFIX)?;
            crate::shared::is_valid_container_name(alias).then(|| alias.to_string())
        })
        .collect();
    names.sort();
    names
}

fn lock(file: &File, operation: libc::c_int) -> bool {
    unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) == 0 }
}

fn still_at(file: &File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos))
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn claim_puts_only_the_marker_beside_the_container() {
        let parent = unique_temp_dir("pr-cli-marker-claim");

        let marker = InstallMarker::claim(&parent, "box").expect("claim");

        assert_eq!(entries(&parent), vec![".box.pr-cli-installing".to_string()]);
        assert!(!marker.took_over());
        drop(marker);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn finish_removes_the_marker() {
        let parent = unique_temp_dir("pr-cli-marker-finish");

        InstallMarker::claim(&parent, "box")
            .expect("claim")
            .finish()
            .expect("finish");

        assert!(entries(&parent).is_empty());
        assert_eq!(pending_install(&parent, "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_running_install_is_in_progress() {
        let parent = unique_temp_dir("pr-cli-marker-running");

        let marker = InstallMarker::claim(&parent, "box").expect("claim");

        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::InProgress)
        );
        assert_eq!(pending_install(&parent, "other"), None);
        drop(marker);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn an_install_that_ended_without_finishing_is_interrupted() {
        let parent = unique_temp_dir("pr-cli-marker-killed");

        drop(InstallMarker::claim(&parent, "box").expect("claim"));

        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::Interrupted)
        );
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_name_without_a_marker_has_no_pending_install() {
        let parent = unique_temp_dir("pr-cli-marker-none");
        fs::create_dir_all(parent.join("box")).expect("create container");

        assert_eq!(pending_install(&parent, "box"), None);
        assert_eq!(pending_install(&parent.join("missing"), "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_second_claim_is_refused_while_the_first_is_held() {
        let parent = unique_temp_dir("pr-cli-marker-twice");
        let first = InstallMarker::claim(&parent, "box").expect("claim");

        let second = InstallMarker::claim(&parent, "box");

        assert_eq!(second.err(), Some(BEING_INSTALLED.to_string()));
        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::InProgress)
        );
        drop(first);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_claim_takes_over_the_marker_of_a_killed_install() {
        let parent = unique_temp_dir("pr-cli-marker-take-over");
        drop(InstallMarker::claim(&parent, "box").expect("claim"));

        let marker = InstallMarker::claim(&parent, "box").expect("take over");

        assert!(marker.took_over());
        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::InProgress)
        );
        marker.finish().expect("finish");
        assert_eq!(pending_install(&parent, "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    fn hold_shared(path: &Path) -> File {
        let file = File::open(path).expect("open marker");
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        file
    }

    #[test]
    fn a_status_check_running_at_the_same_time_still_sees_an_interrupted_install() {
        let parent = unique_temp_dir("pr-cli-marker-probe");
        drop(InstallMarker::claim(&parent, "box").expect("claim"));
        let probe = hold_shared(&marker_path(&parent, "box"));

        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::Interrupted)
        );
        drop(probe);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_claim_waits_out_a_status_check() {
        let parent = unique_temp_dir("pr-cli-marker-claim-probe");
        drop(InstallMarker::claim(&parent, "box").expect("claim"));
        let probe = hold_shared(&marker_path(&parent, "box"));
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            drop(probe);
        });

        let marker = InstallMarker::claim(&parent, "box");

        release.join().expect("join");
        assert!(marker.expect("claimed after the check").took_over());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn marked_names_lists_every_name_with_a_marker() {
        let parent = unique_temp_dir("pr-cli-marker-names");
        drop(InstallMarker::claim(&parent, "box").expect("claim"));
        drop(InstallMarker::claim(&parent, "deb.old").expect("claim"));
        fs::create_dir_all(parent.join("plain")).expect("container");
        fs::write(parent.join(".notes"), b"x").expect("other dot file");
        fs::write(parent.join(".a b.pr-cli-installing"), b"").expect("invalid name");

        assert_eq!(
            marked_names(&parent),
            vec!["box".to_string(), "deb.old".to_string()]
        );
        assert!(marked_names(&parent.join("missing")).is_empty());
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_marker_that_is_a_symlink_is_not_followed() {
        let parent = unique_temp_dir("pr-cli-marker-symlink");
        let outside = unique_temp_dir("pr-cli-marker-symlink-target");
        fs::create_dir_all(&parent).expect("create parent");
        fs::write(&outside, b"x").expect("write outside");
        std::os::unix::fs::symlink(&outside, marker_path(&parent, "box")).expect("symlink");
        let held = File::open(&outside).expect("open outside");
        assert_eq!(
            unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );

        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::Interrupted)
        );
        assert!(InstallMarker::claim(&parent, "box").is_err());
        assert_eq!(fs::read(&outside).expect("outside intact"), b"x");
        drop(held);
        let _ = fs::remove_dir_all(parent);
        let _ = fs::remove_file(outside);
    }
}
