use sha2::Digest;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek};
use std::path::{Component, Path, PathBuf};

pub const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
pub const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
pub const DOCKER_MANIFEST_LIST_MEDIA_TYPE: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";
pub const DOCKER_MANIFEST_MEDIA_TYPE: &str = "application/vnd.docker.distribution.manifest.v2+json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedArchitecture {
    pub project_architecture: &'static str,
    pub oci_architecture: &'static str,
    pub variant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciPlatform {
    pub os: String,
    pub architecture: String,
    pub variant: Option<String>,
    pub os_version: Option<String>,
    pub os_features: Vec<String>,
}

impl OciPlatform {
    pub fn new(os: impl Into<String>, architecture: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            architecture: architecture.into(),
            variant: None,
            os_version: None,
            os_features: Vec::new(),
        }
    }

    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = Some(variant.into());
        self
    }

    pub fn normalized_architecture(&self) -> Option<NormalizedArchitecture> {
        let mut normalized = normalize_architecture(&self.architecture)?;
        if normalized.variant.is_none() {
            normalized.variant = self
                .variant
                .as_ref()
                .map(|variant| normalize_variant(variant));
        }
        Some(normalized)
    }

    fn score_for_target(&self, target: &NormalizedArchitecture) -> Option<u8> {
        if !self.os.is_empty() && !self.os.eq_ignore_ascii_case("linux") {
            return None;
        }

        let platform_arch = self.normalized_architecture()?;
        if platform_arch.project_architecture != target.project_architecture {
            return None;
        }

        match (&target.variant, &platform_arch.variant) {
            (Some(target_variant), Some(platform_variant))
                if target_variant.eq_ignore_ascii_case(platform_variant) =>
            {
                Some(3)
            }
            (None, None) => Some(3),
            (Some(_), None) | (None, Some(_)) => Some(2),
            (Some(_), Some(_)) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciDescriptor {
    pub media_type: Option<String>,
    pub digest: String,
    pub size: Option<u64>,
    pub urls: Vec<String>,
    pub annotations: BTreeMap<String, String>,
    pub platform: Option<OciPlatform>,
}

impl OciDescriptor {
    pub fn new(digest: impl Into<String>) -> Self {
        Self {
            media_type: None,
            digest: digest.into(),
            size: None,
            urls: Vec::new(),
            annotations: BTreeMap::new(),
            platform: None,
        }
    }

    pub fn with_platform(mut self, platform: OciPlatform) -> Self {
        self.platform = Some(platform);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciManifest {
    pub schema_version: u32,
    pub media_type: Option<String>,
    pub config: OciDescriptor,
    pub layers: Vec<OciDescriptor>,
    pub annotations: BTreeMap<String, String>,
}

impl OciManifest {
    pub fn new(config: OciDescriptor, layers: Vec<OciDescriptor>) -> Self {
        Self {
            schema_version: 2,
            media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
            config,
            layers,
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OciImageIndex {
    pub schema_version: u32,
    pub media_type: Option<String>,
    pub manifests: Vec<OciDescriptor>,
    pub annotations: BTreeMap<String, String>,
}

impl OciImageIndex {
    pub fn new(manifests: Vec<OciDescriptor>) -> Self {
        Self {
            schema_version: 2,
            media_type: Some(OCI_IMAGE_INDEX_MEDIA_TYPE.to_string()),
            manifests,
            annotations: BTreeMap::new(),
        }
    }

    pub fn select_manifest_descriptor(&self, architecture: &str) -> Option<&OciDescriptor> {
        select_manifest_descriptor(&self.manifests, architecture)
    }
}

pub fn normalize_architecture(input: &str) -> Option<NormalizedArchitecture> {
    let input = input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return None;
    }

    if let Some((project_architecture, variant)) = architecture_with_variant(&input) {
        return Some(NormalizedArchitecture {
            project_architecture,
            oci_architecture: project_to_oci_architecture(project_architecture),
            variant: Some(variant),
        });
    }

    let project_architecture = project_architecture_name(&input)?;
    Some(NormalizedArchitecture {
        project_architecture,
        oci_architecture: project_to_oci_architecture(project_architecture),
        variant: None,
    })
}

pub fn project_architecture_name(input: &str) -> Option<&'static str> {
    match input {
        "aarch64" | "arm64" | "arm64v8" | "arm64-v8" | "arm64-v8a" => Some("aarch64"),
        "amd64" | "x86_64" => Some("x86_64"),
        "386" | "i386" | "i686" | "x86" => Some("i686"),
        "arm" | "armhf" | "armel" => Some("arm"),
        "riscv64" => Some("riscv64"),
        "mips" => Some("mips"),
        "mips64" => Some("mips64"),
        "ppc64le" => Some("ppc64le"),
        _ => None,
    }
}

pub fn oci_architecture_name(input: &str) -> Option<&'static str> {
    normalize_architecture(input).map(|normalized| normalized.oci_architecture)
}

pub fn select_manifest_descriptor<'a>(
    manifests: &'a [OciDescriptor],
    architecture: &str,
) -> Option<&'a OciDescriptor> {
    let target = normalize_architecture(architecture)?;
    let mut best: Option<(u8, &OciDescriptor)> = None;
    let mut fallback_single: Option<&OciDescriptor> = None;
    let mut manifest_count = 0usize;

    for descriptor in manifests {
        if !is_manifest_media_type(descriptor.media_type.as_deref()) {
            continue;
        }
        manifest_count += 1;
        if manifest_count == 1 {
            fallback_single = Some(descriptor);
        }

        let Some(platform) = descriptor.platform.as_ref() else {
            continue;
        };
        let Some(score) = platform.score_for_target(&target) else {
            continue;
        };

        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score > *best_score)
        {
            best = Some((score, descriptor));
        }
    }

    best.map(|(_, descriptor)| descriptor).or_else(|| {
        (manifest_count == 1)
            .then_some(fallback_single)
            .flatten()
            .filter(|descriptor| descriptor.platform.is_none())
    })
}

fn project_to_oci_architecture(project_architecture: &'static str) -> &'static str {
    match project_architecture {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        "i686" => "386",
        other => other,
    }
}

fn architecture_with_variant(input: &str) -> Option<(&'static str, String)> {
    match input {
        "arm64-v8a" => Some(("aarch64", "v8a".to_string())),
        "arm64-v8" | "arm64v8" => Some(("aarch64", "v8".to_string())),
        "armv8" => Some(("arm", "v8".to_string())),
        "armv7" => Some(("arm", "v7".to_string())),
        "armv6" => Some(("arm", "v6".to_string())),
        _ => input.split_once('/').and_then(|(architecture, variant)| {
            let architecture = project_architecture_name(architecture)?;
            Some((architecture, normalize_variant(variant)))
        }),
    }
}

fn normalize_variant(input: &str) -> String {
    input.trim().to_ascii_lowercase()
}

fn is_manifest_media_type(media_type: Option<&str>) -> bool {
    match media_type {
        None => true,
        Some(media_type) => matches!(
            media_type.trim().to_ascii_lowercase().as_str(),
            OCI_IMAGE_MANIFEST_MEDIA_TYPE | DOCKER_MANIFEST_MEDIA_TYPE
        ),
    }
}

pub fn blob_path(cache_dir: &Path, digest: &str) -> Result<PathBuf, String> {
    let file_name = digest
        .replace(':', "_")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    if file_name.is_empty() {
        return Err("invalid empty digest".to_string());
    }
    Ok(cache_dir.join(file_name))
}

pub fn blob_url(registry_base: &str, repository: &str, digest: &str) -> String {
    let base = registry_base.trim_end_matches('/');
    let repo = repository.trim_matches('/');
    format!("{}/v2/{}/blobs/{}", base, repo, digest)
}

pub async fn download_blob(
    url: &str,
    destination: &Path,
    expected_digest: &str,
) -> Result<(), String> {
    download_blob_with_bearer(url, destination, expected_digest, None).await
}

pub fn partial_blob_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    destination.with_file_name(name)
}

pub fn blob_matches_digest(path: &Path, expected_digest: &str) -> Result<bool, String> {
    let expected_sha256 = normalize_expected_sha256(expected_digest)?;
    let mut file =
        fs::File::open(path).map_err(|e| format!("open blob {}: {}", path.display(), e))?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| format!("read blob {}: {}", path.display(), e))?;
    Ok(hex_lower(&hasher.finalize()) == expected_sha256)
}

pub async fn download_blob_with_bearer(
    url: &str,
    destination: &Path,
    expected_digest: &str,
    bearer_token: Option<&str>,
) -> Result<(), String> {
    let expected_sha256 = normalize_expected_sha256(expected_digest)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create blob dir {}: {}", parent.display(), e))?;
    }

    let partial = partial_blob_path(destination);
    let downloaded = download_to_partial(url, &partial, &expected_sha256, bearer_token)
        .await
        .and_then(|()| {
            fs::rename(&partial, destination).map_err(|e| {
                format!(
                    "rename blob {} to {}: {}",
                    partial.display(),
                    destination.display(),
                    e
                )
            })
        });
    if downloaded.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    downloaded
}

async fn download_to_partial(
    url: &str,
    partial: &Path,
    expected_sha256: &str,
    bearer_token: Option<&str>,
) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let client = reqwest::Client::builder()
        .user_agent("pr-cli-oci/0.1")
        .build()
        .map_err(|e| format!("create reqwest client: {}", e))?;
    let mut request = client.get(url);
    if let Some(token) = bearer_token {
        request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {}", token));
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("download blob {}: {}", url, e))?;
    if !response.status().is_success() {
        return Err(format!(
            "download blob {} failed with HTTP {}",
            url,
            response.status()
        ));
    }

    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(|e| format!("create blob file {}: {}", partial.display(), e))?;
    let mut hasher = sha2::Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(next) = stream.next().await {
        let chunk = next.map_err(|e| format!("read blob stream {}: {}", url, e))?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write blob {}: {}", partial.display(), e))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("flush blob {}: {}", partial.display(), e))?;
    file.sync_all()
        .await
        .map_err(|e| format!("sync blob {}: {}", partial.display(), e))?;
    let actual_sha256 = hex_lower(&hasher.finalize());
    if actual_sha256 != expected_sha256 {
        return Err(format!(
            "digest mismatch for {}: expected sha256:{}, got sha256:{}",
            url, expected_sha256, actual_sha256
        ));
    }
    Ok(())
}

pub fn apply_layer_blob(blob_path: &Path, rootfs: &Path) -> Result<(), String> {
    let blob = fs::File::open(blob_path)
        .map_err(|e| format!("open layer blob {}: {}", blob_path.display(), e))?;
    let mut probe = [0u8; 6];
    let mut reader = std::io::BufReader::new(blob);
    let read_len = reader
        .read(&mut probe)
        .map_err(|e| format!("read layer blob header {}: {}", blob_path.display(), e))?;
    reader
        .rewind()
        .map_err(|e| format!("rewind blob reader {}: {}", blob_path.display(), e))?;

    let boxed_reader: Box<dyn Read> = if read_len >= 2 && probe[0..2] == [0x1f, 0x8b] {
        Box::new(flate2::read::GzDecoder::new(reader))
    } else if read_len >= 6 && probe[0..6] == [0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00] {
        Box::new(xz2::read::XzDecoder::new(reader))
    } else {
        Box::new(reader)
    };

    apply_layer_tar_stream(boxed_reader, rootfs)
}

fn apply_layer_tar_stream(reader: Box<dyn Read>, rootfs: &Path) -> Result<(), String> {
    use std::collections::HashSet;

    fs::create_dir_all(rootfs).map_err(|e| format!("create rootfs {}: {}", rootfs.display(), e))?;
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    archive.set_preserve_mtime(true);
    let mut created_paths: HashSet<PathBuf> = HashSet::new();
    let mut pending_hardlinks: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in archive
        .entries()
        .map_err(|e| format!("read layer entries: {}", e))?
    {
        let mut entry = entry.map_err(|e| format!("read layer entry: {}", e))?;
        let entry_path = entry
            .path()
            .map_err(|e| format!("read layer entry path: {}", e))?;
        let relative_path = sanitize_layer_path(&entry_path)?;
        if relative_path.as_os_str().is_empty() {
            continue;
        }

        if apply_whiteout_if_needed(rootfs, &relative_path, &created_paths)? {
            continue;
        }

        if entry.header().entry_type().is_hard_link() {
            let link_name = entry
                .link_name()
                .map_err(|e| format!("read hardlink target {}: {}", relative_path.display(), e))?
                .ok_or_else(|| {
                    format!("hardlink {} missing link target", relative_path.display())
                })?;
            let link_target = sanitize_layer_path(&link_name)?;
            if link_target.as_os_str().is_empty() {
                return Err(format!(
                    "hardlink {} has empty link target",
                    relative_path.display()
                ));
            }
            pending_hardlinks.push((relative_path, link_target));
            continue;
        }

        let within_root = entry
            .unpack_in(rootfs)
            .map_err(|e| format!("extract {}: {}", relative_path.display(), e))?;
        if !within_root {
            return Err(format!(
                "extract blocked outside rootfs: {}",
                relative_path.display()
            ));
        }

        created_paths.insert(rootfs.join(&relative_path));
    }

    while !pending_hardlinks.is_empty() {
        let mut remaining: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut progressed = false;

        for (relative_path, link_target) in pending_hardlinks {
            let source = rootfs.join(&link_target);
            if !source.exists() {
                remaining.push((relative_path, link_target));
                continue;
            }

            let destination = rootfs.join(&relative_path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("create hardlink parent {}: {}", parent.display(), e))?;
            }
            remove_path_if_exists(&destination)?;
            if let Err(err) = fs::hard_link(&source, &destination) {
                let fallback_allowed = matches!(
                    err.kind(),
                    std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::CrossesDevices
                        | std::io::ErrorKind::Unsupported
                );
                if !fallback_allowed {
                    return Err(format!(
                        "extract {}: create hardlink to {}: {}",
                        relative_path.display(),
                        link_target.display(),
                        err
                    ));
                }

                fs::copy(&source, &destination).map_err(|copy_err| {
                    format!(
                        "extract {}: copy fallback for hardlink {} failed: {}",
                        relative_path.display(),
                        link_target.display(),
                        copy_err
                    )
                })?;
                if let Ok(metadata) = fs::metadata(&source) {
                    let _ = fs::set_permissions(&destination, metadata.permissions());
                }
            }
            created_paths.insert(destination);
            progressed = true;
        }

        if !progressed {
            let (relative_path, link_target) = &remaining[0];
            return Err(format!(
                "extract {}: hardlink target missing {}",
                relative_path.display(),
                link_target.display()
            ));
        }
        pending_hardlinks = remaining;
    }

    Ok(())
}

fn sanitize_layer_path(path: &Path) -> Result<PathBuf, String> {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => return Err(format!("unsafe layer path {}", path.display())),
            Component::Normal(part) => cleaned.push(part),
        }
    }
    Ok(cleaned)
}

fn apply_whiteout_if_needed(
    rootfs: &Path,
    relative_path: &Path,
    protected_paths: &std::collections::HashSet<PathBuf>,
) -> Result<bool, String> {
    let Some(file_name) = relative_path.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    if !file_name.starts_with(".wh.") {
        return Ok(false);
    }

    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    let parent_dir = rootfs.join(parent);
    if has_symlink_ancestor(rootfs, parent)? {
        return Ok(true);
    }
    if file_name == ".wh..wh..opq" {
        remove_directory_children(&parent_dir, protected_paths)?;
        return Ok(true);
    }

    let target_name = file_name.trim_start_matches(".wh.");
    if target_name.is_empty() {
        return Ok(true);
    }

    fn has_symlink_ancestor(rootfs: &Path, relative_path: &Path) -> Result<bool, String> {
        let mut current = rootfs.to_path_buf();
        for component in relative_path.components() {
            if let Component::Normal(part) = component {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                    Err(e) => return Err(format!("stat {}: {}", current.display(), e)),
                }
            }
        }
        Ok(false)
    }
    let target_path = rootfs.join(parent).join(target_name);
    remove_path_if_exists_with_protection(&target_path, protected_paths)?;
    Ok(true)
}

fn remove_directory_children(
    dir: &Path,
    protected_paths: &std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("stat dir {}: {}", dir.display(), e)),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("read dir {}: {}", dir.display(), e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir entry {}: {}", dir.display(), e))?;
        let entry_path = entry.path();
        remove_path_if_exists_with_protection(&entry_path, protected_paths)?;
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("stat {}: {}", path.display(), e)),
    };
    if metadata.is_dir() {
        fs::remove_dir_all(path).map_err(|e| format!("remove dir {}: {}", path.display(), e))?;
    } else {
        fs::remove_file(path).map_err(|e| format!("remove file {}: {}", path.display(), e))?;
    }
    Ok(())
}

fn remove_path_if_exists_with_protection(
    path: &Path,
    protected_paths: &std::collections::HashSet<PathBuf>,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("stat {}: {}", path.display(), e)),
    };
    if protected_paths.iter().any(|protected| protected == path) {
        return Ok(());
    }

    let has_protected_descendants = protected_paths
        .iter()
        .any(|protected| protected.starts_with(path));
    if !has_protected_descendants {
        return remove_path_if_exists(path);
    }

    if !metadata.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(path).map_err(|e| format!("read dir {}: {}", path.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read dir entry {}: {}", path.display(), e))?;
        remove_path_if_exists_with_protection(&entry.path(), protected_paths)?;
    }
    Ok(())
}

fn normalize_expected_sha256(digest: &str) -> Result<String, String> {
    let Some((algorithm, hex)) = digest.split_once(':') else {
        return Err(format!("invalid digest format: {}", digest));
    };
    if !algorithm.eq_ignore_ascii_case("sha256") {
        return Err(format!("unsupported digest algorithm: {}", algorithm));
    }
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("invalid sha256 digest: {}", digest));
    }
    Ok(hex.to_ascii_lowercase())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pr-cli-{}-{}", label, nanos));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn write_layer_tar(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
        let file = fs::File::create(path).expect("create tar");
        let mut builder = tar::Builder::new(file);

        for (entry_path, content) in entries {
            let mut header = tar::Header::new_gnu();
            if let Some(content) = content {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o644);
                header.set_size(content.len() as u64);
                header.set_cksum();
                builder
                    .append_data(&mut header, *entry_path, *content)
                    .expect("append file");
            } else {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o000);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, *entry_path, std::io::empty())
                    .expect("append whiteout");
            }
        }
        builder.finish().expect("finish tar");
    }

    fn write_layer_tar_with_hardlink_before_target(path: &Path) {
        let file = fs::File::create(path).expect("create tar");
        let mut builder = tar::Builder::new(file);

        let mut hardlink = tar::Header::new_gnu();
        hardlink.set_entry_type(tar::EntryType::Link);
        hardlink.set_mode(0o755);
        hardlink.set_size(0);
        hardlink.set_cksum();
        builder
            .append_link(&mut hardlink, "usr/bin/perl5.38.2", "usr/bin/perl")
            .expect("append hardlink");

        let mut target = tar::Header::new_gnu();
        let content = b"#!/bin/sh\nexit 0\n";
        target.set_entry_type(tar::EntryType::Regular);
        target.set_mode(0o755);
        target.set_size(content.len() as u64);
        target.set_cksum();
        builder
            .append_data(&mut target, "usr/bin/perl", &content[..])
            .expect("append target file");

        builder.finish().expect("finish tar");
    }

    fn write_gzip_layer_tar(path: &Path, entries: &[(&str, Option<&[u8]>)]) {
        let file = fs::File::create(path).expect("create gz");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);

        for (entry_path, content) in entries {
            let mut header = tar::Header::new_gnu();
            if let Some(content) = content {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o644);
                header.set_size(content.len() as u64);
                header.set_cksum();
                builder
                    .append_data(&mut header, *entry_path, *content)
                    .expect("append file");
            } else {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o000);
                header.set_size(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, *entry_path, std::io::empty())
                    .expect("append whiteout");
            }
        }
        let encoder = builder.into_inner().expect("finish tar");
        let _ = encoder.finish().expect("finish gzip");
    }

    #[test]
    fn normalizes_common_architecture_aliases() {
        let arm64 = normalize_architecture("arm64").expect("expected normalized arch");
        assert_eq!(arm64.project_architecture, "aarch64");
        assert_eq!(arm64.oci_architecture, "arm64");

        let amd64 = normalize_architecture("amd64").expect("expected normalized arch");
        assert_eq!(amd64.project_architecture, "x86_64");
        assert_eq!(amd64.oci_architecture, "amd64");

        let armv7 = normalize_architecture("arm/v7").expect("expected normalized arch");
        assert_eq!(armv7.project_architecture, "arm");
        assert_eq!(armv7.oci_architecture, "arm");
        assert_eq!(armv7.variant.as_deref(), Some("v7"));
    }

    #[test]
    fn selects_matching_manifest_for_arm64() {
        let index = OciImageIndex::new(vec![
            OciDescriptor::new("sha256:aaa").with_platform(OciPlatform::new("linux", "amd64")),
            OciDescriptor::new("sha256:bbb").with_platform(OciPlatform::new("linux", "arm64")),
        ]);

        let selected = index
            .select_manifest_descriptor("aarch64")
            .expect("expected matching descriptor");
        assert_eq!(selected.digest, "sha256:bbb");
    }

    #[test]
    fn prefers_exact_variant_match_when_available() {
        let manifests = vec![
            OciDescriptor::new("sha256:aaa")
                .with_platform(OciPlatform::new("linux", "arm").with_variant("v6")),
            OciDescriptor::new("sha256:bbb")
                .with_platform(OciPlatform::new("linux", "arm").with_variant("v7")),
        ];

        let selected =
            select_manifest_descriptor(&manifests, "arm/v7").expect("expected matching descriptor");
        assert_eq!(selected.digest, "sha256:bbb");
    }

    #[test]
    fn rejects_explicit_variant_mismatch() {
        let manifests = vec![OciDescriptor::new("sha256:aaa")
            .with_platform(OciPlatform::new("linux", "arm").with_variant("v6"))];

        let selected = select_manifest_descriptor(&manifests, "arm/v7");
        assert!(selected.is_none());
    }

    #[test]
    fn ignores_non_linux_platforms() {
        let manifests = vec![
            OciDescriptor::new("sha256:aaa").with_platform(OciPlatform::new("windows", "amd64")),
            OciDescriptor::new("sha256:bbb").with_platform(OciPlatform::new("linux", "amd64")),
        ];

        let selected =
            select_manifest_descriptor(&manifests, "x86_64").expect("expected linux descriptor");
        assert_eq!(selected.digest, "sha256:bbb");
    }

    #[test]
    fn exposes_oci_architecture_name_for_outbound_requests() {
        assert_eq!(oci_architecture_name("aarch64"), Some("arm64"));
        assert_eq!(oci_architecture_name("x86_64"), Some("amd64"));
    }

    #[test]
    fn supports_docker_manifest_media_type() {
        let manifests = vec![
            OciDescriptor::new("sha256:aaa").with_platform(OciPlatform::new("linux", "amd64")),
            OciDescriptor {
                media_type: Some(DOCKER_MANIFEST_MEDIA_TYPE.to_string()),
                digest: "sha256:bbb".to_string(),
                size: None,
                urls: Vec::new(),
                annotations: BTreeMap::new(),
                platform: Some(OciPlatform::new("linux", "arm64")),
            },
        ];

        let selected = select_manifest_descriptor(&manifests, "aarch64")
            .expect("expected docker manifest descriptor");
        assert_eq!(selected.digest, "sha256:bbb");
    }

    #[test]
    fn falls_back_to_first_manifest_when_platform_is_missing() {
        let manifests = vec![
            OciDescriptor {
                media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
                digest: "sha256:aaa".to_string(),
                size: None,
                urls: Vec::new(),
                annotations: BTreeMap::new(),
                platform: None,
            },
            OciDescriptor {
                media_type: Some(OCI_IMAGE_INDEX_MEDIA_TYPE.to_string()),
                digest: "sha256:index".to_string(),
                size: None,
                urls: Vec::new(),
                annotations: BTreeMap::new(),
                platform: None,
            },
        ];

        let selected = select_manifest_descriptor(&manifests, "aarch64")
            .expect("expected fallback manifest descriptor");
        assert_eq!(selected.digest, "sha256:aaa");
    }

    #[test]
    fn does_not_guess_when_multiple_manifests_lack_platform_data() {
        let manifests = vec![
            OciDescriptor {
                media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
                digest: "sha256:aaa".to_string(),
                size: None,
                urls: Vec::new(),
                annotations: BTreeMap::new(),
                platform: None,
            },
            OciDescriptor {
                media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
                digest: "sha256:bbb".to_string(),
                size: None,
                urls: Vec::new(),
                annotations: BTreeMap::new(),
                platform: None,
            },
        ];

        let selected = select_manifest_descriptor(&manifests, "aarch64");
        assert!(selected.is_none());
    }

    #[test]
    fn does_not_fallback_to_single_explicit_mismatch_platform() {
        let manifests = vec![OciDescriptor {
            media_type: Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
            digest: "sha256:aaa".to_string(),
            size: None,
            urls: Vec::new(),
            annotations: BTreeMap::new(),
            platform: Some(OciPlatform::new("linux", "amd64")),
        }];

        let selected = select_manifest_descriptor(&manifests, "aarch64");
        assert!(selected.is_none());
    }

    #[test]
    fn applies_whiteout_for_regular_file() {
        let tmp = unique_tmp_dir("oci-whiteout-file");
        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("mkdir rootfs");
        fs::write(rootfs.join("etc/obsolete.conf"), b"old").expect("seed old file");

        let layer = tmp.join("layer.tar");
        write_layer_tar(
            &layer,
            &[
                ("etc/.wh.obsolete.conf", None),
                ("etc/new.conf", Some(b"new")),
            ],
        );
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert!(!rootfs.join("etc/obsolete.conf").exists());
        assert_eq!(
            fs::read(rootfs.join("etc/new.conf")).expect("read new file"),
            b"new"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn applies_layer_with_hardlink_declared_before_target() {
        let tmp = unique_tmp_dir("oci-hardlink-before-target");
        let layer = tmp.join("layer.tar");
        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(&rootfs).expect("create rootfs");
        write_layer_tar_with_hardlink_before_target(&layer);

        apply_layer_blob(&layer, &rootfs).expect("apply layer blob");

        let target = fs::read(rootfs.join("usr/bin/perl")).expect("read target");
        let hardlink = fs::read(rootfs.join("usr/bin/perl5.38.2")).expect("read hardlink");
        assert_eq!(target, hardlink);

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn late_regular_whiteout_does_not_remove_same_layer_files() {
        let tmp = unique_tmp_dir("oci-whiteout-late");
        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("mkdir rootfs");
        fs::write(rootfs.join("etc/base.conf"), b"base").expect("seed base");

        let layer = tmp.join("layer.tar");
        write_layer_tar(
            &layer,
            &[("etc/new.conf", Some(b"new")), ("etc/.wh.new.conf", None)],
        );
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert_eq!(
            fs::read(rootfs.join("etc/new.conf")).expect("read new file"),
            b"new"
        );
        assert_eq!(
            fs::read(rootfs.join("etc/base.conf")).expect("read base file"),
            b"base"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn applies_opaque_whiteout_for_directory() {
        let tmp = unique_tmp_dir("oci-whiteout-opq");
        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(rootfs.join("var/cache")).expect("mkdir cache");
        fs::write(rootfs.join("var/cache/a"), b"a").expect("seed a");
        fs::write(rootfs.join("var/cache/b"), b"b").expect("seed b");

        let layer = tmp.join("layer.tar.gz");
        write_gzip_layer_tar(
            &layer,
            &[
                ("var/cache/.wh..wh..opq", None),
                ("var/cache/new", Some(b"new")),
            ],
        );
        apply_layer_blob(&layer, &rootfs).expect("apply gzip layer");

        assert!(!rootfs.join("var/cache/a").exists());
        assert!(!rootfs.join("var/cache/b").exists());
        assert_eq!(
            fs::read(rootfs.join("var/cache/new")).expect("read new"),
            b"new"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn opaque_whiteout_removes_stale_children_in_mixed_subdir() {
        let tmp = unique_tmp_dir("oci-whiteout-mixed-subdir");
        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(rootfs.join("var/cache/sub")).expect("mkdir subdir");
        fs::write(rootfs.join("var/cache/sub/old"), b"old").expect("seed old");

        let layer = tmp.join("layer.tar");
        write_layer_tar(
            &layer,
            &[
                ("var/cache/sub/new", Some(b"new")),
                ("var/cache/.wh..wh..opq", None),
            ],
        );
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert!(!rootfs.join("var/cache/sub/old").exists());
        assert_eq!(
            fs::read(rootfs.join("var/cache/sub/new")).expect("read new"),
            b"new"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn opaque_whiteout_does_not_follow_symlink_parent() {
        let tmp = unique_tmp_dir("oci-whiteout-symlink-parent");
        let outside = tmp.join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(outside.join("victim"), b"victim").expect("seed victim");

        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(&rootfs).expect("mkdir rootfs");
        symlink(&outside, rootfs.join("etc")).expect("symlink etc");

        let layer = tmp.join("layer.tar");
        write_layer_tar(&layer, &[("etc/.wh..wh..opq", None)]);
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert_eq!(
            fs::read(outside.join("victim")).expect("read victim"),
            b"victim"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn regular_whiteout_does_not_follow_symlink_parent() {
        let tmp = unique_tmp_dir("oci-whiteout-symlink-regular");
        let outside = tmp.join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(outside.join("victim"), b"victim").expect("seed victim");

        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(&rootfs).expect("mkdir rootfs");
        symlink(&outside, rootfs.join("etc")).expect("symlink etc");

        let layer = tmp.join("layer.tar");
        write_layer_tar(&layer, &[("etc/.wh.victim", None)]);
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert_eq!(
            fs::read(outside.join("victim")).expect("read victim"),
            b"victim"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn regular_whiteout_does_not_follow_nested_symlink_ancestor() {
        let tmp = unique_tmp_dir("oci-whiteout-symlink-nested-regular");
        let outside = tmp.join("outside");
        fs::create_dir_all(outside.join("b")).expect("mkdir outside/b");
        fs::write(outside.join("b/victim"), b"victim").expect("seed victim");

        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(&rootfs).expect("mkdir rootfs");
        symlink(&outside, rootfs.join("a")).expect("symlink a");

        let layer = tmp.join("layer.tar");
        write_layer_tar(&layer, &[("a/b/.wh.victim", None)]);
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert_eq!(
            fs::read(outside.join("b/victim")).expect("read victim"),
            b"victim"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn opaque_whiteout_does_not_follow_nested_symlink_ancestor() {
        let tmp = unique_tmp_dir("oci-whiteout-symlink-nested-opaque");
        let outside = tmp.join("outside");
        fs::create_dir_all(outside.join("b")).expect("mkdir outside/b");
        fs::write(outside.join("b/victim"), b"victim").expect("seed victim");

        let rootfs = tmp.join("rootfs");
        fs::create_dir_all(&rootfs).expect("mkdir rootfs");
        symlink(&outside, rootfs.join("a")).expect("symlink a");

        let layer = tmp.join("layer.tar");
        write_layer_tar(&layer, &[("a/b/.wh..wh..opq", None)]);
        apply_layer_blob(&layer, &rootfs).expect("apply layer");

        assert_eq!(
            fs::read(outside.join("b/victim")).expect("read victim"),
            b"victim"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn builds_blob_url_and_cache_path() {
        let cache = Path::new("/tmp/blobs");
        let path = blob_path(cache, "sha256:abc123").expect("blob path");
        assert_eq!(path, Path::new("/tmp/blobs/sha256_abc123"));
        assert_eq!(
            blob_url(
                "https://registry-1.docker.io/",
                "library/debian",
                "sha256:deadbeef"
            ),
            "https://registry-1.docker.io/v2/library/debian/blobs/sha256:deadbeef"
        );
    }

    #[test]
    fn accepts_valid_sha256_digest() {
        let normalized = normalize_expected_sha256(
            "sha256:ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789",
        )
        .expect("normalize digest");
        assert_eq!(
            normalized,
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn rejects_invalid_digest_algorithm_or_length() {
        assert!(normalize_expected_sha256("sha512:abcd").is_err());
        assert!(normalize_expected_sha256("sha256:abcd").is_err());
        assert!(normalize_expected_sha256("not-a-digest").is_err());
    }

    fn sha256_digest(bytes: &[u8]) -> String {
        format!("sha256:{}", hex_lower(&sha2::Sha256::digest(bytes)))
    }

    fn serve_blob(
        content_length: usize,
        head: Vec<u8>,
        tail: Option<(std::sync::mpsc::Receiver<()>, Vec<u8>)>,
    ) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!(
            "http://{}/v2/test/blobs/sha256:blob",
            listener.local_addr().expect("local addr")
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone stream"));
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).expect("read request");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content_length
            )
            .expect("write response head");
            stream.write_all(&head).expect("write body head");
            stream.flush().expect("flush body head");
            if let Some((gate, rest)) = tail {
                gate.recv().expect("wait for release");
                stream.write_all(&rest).expect("write body tail");
            }
        });
        (url, server)
    }

    fn test_body() -> Vec<u8> {
        (0..64 * 1024).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn blob_matches_digest_rejects_truncated_content() {
        let tmp = unique_tmp_dir("oci-blob-digest");
        let body = test_body();
        let digest = sha256_digest(&body);
        let blob = tmp.join("sha256_blob");
        fs::write(&blob, &body).expect("write blob");
        assert!(blob_matches_digest(&blob, &digest).expect("hash blob"));
        fs::write(&blob, &body[..body.len() / 2]).expect("truncate blob");
        assert!(!blob_matches_digest(&blob, &digest).expect("hash truncated blob"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn download_keeps_the_final_name_absent_until_the_blob_is_complete() {
        let tmp = unique_tmp_dir("oci-download-in-progress");
        let body = test_body();
        let digest = sha256_digest(&body);
        let (head, rest) = body.split_at(body.len() / 2);
        let (release, gate) = std::sync::mpsc::channel();
        let (url, server) = serve_blob(body.len(), head.to_vec(), Some((gate, rest.to_vec())));
        let destination = tmp.join("sha256_blob");
        let partial = partial_blob_path(&destination);

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let task = {
            let destination = destination.clone();
            let digest = digest.clone();
            rt.spawn(async move { download_blob(&url, &destination, &digest).await })
        };
        let received = |path: &Path| fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while received(&partial) < head.len() as u64
            && received(&destination) < head.len() as u64
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let final_name_seen_mid_download = destination.exists();
        let partial_seen_mid_download = received(&partial) == head.len() as u64;
        release.send(()).expect("release server");
        let result = rt.block_on(task).expect("join download");
        server.join().expect("server thread");

        assert!(!final_name_seen_mid_download);
        assert!(partial_seen_mid_download);
        result.expect("download");
        assert_eq!(fs::read(&destination).expect("read blob"), body);
        assert!(!partial.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn download_leaves_nothing_behind_when_the_connection_drops() {
        let tmp = unique_tmp_dir("oci-download-dropped");
        let body = test_body();
        let digest = sha256_digest(&body);
        let (url, server) = serve_blob(body.len(), body[..body.len() / 2].to_vec(), None);
        let destination = tmp.join("sha256_blob");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let result = rt.block_on(download_blob(&url, &destination, &digest));
        server.join().expect("server thread");

        assert!(result.is_err());
        assert!(!destination.exists());
        assert!(!partial_blob_path(&destination).exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn download_leaves_nothing_behind_on_a_digest_mismatch() {
        let tmp = unique_tmp_dir("oci-download-mismatch");
        let body = test_body();
        let digest = sha256_digest(b"another blob");
        let (url, server) = serve_blob(body.len(), body.clone(), None);
        let destination = tmp.join("sha256_blob");

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let err = rt
            .block_on(download_blob(&url, &destination, &digest))
            .expect_err("digest mismatch");
        server.join().expect("server thread");

        assert!(err.contains("digest mismatch"));
        assert!(!destination.exists());
        assert!(!partial_blob_path(&destination).exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn download_overwrites_a_partial_file_left_by_a_killed_run() {
        let tmp = unique_tmp_dir("oci-download-stale-partial");
        let body = test_body();
        let digest = sha256_digest(&body);
        let destination = tmp.join("sha256_blob");
        let partial = partial_blob_path(&destination);
        fs::write(&partial, vec![0xa5u8; body.len() * 2]).expect("seed stale partial");
        let (url, server) = serve_blob(body.len(), body.clone(), None);

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(download_blob(&url, &destination, &digest))
            .expect("download");
        server.join().expect("server thread");

        assert_eq!(fs::read(&destination).expect("read blob"), body);
        assert!(!partial.exists());
        let _ = fs::remove_dir_all(&tmp);
    }
}
