use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSourceInputKind {
    OciImageReference,
    DirectUrl,
    LocalArchive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSourceInput {
    OciImageReference { reference: String },
    DirectUrl { url: String },
    LocalArchive { path: PathBuf },
}

impl InstallSourceInput {
    pub fn classify(input: impl AsRef<str>) -> Option<Self> {
        let input = input.as_ref().trim();
        if input.is_empty() || input.chars().any(char::is_whitespace) {
            return None;
        }

        if looks_like_url(input) {
            return Some(Self::DirectUrl {
                url: input.to_string(),
            });
        }

        if looks_like_local_archive_path(input) && looks_like_local_archive_reference(input) {
            return Some(Self::LocalArchive {
                path: PathBuf::from(input),
            });
        }

        if looks_like_oci_image_reference(input) {
            return Some(Self::OciImageReference {
                reference: input.to_string(),
            });
        }

        None
    }

    pub fn kind(&self) -> InstallSourceInputKind {
        match self {
            InstallSourceInput::OciImageReference { .. } => {
                InstallSourceInputKind::OciImageReference
            }
            InstallSourceInput::DirectUrl { .. } => InstallSourceInputKind::DirectUrl,
            InstallSourceInput::LocalArchive { .. } => InstallSourceInputKind::LocalArchive,
        }
    }

    pub fn source_text(&self) -> Cow<'_, str> {
        match self {
            InstallSourceInput::OciImageReference { reference } => {
                Cow::Borrowed(reference.as_str())
            }
            InstallSourceInput::DirectUrl { url } => Cow::Borrowed(url.as_str()),
            InstallSourceInput::LocalArchive { path } => Cow::Owned(path.display().to_string()),
        }
    }

    pub fn source_path(&self) -> Option<&Path> {
        match self {
            InstallSourceInput::LocalArchive { path } => Some(path.as_path()),
            _ => None,
        }
    }
}

const LOCAL_ARCHIVE_SUFFIXES: &[&str] = &[
    ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2", ".tar.zst", ".tzst",
];

fn looks_like_url(input: &str) -> bool {
    input.contains("://")
}

fn looks_like_local_archive_path(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    LOCAL_ARCHIVE_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn looks_like_local_archive_reference(input: &str) -> bool {
    if input.starts_with('/')
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
    {
        return true;
    }

    match input.split_once('/') {
        None => true,
        Some((first, _rest)) => {
            if first.starts_with('.') {
                return true;
            }

            let slash_count = input.matches('/').count();
            if slash_count == 1
                && first.contains('.')
                && first.matches('.').count() == 1
                && !first.contains(':')
                && first != "localhost"
                && !is_known_registry_host(first)
            {
                return true;
            }

            !looks_like_registry_host(first)
        }
    }
}

fn looks_like_oci_image_reference(input: &str) -> bool {
    if input.starts_with('/')
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
        || input.contains("://")
    {
        return false;
    }

    let last_slash = input.rfind('/');
    let digest_at = input
        .rfind('@')
        .filter(|&at| last_slash.is_none_or(|slash| at > slash));
    let tag_colon = match digest_at {
        Some(at) => input[..at]
            .rfind(':')
            .filter(|&colon| last_slash.is_none_or(|slash| colon > slash)),
        None => input
            .rfind(':')
            .filter(|&colon| last_slash.is_none_or(|slash| colon > slash)),
    };

    let name_end = tag_colon.or(digest_at).unwrap_or(input.len());
    let has_tag = tag_colon.is_some();
    let has_digest = digest_at.is_some();

    let name = &input[..name_end];

    if name.is_empty() || name.starts_with('.') || name.ends_with('/') || name.contains("//") {
        return false;
    }

    let mut segments = name.split('/');
    let Some(first) = segments.next() else {
        return false;
    };

    if !is_valid_image_component(first, true) {
        return false;
    }

    if !segments.all(|segment| is_valid_image_component(segment, false)) {
        return false;
    }

    if has_digest {
        let Some(digest_at) = digest_at else {
            return false;
        };
        if !looks_like_oci_digest(&input[digest_at + 1..]) {
            return false;
        }
    }

    if has_tag {
        let Some(tag_colon) = tag_colon else {
            return false;
        };
        let tag_end = digest_at.unwrap_or(input.len());
        if !looks_like_oci_tag(&input[tag_colon + 1..tag_end]) {
            return false;
        }
    }

    has_tag || has_digest || input.contains('/') || input.contains('.')
}

fn looks_like_registry_host(component: &str) -> bool {
    component == "localhost"
        || component.contains(':')
        || is_known_registry_host(component)
        || component.contains('.')
}

fn is_known_registry_host(component: &str) -> bool {
    matches!(component, "docker.io" | "ghcr.io" | "quay.io" | "gcr.io")
}

fn looks_like_oci_digest(input: &str) -> bool {
    let Some((algorithm, hex)) = input.split_once(':') else {
        return false;
    };

    !algorithm.is_empty()
        && !hex.is_empty()
        && algorithm.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '-')
        })
        && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn looks_like_oci_tag(input: &str) -> bool {
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first.is_ascii_alphanumeric() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn is_valid_image_component(component: &str, allow_port: bool) -> bool {
    if component.is_empty() {
        return false;
    }

    if allow_port {
        if let Some((host, port)) = component.split_once(':') {
            return !host.is_empty()
                && !port.is_empty()
                && host.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-')
                })
                && port.chars().all(|c| c.is_ascii_digit());
        }
    }

    component
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_oci_image_references() {
        for input in [
            "docker.io/library/debian:stable",
            "docker.io/library/debian@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "docker.io/library/rootfs.tar",
            "docker.io/rootfs.tar",
            "registry.example.com/rootfs.tar",
            "example.io/team/rootfs.tar",
            "localhost:5000/rootfs.tar.xz",
            "debian:stable",
            "debian:Stable",
            "ghcr.io/acme/image:RC1",
            "ubuntu:24.04",
        ] {
            let parsed = InstallSourceInput::classify(input).expect("expected OCI reference");
            assert_eq!(parsed.kind(), InstallSourceInputKind::OciImageReference);
            assert_eq!(parsed.source_text(), input);
            assert!(parsed.source_path().is_none());
        }
    }

    #[test]
    fn classifies_direct_urls() {
        let parsed = InstallSourceInput::classify("https://example.com/rootfs.tar.xz")
            .expect("expected URL");
        assert_eq!(parsed.kind(), InstallSourceInputKind::DirectUrl);
        assert_eq!(parsed.source_text(), "https://example.com/rootfs.tar.xz");
        assert!(parsed.source_path().is_none());
    }

    #[test]
    fn classifies_local_archive_paths() {
        for input in [
            "/sdcard/Download/rootfs.tar.xz",
            "./rootfs.tar.gz",
            "rootfs.tar.xz",
            "subdir/rootfs.tar.xz",
            "my.dir/rootfs.tar.xz",
            ".cache/rootfs.tar.xz",
        ] {
            let parsed = InstallSourceInput::classify(input).expect("expected archive path");
            assert_eq!(parsed.kind(), InstallSourceInputKind::LocalArchive);
            assert_eq!(parsed.source_text(), input);
            assert_eq!(parsed.source_path(), Some(Path::new(input)));
        }
    }

    #[test]
    fn rejects_plain_names_to_remain_conservative() {
        assert!(InstallSourceInput::classify("debian").is_none());
    }
}
