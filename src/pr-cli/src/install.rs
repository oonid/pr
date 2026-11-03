use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::color::*;
use crate::commands_extra::force_remove_dir_all;
use crate::install_marker::{pending_install, InstallMarker, PendingInstall};
use crate::install_model::{write_oci_install_metadata, OciInstallMetadata};
use crate::oci::{
    apply_layer_blob, blob_matches_digest, blob_path, blob_url, download_blob_with_bearer,
    link_inside_root, resolve_in_root, sanitize_layer_path, select_manifest_descriptor,
    OciDescriptor, OciManifest, OciPlatform, DOCKER_MANIFEST_LIST_MEDIA_TYPE,
    DOCKER_MANIFEST_MEDIA_TYPE, OCI_IMAGE_INDEX_MEDIA_TYPE, OCI_IMAGE_MANIFEST_MEDIA_TYPE,
};
use crate::plugin::{load_plugins, DistroPlugin};
use crate::shared::{
    build_proot_args, build_proot_child_env, build_proot_runtime_env, get_download_cache_dir,
    get_installed_rootfs_dir, get_native_proot, get_oci_container_dir,
    get_oci_container_manifest_path, get_oci_container_rootfs_dir, get_oci_containers_dir,
    get_plugins_dir, is_valid_container_name, msg_error, msg_status, network_dns_servers,
    read_rootfs_file, resolv_conf_contents, resolve_installed_rootfs, write_rootfs_file,
    DEFAULT_FAKE_KERNEL_RELEASE, DEFAULT_FAKE_KERNEL_VERSION,
};
use crate::source_parse::{InstallSourceInput, InstallSourceInputKind};

fn detect_device_arch() -> String {
    if let Ok(arch) = std::env::var("DISTRO_ARCH") {
        if !arch.is_empty() {
            return arch;
        }
    }

    let mut data = [0u8; 20];
    let header = fs::File::open("/proc/self/exe").and_then(|mut f| f.read_exact(&mut data));

    match header {
        Ok(()) => {
            if &data[1..4] == b"ELF" {
                let machine = &data[18..20];
                match machine {
                    [0xb7, _] => "aarch64".to_string(),
                    [0x28, _] => "arm".to_string(),
                    [0x3e, _] => "x86_64".to_string(),
                    [0x03, _] => "i686".to_string(),
                    [0xf3, _] => "riscv64".to_string(),
                    [0x08, _] => "mips".to_string(),
                    _ => "unknown".to_string(),
                }
            } else {
                "unknown".to_string()
            }
        }
        Err(_) => "unknown".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedOciReference {
    registry_base: String,
    repository: String,
    reference: String,
    digest_reference: bool,
}

#[derive(Debug, Deserialize)]
struct RegistryManifestResponse {
    #[serde(default, rename = "schemaVersion")]
    schema_version: u32,
    #[serde(default, rename = "mediaType")]
    media_type: Option<String>,
    #[serde(default)]
    manifests: Vec<RegistryDescriptor>,
    #[serde(default)]
    config: Option<RegistryDescriptor>,
    #[serde(default)]
    layers: Vec<RegistryDescriptor>,
}

#[derive(Debug, Deserialize)]
struct RegistryDescriptor {
    #[serde(default, rename = "mediaType")]
    media_type: Option<String>,
    digest: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    annotations: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    platform: Option<RegistryPlatform>,
}

#[derive(Debug, Deserialize)]
struct RegistryPlatform {
    #[serde(default)]
    os: String,
    #[serde(default)]
    architecture: String,
    #[serde(default)]
    variant: Option<String>,
    #[serde(default, rename = "os.version")]
    os_version: Option<String>,
    #[serde(default, rename = "os.features")]
    os_features: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct BearerTokenResponse {
    token: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug)]
struct BearerChallenge {
    realm: String,
    service: Option<String>,
    scope: Option<String>,
}

#[derive(Debug)]
struct ResolvedOciManifest {
    manifest: OciManifest,
    bearer_token: Option<String>,
}

fn resolve_oci_reference(input: &str) -> Result<ResolvedOciReference, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty OCI reference".to_string());
    }

    let (name_part, reference, is_digest) = if let Some((name, digest)) = input.rsplit_once('@') {
        if name.is_empty() || digest.is_empty() {
            return Err(format!("invalid OCI reference '{}'", input));
        }
        (name, digest.to_string(), true)
    } else {
        let last_slash = input.rfind('/');
        let last_colon = input.rfind(':');
        let (name, tag) = match (last_slash, last_colon) {
            (_, None) => (input, "latest".to_string()),
            (Some(slash), Some(colon)) if colon > slash => {
                (&input[..colon], input[colon + 1..].to_string())
            }
            (None, Some(colon)) => (&input[..colon], input[colon + 1..].to_string()),
            _ => (input, "latest".to_string()),
        };
        if name.is_empty() || tag.is_empty() {
            return Err(format!("invalid OCI reference '{}'", input));
        }
        (name, tag, false)
    };

    let segments: Vec<&str> = name_part.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(format!("invalid OCI reference '{}'", input));
    }

    let (registry_host, repository) = if segments.len() == 1 {
        (
            "registry-1.docker.io".to_string(),
            format!("library/{}", segments[0]),
        )
    } else if looks_like_registry_host(segments[0]) {
        let registry_host = normalize_registry_host(segments[0]);
        let mut repository = segments[1..].join("/");
        if is_docker_hub_host(&registry_host) && !repository.contains('/') {
            repository = format!("library/{}", repository);
        }
        (registry_host, repository)
    } else {
        ("registry-1.docker.io".to_string(), segments.join("/"))
    };

    if repository.is_empty() {
        return Err(format!("invalid OCI repository '{}'", input));
    }

    let reference = if is_digest {
        if !reference.contains(':') {
            return Err(format!("invalid OCI digest reference '{}'", input));
        }
        reference
    } else {
        reference
    };

    Ok(ResolvedOciReference {
        registry_base: format!("https://{}", registry_host),
        repository,
        reference,
        digest_reference: is_digest,
    })
}

fn looks_like_registry_host(segment: &str) -> bool {
    segment == "localhost" || segment.contains('.') || segment.contains(':')
}

fn normalize_registry_host(host: &str) -> String {
    let normalized = host.to_ascii_lowercase();
    if normalized == "docker.io" || normalized == "index.docker.io" {
        "registry-1.docker.io".to_string()
    } else {
        normalized
    }
}

fn is_docker_hub_host(host: &str) -> bool {
    matches!(
        host,
        "docker.io" | "index.docker.io" | "registry-1.docker.io"
    )
}

fn default_install_name_for_oci(reference: &ResolvedOciReference) -> String {
    reference
        .repository
        .rsplit('/')
        .next()
        .map(sanitize_install_name)
        .unwrap_or_else(|| "container".to_string())
}

fn derive_oci_install_name(
    resolved: &ResolvedOciReference,
    override_alias: Option<&str>,
) -> Result<String, String> {
    if let Some(alias) = override_alias {
        validate_alias_format(alias)?;
        return Ok(alias.to_string());
    }

    let default_name = default_install_name_for_oci(resolved);
    if validate_alias_format(&default_name).is_ok() {
        return Ok(default_name);
    }

    Ok("container".to_string())
}

fn normalized_oci_reference(reference: &ResolvedOciReference) -> String {
    let registry_host = reference
        .registry_base
        .strip_prefix("https://")
        .unwrap_or(reference.registry_base.as_str())
        .trim_end_matches('/');
    if reference.digest_reference {
        format!(
            "{}/{}@{}",
            registry_host, reference.repository, reference.reference
        )
    } else {
        format!(
            "{}/{}:{}",
            registry_host, reference.repository, reference.reference
        )
    }
}

fn sanitize_install_name(raw: &str) -> String {
    let mut output = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-') {
            output.push(c.to_ascii_lowercase());
        }
    }
    if output.is_empty() {
        "container".to_string()
    } else {
        output
    }
}

fn validate_alias_format(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("argument to --override-alias should not be empty".to_string());
    }
    if alias.ends_with(".sh") {
        return Err("argument to --override-alias should not end with '.sh'".to_string());
    }
    if alias.contains('/') || alias.contains('\\') || alias.contains("..") {
        return Err("argument to --override-alias must not contain path separators".to_string());
    }
    if !alias
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err(
            "argument to --override-alias should start with an alphanumeric character".to_string(),
        );
    }
    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '+' || c == '-')
    {
        return Err(
            "argument to --override-alias should consist of alphanumeric characters including symbols '_.+-'"
                .to_string(),
        );
    }
    Ok(())
}

fn map_descriptor(input: RegistryDescriptor) -> OciDescriptor {
    OciDescriptor {
        media_type: input.media_type,
        digest: input.digest,
        size: input.size,
        urls: input.urls,
        annotations: input.annotations,
        platform: input.platform.map(|platform| OciPlatform {
            os: platform.os,
            architecture: platform.architecture,
            variant: platform.variant,
            os_version: platform.os_version,
            os_features: platform.os_features,
        }),
    }
}

async fn fetch_bearer_token(
    client: &reqwest::Client,
    challenge: &BearerChallenge,
) -> Result<String, String> {
    let mut request = client.get(&challenge.realm);
    if let Some(service) = challenge.service.as_deref() {
        request = request.query(&[("service", service)]);
    }
    if let Some(scope) = challenge.scope.as_deref() {
        request = request.query(&[("scope", scope)]);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("fetch OCI auth token: {}", e))?;
    if !response.status().is_success() {
        return Err(format!(
            "fetch OCI auth token failed with HTTP {}",
            response.status()
        ));
    }
    let body: BearerTokenResponse = response
        .json()
        .await
        .map_err(|e| format!("decode OCI auth token response: {}", e))?;
    body.token
        .or(body.access_token)
        .ok_or_else(|| "OCI auth token response missing token field".to_string())
}

fn parse_bearer_challenge(header: &str) -> Option<BearerChallenge> {
    let header = header.trim();
    let rest = header.strip_prefix("Bearer ")?;
    let mut realm = None;
    let mut service = None;
    let mut scope = None;

    for part in rest.split(',') {
        let (key, value) = part.trim().split_once('=')?;
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "realm" => realm = Some(value),
            "service" => service = Some(value),
            "scope" => scope = Some(value),
            _ => {}
        }
    }

    Some(BearerChallenge {
        realm: realm?,
        service,
        scope,
    })
}

async fn fetch_manifest_json(
    client: &reqwest::Client,
    url: &str,
) -> Result<(RegistryManifestResponse, Option<String>, Option<String>), String> {
    let accept = format!(
        "{},{},{},{}",
        OCI_IMAGE_INDEX_MEDIA_TYPE,
        DOCKER_MANIFEST_LIST_MEDIA_TYPE,
        OCI_IMAGE_MANIFEST_MEDIA_TYPE,
        DOCKER_MANIFEST_MEDIA_TYPE
    );
    let mut bearer_token: Option<String> = None;

    for _ in 0..2 {
        let mut request = client.get(url).header(reqwest::header::ACCEPT, &accept);
        if let Some(token) = bearer_token.as_deref() {
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {}", token));
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("request OCI manifest {}: {}", url, e))?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED && bearer_token.is_none() {
            let challenge_header = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| {
                    "OCI registry requires auth but no challenge was provided".to_string()
                })?;
            let challenge = parse_bearer_challenge(challenge_header).ok_or_else(|| {
                format!(
                    "unsupported WWW-Authenticate challenge: {}",
                    challenge_header
                )
            })?;
            bearer_token = Some(fetch_bearer_token(client, &challenge).await?);
            continue;
        }

        if !response.status().is_success() {
            return Err(format!(
                "request OCI manifest {} failed with HTTP {}",
                url,
                response.status()
            ));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .map(|v| v.to_string());
        let body = response
            .json::<RegistryManifestResponse>()
            .await
            .map_err(|e| format!("decode OCI manifest {}: {}", url, e))?;
        return Ok((body, content_type, bearer_token.clone()));
    }

    Err(format!("failed to authorize OCI manifest request {}", url))
}

async fn resolve_oci_manifest(
    oci_ref: &ResolvedOciReference,
    architecture: &str,
) -> Result<ResolvedOciManifest, String> {
    let client = reqwest::Client::builder()
        .user_agent("pr-cli-oci/0.1")
        .build()
        .map_err(|e| format!("create OCI registry client: {}", e))?;

    let manifest_url = format!(
        "{}/v2/{}/manifests/{}",
        oci_ref.registry_base.trim_end_matches('/'),
        oci_ref.repository.trim_matches('/'),
        oci_ref.reference
    );
    let (top_manifest, top_content_type, top_token) =
        fetch_manifest_json(&client, &manifest_url).await?;

    let top_media_type = top_manifest
        .media_type
        .as_deref()
        .or(top_content_type.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_index = top_media_type.contains("manifest.list")
        || top_media_type.contains("image.index")
        || !top_manifest.manifests.is_empty();

    let (selected_manifest, selected_token) = if is_index {
        let descriptors: Vec<OciDescriptor> = top_manifest
            .manifests
            .into_iter()
            .map(map_descriptor)
            .collect();
        let selected = select_manifest_descriptor(&descriptors, architecture).ok_or_else(|| {
            format!(
                "OCI image '{}' does not provide a supported manifest for architecture '{}'",
                oci_ref.repository, architecture
            )
        })?;
        let selected_url = format!(
            "{}/v2/{}/manifests/{}",
            oci_ref.registry_base.trim_end_matches('/'),
            oci_ref.repository.trim_matches('/'),
            selected.digest
        );
        let (manifest, _, token) = fetch_manifest_json(&client, &selected_url).await?;
        (manifest, token.or(top_token))
    } else {
        (top_manifest, top_token)
    };

    let config = selected_manifest
        .config
        .ok_or_else(|| "OCI manifest missing config descriptor".to_string())?;
    if selected_manifest.schema_version != 2 {
        return Err(format!(
            "unsupported OCI manifest schema version {}",
            selected_manifest.schema_version
        ));
    }

    Ok(ResolvedOciManifest {
        manifest: OciManifest {
            schema_version: selected_manifest.schema_version,
            media_type: selected_manifest.media_type,
            config: map_descriptor(config),
            layers: selected_manifest
                .layers
                .into_iter()
                .map(map_descriptor)
                .collect(),
            annotations: std::collections::BTreeMap::new(),
        },
        bearer_token: selected_token,
    })
}

fn cleanup_oci_on_failure(container_dir: &Path) {
    let containers_root = Path::new(&get_oci_containers_dir()).to_path_buf();
    if container_dir.parent() != Some(containers_root.as_path()) {
        return;
    }
    let Some(leaf_name) = container_dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    if validate_alias_format(leaf_name).is_err() {
        return;
    }

    match fs::symlink_metadata(container_dir) {
        Ok(meta) if meta.is_dir() => force_remove_dir_all(container_dir),
        Ok(_) => {
            let _ = fs::remove_file(container_dir);
        }
        Err(_) => return,
    }
    if fs::symlink_metadata(container_dir).is_ok() {
        msg_error(&format!(
            "could not remove the partial container '{}'",
            container_dir.display()
        ));
    }
}

fn claim_container(
    parent: &Path,
    name: &str,
    dir: &Path,
    installed: impl Fn() -> bool,
) -> Result<InstallMarker, String> {
    let marker = InstallMarker::claim_reporting(parent, name)?;
    if marker.took_over() {
        msg_status(&format!(
            "Removing the interrupted install of '{}'...",
            name
        ));
        force_remove_dir_all(dir);
        if fs::symlink_metadata(dir).is_ok() {
            return Err(format!(
                "could not remove the interrupted install '{}'",
                dir.display()
            ));
        }
    } else if installed() {
        marker.finish()?;
        return Err(format!("distribution '{}' is already installed", name));
    }
    Ok(marker)
}

fn refuse_unfinished_install_in(other_parent: &str, name: &str) -> Result<(), String> {
    if pending_install(Path::new(other_parent), name).is_some() {
        msg_error(&format!(
            "'{}' has an unfinished install of another kind. Remove it with: pr-cli remove {}",
            name, name
        ));
        return Err(format!("'{}' has an unfinished install", name));
    }
    Ok(())
}

fn install_container<I, F, C>(
    parent: &Path,
    name: &str,
    dir: &Path,
    installed: I,
    populate: F,
    cleanup: C,
) -> Result<(), String>
where
    I: Fn() -> bool,
    F: FnOnce() -> Result<(), String>,
    C: FnOnce(),
{
    let marker = claim_container(parent, name, dir, installed)?;
    match populate() {
        Ok(()) => marker.finish(),
        Err(e) => {
            cleanup();
            if fs::symlink_metadata(dir).is_err() {
                let _ = marker.finish();
            }
            Err(e)
        }
    }
}

fn install_oci_layers<F>(
    layers: &[OciDescriptor],
    cache_dir: &Path,
    rootfs: &Path,
    mut fetch: F,
) -> Result<(), String>
where
    F: FnMut(&OciDescriptor, &Path) -> Result<(), String>,
{
    for layer in layers {
        let blob = blob_path(cache_dir, &layer.digest)?;
        if blob.exists() && !blob_matches_digest(&blob, &layer.digest)? {
            msg_status(&format!(
                "Cached OCI layer {} is damaged; downloading again...",
                layer.digest
            ));
            fs::remove_file(&blob)
                .map_err(|e| format!("remove damaged blob {}: {}", blob.display(), e))?;
        }
        if !blob.exists() {
            msg_status(&format!("Downloading OCI layer {}...", layer.digest));
            fetch(layer, &blob)?;
        }
        apply_layer_blob(&blob, rootfs)?;
    }
    Ok(())
}

fn install_from_oci_reference(reference: &str, override_alias: Option<&str>) -> Result<(), String> {
    let resolved = resolve_oci_reference(reference)?;
    let install_name = derive_oci_install_name(&resolved, override_alias)?;

    install_oci_container(&install_name, || {
        msg_status(&format!(
            "Installing OCI image {}:{} as '{}'...",
            resolved.repository, resolved.reference, install_name
        ));
        populate_oci_container(reference, &resolved, &install_name)
    })?;
    msg_status("Finished.");
    Ok(())
}

fn install_oci_container<F>(install_name: &str, populate: F) -> Result<(), String>
where
    F: FnOnce() -> Result<(), String>,
{
    if resolve_installed_rootfs(install_name).is_some() {
        return Err(format!(
            "distribution '{}' is already installed",
            install_name
        ));
    }

    refuse_unfinished_install_in(&get_installed_rootfs_dir(), install_name)?;
    let container_dir = PathBuf::from(get_oci_container_dir(install_name));
    install_container(
        Path::new(&get_oci_containers_dir()),
        install_name,
        &container_dir,
        || container_dir.join("rootfs").is_dir(),
        populate,
        || cleanup_oci_on_failure(&container_dir),
    )
}

fn populate_oci_container(
    reference: &str,
    resolved: &ResolvedOciReference,
    install_name: &str,
) -> Result<(), String> {
    let rootfs = get_oci_container_rootfs_dir(install_name);
    let metadata_path = get_oci_container_manifest_path(install_name);

    fs::create_dir_all(&rootfs).map_err(|e| format!("create OCI rootfs dir: {}", e))?;
    let l2s_dir = format!("{}/.l2s", rootfs);
    fs::create_dir_all(&l2s_dir).map_err(|e| format!("create OCI .l2s dir: {}", e))?;

    let device_arch = detect_device_arch();
    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("create tokio runtime: {}", e))?;
    let resolved_manifest = rt.block_on(resolve_oci_manifest(resolved, &device_arch))?;
    let bearer_token = resolved_manifest.bearer_token;
    let manifest = resolved_manifest.manifest;

    if manifest.layers.is_empty() {
        return Err("OCI manifest does not contain filesystem layers".to_string());
    }

    let cache_dir = Path::new(&get_download_cache_dir()).to_path_buf();
    fs::create_dir_all(&cache_dir).map_err(|e| format!("create cache dir: {}", e))?;
    install_oci_layers(
        &manifest.layers,
        &cache_dir,
        Path::new(&rootfs),
        |layer, blob| {
            let url = blob_url(&resolved.registry_base, &resolved.repository, &layer.digest);
            rt.block_on(download_blob_with_bearer(
                &url,
                blob,
                &layer.digest,
                bearer_token.as_deref(),
            ))
        },
    )?;

    if !Path::new(&format!("{}/etc", rootfs)).exists() {
        return Err("OCI rootfs has unexpected structure (missing /etc)".to_string());
    }

    write_config_files(&rootfs, None)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock error while writing OCI metadata: {}", e))?
        .as_secs();
    let metadata = OciInstallMetadata::new(
        install_name.to_string(),
        reference,
        normalized_oci_reference(resolved),
        device_arch,
        created_at,
    );
    write_oci_install_metadata(Path::new(&metadata_path), &metadata)
}

fn strip_archive_path(path: &Path, strip_components: usize) -> Result<Option<PathBuf>, String> {
    let rest: PathBuf = path.components().skip(strip_components).collect();
    let clean = sanitize_layer_path(&rest)?;
    Ok((!clean.as_os_str().is_empty()).then_some(clean))
}

fn extract_tarball(
    archive_path: &str,
    dest: &str,
    strip_components: usize,
    exclude: &[&str],
) -> Result<(), String> {
    let root = Path::new(dest);
    fs::create_dir_all(root).map_err(|e| format!("create {}: {}", dest, e))?;
    let file = fs::File::open(archive_path).map_err(|e| format!("open archive: {}", e))?;
    let decompressor = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressor);

    for entry in archive
        .entries()
        .map_err(|e| format!("read tar entries: {}", e))?
    {
        let mut entry = entry.map_err(|e| format!("read tar entry: {}", e))?;
        let path = entry.path().map_err(|e| format!("get tar path: {}", e))?;
        let Some(relative) = strip_archive_path(&path, strip_components)? else {
            continue;
        };
        if exclude.iter().any(|exc| relative.starts_with(exc)) {
            continue;
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|e| format!("read hardlink target: {}", e))?
                .ok_or_else(|| format!("hardlink {} has no target", relative.display()))?;
            let Some(target) = strip_archive_path(&target, strip_components)? else {
                continue;
            };
            if exclude.iter().any(|exc| target.starts_with(exc)) {
                continue;
            }
            if !link_inside_root(root, &relative, &target)? {
                return Err(format!(
                    "extract {}: hardlink target missing {}",
                    relative.display(),
                    target.display()
                ));
            }
            continue;
        }

        unpack_entry(root, &relative, &mut entry)?;
    }

    // Post-extraction: ensure ELF binaries have their execute bit set.
    // This is a safety net in case fchmod silently failed for some entries.
    fix_elf_execute_bits(dest);

    Ok(())
}

pub(crate) fn unpack_entry<R: Read>(
    root: &Path,
    relative: &Path,
    entry: &mut tar::Entry<R>,
) -> Result<(), String> {
    let entry_type = entry.header().entry_type();
    let dest_path = resolve_in_root(root, relative, false)?;
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {}", parent.display(), e))?;
    }
    let existing = fs::symlink_metadata(&dest_path).ok();
    if let Some(meta) = &existing {
        if !(entry_type.is_dir() && meta.is_dir()) {
            if meta.is_dir() {
                fs::remove_dir_all(&dest_path)
            } else {
                fs::remove_file(&dest_path)
            }
            .map_err(|e| format!("replace {}: {}", dest_path.display(), e))?;
        }
    }

    if entry_type.is_dir() {
        fs::create_dir_all(&dest_path)
            .map_err(|e| format!("create dir {}: {}", dest_path.display(), e))?;
        let mode = entry.header().mode().unwrap_or(0o755) | 0o700;
        let _ = fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode));
    } else if entry_type.is_symlink() {
        let target = entry
            .link_name()
            .map_err(|e| format!("read symlink target: {}", e))?
            .map(|t| t.into_owned())
            .unwrap_or_default();
        std::os::unix::fs::symlink(&target, &dest_path).map_err(|e| {
            format!(
                "symlink {} -> {}: {}",
                dest_path.display(),
                target.display(),
                e
            )
        })?;
    } else {
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest_path)
            .map_err(|e| format!("create {}: {}", dest_path.display(), e))?;
        std::io::copy(entry, &mut out)
            .map_err(|e| format!("write {}: {}", dest_path.display(), e))?;
        let mode = entry.header().mode().unwrap_or(0o644);
        // Use fchmod on the open fd for reliable permission setting on Android.
        // Strip setuid/setgid (0o6000) bits; Android blocks those for non-root.
        let safe_mode = (mode & 0o1777) as libc::mode_t;
        unsafe {
            libc::fchmod(out.as_raw_fd(), safe_mode);
        }
    }
    Ok(())
}

/// Walk `rootfs`, read the first 4 bytes of every regular file, and add the
/// user/group/other execute bit for any file whose magic is `\x7fELF`.
fn fix_elf_execute_bits(rootfs: &str) {
    let Ok(walker) = fs::read_dir(rootfs) else {
        return;
    };
    let mut stack: Vec<std::path::PathBuf> = walker.flatten().map(|e| e.path()).collect();

    while let Some(path) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(&path) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else if meta.is_file() {
            let mode = meta.permissions().mode();
            if mode & 0o111 != 0 {
                continue; // already executable
            }
            // Peek at magic bytes
            let mut buf = [0u8; 4];
            if let Ok(mut f) = fs::File::open(&path) {
                use std::io::Read as _;
                if f.read_exact(&mut buf).is_ok() && &buf == b"\x7fELF" {
                    let new_mode = (mode | 0o111) as libc::mode_t;
                    if let Ok(cstr) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) {
                        unsafe {
                            libc::chmod(cstr.as_ptr(), new_mode);
                        }
                    }
                }
            }
        }
    }
}

fn download_file(url: &str, output_path: &str, max_retries: u32) -> Result<(), String> {
    let mut retry = 0;
    let mut delay = 5u64;

    while retry < max_retries {
        if retry > 0 {
            println!(
                "{}[{}*{}{}] Retry {}/{} after {}s...{}",
                BLUE, YELLOW, BLUE, CYAN, retry, max_retries, delay, RESET
            );
            std::thread::sleep(std::time::Duration::from_secs(delay));
            delay = (delay * 2).min(60);
        }

        let _ = fs::remove_file(output_path);

        let rt =
            tokio::runtime::Runtime::new().map_err(|e| format!("create tokio runtime: {}", e))?;

        let result = rt.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| format!("build HTTP client: {}", e))?;

            let resp = client.get(url).send().await.map_err(|e| {
                let mut msg = format!("HTTP request failed: {}", e);
                let mut source: Option<&dyn Error> = e.source();
                while let Some(err) = source {
                    msg.push_str(&format!("\n  caused by: {}", err));
                    source = err.source();
                }
                eprintln!("{}", msg);
                msg
            })?;

            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }

            let total = resp.content_length();
            let mut file =
                fs::File::create(output_path).map_err(|e| format!("create file: {}", e))?;

            let mut downloaded: u64 = 0;
            let mut stream = resp.bytes_stream();
            use futures_util::StreamExt;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("read chunk: {}", e))?;
                file.write_all(&chunk)
                    .map_err(|e| format!("write chunk: {}", e))?;
                downloaded += chunk.len() as u64;

                if let Some(total) = total {
                    if total > 0 {
                        let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
                        println!(
                            "\r{}[{}*{}{}] {:.1}MB / {:.1}MB ({}%){}   ",
                            BLUE,
                            GREEN,
                            BLUE,
                            CYAN,
                            downloaded as f64 / (1024.0 * 1024.0),
                            total as f64 / (1024.0 * 1024.0),
                            pct,
                            RESET
                        );
                    }
                }
            }

            file.sync_all().map_err(|e| format!("flush file: {}", e))?;
            drop(file);

            Ok(())
        });

        match result {
            Ok(()) => {
                if Path::new(output_path).exists()
                    && fs::metadata(output_path).map(|m| m.len()).unwrap_or(0) > 0
                {
                    return Ok(());
                }
            }
            Err(e) => {
                eprintln!("download error: {}", e);
            }
        }

        retry += 1;
    }

    let _ = fs::remove_file(output_path);
    Err(format!("download failed after {} retries", max_retries))
}

fn verify_sha256(expected: &str, filepath: &str) -> Result<(), String> {
    let mut file = fs::File::open(filepath).map_err(|e| format!("open {}: {}", filepath, e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("read {}: {}", filepath, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hash = hasher.finalize();
    let actual = format!("{:x}", hash);

    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA256 mismatch: expected {}, got {}",
            expected, actual
        ))
    }
}

fn setup_fake_sysdata(rootfs: &str) -> Result<(), String> {
    let root = Path::new(rootfs);
    for d in &["proc", "sys", "sys/.empty"] {
        let path = resolve_in_root(root, Path::new(d), true)?;
        fs::create_dir_all(&path)
            .map_err(|e| format!("failed to create {}: {}", path.display(), e))?;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
    }

    let loadavg = "0.12 0.07 0.02 2/165 765\n";
    write_rootfs_file(root, "proc/.loadavg", loadavg, None)?;

    let stat = "cpu  1957 0 2877 93280 262 342 254 87 0 0\ncpu0 31 0 226 12027 82 10 4 9 0 0\ncpu1 45 0 664 11144 21 263 233 12 0 0\ncpu2 494 0 537 11283 27 10 3 8 0 0\ncpu3 359 0 234 11723 24 26 5 7 0 0\ncpu4 295 0 268 11772 10 12 2 12 0 0\ncpu5 270 0 251 11833 15 3 1 10 0 0\ncpu6 430 0 520 11386 30 8 1 12 0 0\ncpu7 30 0 172 12108 50 8 1 13 0 0\nintr 127541\nctxt 140223\nbtime 1680020856\nprocesses 772\nprocs_running 2\nprocs_blocked 0\nsoftirq 75663 0 5903 6 25375 10774 0 243 11685 0 21677\n";
    write_rootfs_file(root, "proc/.stat", stat, None)?;

    let uptime = "124.08 932.80\n";
    write_rootfs_file(root, "proc/.uptime", uptime, None)?;

    let version = format!(
        "Linux version {} (proot@pr) (gcc (GCC) 13.3.0, GNU ld (GNU Binutils) 2.42) {}\n",
        DEFAULT_FAKE_KERNEL_RELEASE, DEFAULT_FAKE_KERNEL_VERSION
    );
    write_rootfs_file(root, "proc/.version", &version, None)?;

    let vmstat = "nr_free_pages 1743136\nnr_zone_inactive_anon 179281\nnr_zone_active_anon 7183\nnr_zone_inactive_file 22858\nnr_zone_active_file 51328\nnr_zone_unevictable 642\nnr_zone_write_pending 0\nnr_mlock 0\nnr_bounce 0\n";
    write_rootfs_file(root, "proc/.vmstat", vmstat, None)?;

    Ok(())
}

fn followed_relative(rootfs: &Path, relative: &str) -> Result<String, String> {
    resolve_in_root(rootfs, Path::new(relative), true)?
        .strip_prefix(rootfs)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|_| format!("resolve {}", relative))
}

fn append_line_if_missing(rootfs: &Path, relative: &str, line: &str) -> Result<(), String> {
    let target = followed_relative(rootfs, relative)?;
    let mut content = read_rootfs_file(rootfs, &target)?.unwrap_or_default();
    if !content
        .lines()
        .any(|existing| existing.trim() == line.trim())
    {
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push_str(line);
        content.push('\n');
        write_rootfs_file(rootfs, &target, &content, None)?;
    }
    Ok(())
}

fn uncomment_en_us_locale(rootfs: &str) -> Result<(), String> {
    let root = Path::new(rootfs);
    let target = followed_relative(root, "etc/locale.gen")?;
    let content = read_rootfs_file(root, &target)?
        .ok_or_else(|| format!("read {}/etc/locale.gen: not found", rootfs))?;
    let updated = content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#") {
                let uncommented = trimmed.trim_start_matches('#').trim_start();
                if uncommented.starts_with("en_US.UTF-8") && uncommented.contains("UTF-8") {
                    return uncommented.to_string();
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    write_rootfs_file(root, &target, &format!("{}\n", updated), None)
}

fn guest_command(rootfs: &str, argv: &[&str], envs: &[(&str, &str)]) -> Command {
    let args = build_proot_args(rootfs, true, false, &[]);
    let mut cmd = Command::new(get_native_proot());
    cmd.envs(build_proot_runtime_env())
        .envs(build_proot_child_env())
        .env("LANG", "C.UTF-8")
        .envs(envs.iter().copied())
        .args(args)
        .args(argv);
    cmd
}

fn run_guest_command(rootfs: &str, argv: &[&str], envs: &[(&str, &str)]) -> Result<(), String> {
    let command = argv.join(" ");
    let status = guest_command(rootfs, argv, envs)
        .status()
        .map_err(|e| format!("run guest command '{}': {}", command, e))?;
    if !status.success() {
        return Err(format!(
            "guest command failed with exit code {:?}: {}",
            status.code(),
            command
        ));
    }
    Ok(())
}

fn apply_rust_owned_distro_setup(rootfs: &str, distro_alias: &str) -> Result<(), String> {
    match distro_alias {
        "debian" => {
            uncomment_en_us_locale(rootfs)?;
            run_guest_command(
                rootfs,
                &["dpkg-reconfigure", "locales"],
                &[("DEBIAN_FRONTEND", "noninteractive")],
            )?;
        }
        "ubuntu" => {
            uncomment_en_us_locale(rootfs)?;
            run_guest_command(
                rootfs,
                &["dpkg-reconfigure", "locales"],
                &[("DEBIAN_FRONTEND", "noninteractive")],
            )?;
            if let Err(e) = run_guest_command(
                rootfs,
                &[
                    "add-apt-repository",
                    "--yes",
                    "--no-update",
                    "ppa:mozillateam/ppa",
                ],
                &[],
            ) {
                msg_error(&format!(
                    "adding the Mozilla PPA failed (continuing): {}",
                    e
                ));
            }
            let pin = "etc/apt/preferences.d/pin-mozilla-ppa";
            let pin_dir =
                resolve_in_root(Path::new(rootfs), Path::new("etc/apt/preferences.d"), true)?;
            fs::create_dir_all(&pin_dir)
                .map_err(|e| format!("create {}: {}", pin_dir.display(), e))?;
            write_rootfs_file(
                Path::new(rootfs),
                pin,
                "Package: *\nPin: release o=LP-PPA-mozillateam\nPin-Priority: 9999\n",
                None,
            )?;
        }
        "archlinux" => {
            for file in ["su", "su-l", "system-local-login", "system-remote-login"] {
                append_line_if_missing(
                    Path::new(rootfs),
                    &format!("etc/pam.d/{}", file),
                    "session  required  pam_env.so readenv=1",
                )?;
            }
            uncomment_en_us_locale(rootfs)?;
            run_guest_command(rootfs, &["locale-gen"], &[])?;
        }
        "manjaro" => {
            for file in ["su", "su-l", "system-local-login", "system-remote-login"] {
                append_line_if_missing(
                    Path::new(rootfs),
                    &format!("etc/pam.d/{}", file),
                    "session  required  pam_env.so readenv=1",
                )?;
            }
        }
        "fedora" => {
            run_guest_command(rootfs, &["authselect", "opt-out"], &[])?;
            append_line_if_missing(
                Path::new(rootfs),
                "etc/pam.d/system-auth",
                "session  required  pam_env.so readenv=1",
            )?;
        }
        "opensuse" => {
            run_guest_command(rootfs, &["zypper", "al", "filesystem"], &[])?;
        }
        _ => {}
    }
    Ok(())
}

fn has_rust_owned_distro_setup(distro_alias: &str) -> bool {
    matches!(
        distro_alias,
        "debian" | "ubuntu" | "archlinux" | "manjaro" | "fedora" | "opensuse"
    )
}

fn write_config_files(rootfs: &str, distro_name: Option<&str>) -> Result<(), String> {
    let default_path_env = crate::shared::DEFAULT_PATH_ENV_SUFFIX;

    // /etc/resolv.conf
    let root = Path::new(rootfs);
    let resolv_path = format!("{}/etc/resolv.conf", rootfs);
    let resolv = resolv_conf_contents(&network_dns_servers());
    write_rootfs_file(root, "etc/resolv.conf", &resolv, Some(0o644))?;
    msg_status(&format!("Creating file '{}'...", resolv_path));

    // /etc/hosts
    let hosts_path = format!("{}/etc/hosts", rootfs);
    let hosts = "# IPv4.\n127.0.0.1   localhost.localdomain localhost\n\n# IPv6.\n::1         localhost.localdomain localhost ip6-localhost ip6-loopback\nfe00::0     ip6-localnet\nff00::0     ip6-mcastprefix\nff02::1     ip6-allnodes\nff02::2     ip6-allrouters\nff02::3     ip6-allhosts\n";
    write_rootfs_file(root, "etc/hosts", hosts, Some(0o644))?;
    msg_status(&format!("Creating file '{}'...", hosts_path));

    // /etc/environment
    let env_path = format!("{}/etc/environment", rootfs);
    let mut env_content = String::new();
    for var in &[
        "ANDROID_ART_ROOT",
        "ANDROID_DATA",
        "ANDROID_I18N_ROOT",
        "ANDROID_ROOT",
        "ANDROID_RUNTIME_ROOT",
        "ANDROID_TZDATA_ROOT",
        "BOOTCLASSPATH",
        "DEX2OATBOOTCLASSPATH",
        "EXTERNAL_STORAGE",
    ] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                env_content.push_str(&format!("{}={}\n", var, val));
            }
        }
    }
    let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string());
    env_content.push_str(&format!(
        "LANG=en_US.UTF-8\nMOZ_FAKE_NO_SANDBOX=1\nPATH={}\nPULSE_SERVER=127.0.0.1\nTERM={}\nTMPDIR=/tmp\n",
        default_path_env, term
    ));
    write_rootfs_file(root, "etc/environment", &env_content, Some(0o644))?;
    msg_status(&format!("Writing file '{}'...", env_path));

    // /etc/passwd, /etc/group, /etc/shadow, /etc/gshadow - register Android UIDs
    msg_status("Registering Android-specific UIDs and GIDs...");

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let identity = crate::shared::current_identity();
    let username = identity.user.as_str();

    // passwd entry
    let passwd_entry = format!(
        "aid_{}:x:{}:{}:proot-distro:/:/sbin/nologin\n",
        username, uid, gid
    );
    let mut passwd = read_rootfs_file(root, "etc/passwd")?.unwrap_or_default();
    passwd.push_str(&passwd_entry);
    write_rootfs_file(root, "etc/passwd", &passwd, Some(0o644))?;

    // shadow entry
    let shadow_entry = format!("aid_{}:*:18446:0:99999:7:::\n", username);
    let mut shadow = read_rootfs_file(root, "etc/shadow")?.unwrap_or_default();
    shadow.push_str(&shadow_entry);
    write_rootfs_file(root, "etc/shadow", &shadow, Some(0o644))?;

    // group entries
    let group_names: Vec<&str> = identity.groups.iter().map(|(n, _)| n.as_str()).collect();

    let mut group = read_rootfs_file(root, "etc/group")?.unwrap_or_default();
    for (gname, gid_val) in &identity.groups {
        group.push_str(&format!(
            "aid_{}:x:{}:root,aid_{}\n",
            gname, gid_val, username
        ));
    }
    write_rootfs_file(root, "etc/group", &group, Some(0o644))?;

    // gshadow entries
    let gshadow_path = format!("{}/etc/gshadow", rootfs);
    if fs::symlink_metadata(&gshadow_path).is_ok() {
        let mut gshadow = read_rootfs_file(root, "etc/gshadow")?.unwrap_or_default();
        for gname in &group_names {
            gshadow.push_str(&format!("aid_{}:*::root,aid_{}\n", gname, username));
        }
        write_rootfs_file(root, "etc/gshadow", &gshadow, Some(0o644))?;
    }

    // Fake /proc and /sys
    msg_status(&format!(
        "Creating fake /proc and /sys data in '{}'...",
        rootfs
    ));
    setup_fake_sysdata(rootfs)?;

    // Rust-owned distro-specific setup profile
    let Some(distro_name) = distro_name else {
        return Ok(());
    };
    if has_rust_owned_distro_setup(distro_name) {
        msg_status("Running distribution-specific configuration steps...");
        if let Err(e) = apply_rust_owned_distro_setup(rootfs, distro_name) {
            msg_error(&format!(
                "distribution-specific setup for '{}' failed (continuing): {}",
                distro_name, e
            ));
        }
    }

    Ok(())
}

fn cleanup_on_failure(rootfs: &str, distro_name: &str) {
    if fs::symlink_metadata(rootfs).is_ok() {
        force_remove_dir_all(Path::new(rootfs));
    }

    let override_path = format!("{}/{}.override.sh", get_plugins_dir(), distro_name);
    if Path::new(&override_path).exists() {
        let _ = fs::remove_file(&override_path);
    }
}

fn render_override_plugin(plugin: &DistroPlugin, alias: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("DISTRO_NAME=\"{} - {}\"", plugin.name, alias));
    if let Some(comment) = &plugin.comment {
        lines.push(format!("DISTRO_COMMENT=\"{}\"", comment));
    }
    let mut arches: Vec<&String> = plugin.tarballs.keys().collect();
    arches.sort();
    for arch in arches {
        if let Some(tb) = plugin.tarballs.get(arch) {
            lines.push(format!("TARBALL_URL_{}=\"{}\"", arch, tb.url));
            lines.push(format!("TARBALL_SHA256_{}=\"{}\"", arch, tb.sha256));
        }
    }
    lines.join("\n") + "\n"
}

fn plugins_for_install(
    plugins_dir: &str,
    distro_name: &str,
    tarball_override: bool,
) -> Vec<DistroPlugin> {
    let mut plugins = load_plugins(Path::new(plugins_dir));
    if tarball_override && !plugins.iter().any(|p| p.alias == distro_name) {
        if let Some(entry) = crate::catalog::catalog_entry(distro_name) {
            plugins.push(DistroPlugin {
                alias: entry.alias.to_string(),
                name: entry.name.to_string(),
                comment: None,
                tarballs: std::collections::HashMap::new(),
                has_setup: false,
            });
        }
    }
    plugins
}

fn oci_install_reference(
    distro_name: &str,
    plugins: &[DistroPlugin],
) -> Option<(String, Option<String>)> {
    if plugins.iter().any(|plugin| plugin.alias == distro_name) {
        return None;
    }
    if let Some(entry) = crate::catalog::catalog_entry(distro_name) {
        return Some((entry.reference.to_string(), Some(entry.alias.to_string())));
    }
    InstallSourceInput::classify(distro_name)
        .filter(|source| source.kind() == InstallSourceInputKind::OciImageReference)
        .map(|source| (source.source_text().into_owned(), None))
}

pub fn command_install(
    distro_name: &str,
    override_alias: Option<&str>,
    override_tarball_url: Option<&str>,
    override_tarball_sha256: Option<&str>,
) -> Result<(), String> {
    let requested_distro = distro_name.to_string();
    let plugins_dir = get_plugins_dir();
    let tarball_override = override_tarball_url.is_some();
    if let Some((reference, default_alias)) = oci_install_reference(
        distro_name,
        &plugins_for_install(&plugins_dir, distro_name, tarball_override),
    ) {
        if override_tarball_url.is_some() || override_tarball_sha256.is_some() {
            return Err(
                "--override-tarball-url and --override-tarball-sha256 are not supported for OCI image installs"
                    .to_string(),
            );
        }
        if let Some(alias) = override_alias {
            validate_alias_format(alias)?;
        }
        return install_from_oci_reference(&reference, override_alias.or(default_alias.as_deref()));
    }
    if !is_valid_container_name(distro_name) {
        msg_error(&format!(
            "'{}' is not a valid distribution name: use alphanumerics and '_.+-'.",
            distro_name
        ));
        return Err("invalid distribution name".to_string());
    }

    let installed_rootfs_dir = get_installed_rootfs_dir();
    let download_cache_dir = get_download_cache_dir();

    // Validate alias format if override-alias given
    let mut override_plugin: Option<(String, String, DistroPlugin)> = None;
    let distro_name = if let Some(alias) = override_alias {
        validate_alias_format(alias)?;

        let override_alt = format!("{}/{}.override.sh", plugins_dir, alias);
        let plugins =
            plugins_for_install(&plugins_dir, requested_distro.as_str(), tarball_override);
        let retrying = pending_install(Path::new(&installed_rootfs_dir), alias)
            == Some(PendingInstall::Interrupted)
            && !Path::new(&format!("{}/{}.sh", plugins_dir, alias)).exists();
        if !retrying
            && (plugins.iter().any(|p| p.alias == alias) || Path::new(&override_alt).exists())
        {
            return Err(format!(
                "distribution with alias '{}' already exists",
                alias
            ));
        }

        let orig_plugin = plugins
            .iter()
            .find(|p| p.alias == distro_name)
            .ok_or_else(|| format!("unknown distribution '{}'", distro_name))?;

        override_plugin = Some((
            override_alt,
            render_override_plugin(orig_plugin, alias),
            DistroPlugin {
                alias: alias.to_string(),
                name: format!("{} - {}", orig_plugin.name, alias),
                comment: orig_plugin.comment.clone(),
                tarballs: orig_plugin.tarballs.clone(),
                has_setup: false,
            },
        ));

        alias.to_string()
    } else {
        distro_name.to_string()
    };

    let mut plugins = plugins_for_install(&plugins_dir, &distro_name, tarball_override);
    if let Some((_, _, plugin)) = &override_plugin {
        plugins.retain(|p| p.alias != plugin.alias);
        plugins.push(plugin.clone());
    }
    if !plugins.iter().any(|p| p.alias == distro_name) {
        println!();
        msg_error(&format!(
            "unknown distribution '{}' was requested to be installed.",
            distro_name
        ));
        println!();
        println!(
            "{}View supported distributions by: {}pr-cli list{}",
            CYAN, GREEN, RESET
        );
        println!();
        return Err(format!("unknown distribution '{}'", distro_name));
    };

    // Check not already installed
    let rootfs = format!("{}/{}", installed_rootfs_dir, distro_name);
    if resolve_installed_rootfs(&distro_name).is_some() {
        println!();
        msg_error(&format!(
            "distribution '{}' is already installed.",
            distro_name
        ));
        println!();
        println!(
            "{}Log in:     {}pr-cli login {}{}",
            CYAN, GREEN, distro_name, RESET
        );
        println!(
            "{}Reinstall:  {}pr-cli reset {}{}",
            CYAN, GREEN, distro_name, RESET
        );
        println!(
            "{}Uninstall:  {}pr-cli remove {}{}",
            CYAN, GREEN, distro_name, RESET
        );
        println!();
        return Err("already installed".to_string());
    }

    // Parse plugin
    let plugin = plugins
        .iter()
        .find(|p| p.alias == distro_name)
        .ok_or_else(|| format!("failed to parse plugin for '{}'", distro_name))?;

    refuse_unfinished_install_in(&get_oci_containers_dir(), &distro_name)?;

    // Detect device architecture
    let device_arch = detect_device_arch();

    msg_status(&format!("Installing {}...", plugin.name));

    let populate = || {
        if let Some((path, content, plugin)) = &override_plugin {
            msg_status(&format!("Creating file '{}.override.sh'...", plugin.alias));
            fs::create_dir_all(&plugins_dir).map_err(|e| format!("create plugins dir: {}", e))?;
            fs::write(path, content).map_err(|e| format!("write override plugin: {}", e))?;
        }

        // Create rootfs directory
        fs::create_dir_all(&rootfs).map_err(|e| format!("create rootfs dir: {}", e))?;
        msg_status(&format!("Creating directory '{}'...", rootfs));

        // Create .l2s directory
        let l2s_dir = format!("{}/.l2s", rootfs);
        fs::create_dir_all(&l2s_dir).map_err(|e| format!("create .l2s dir: {}", e))?;

        // Determine tarball URL and SHA256
        let arch = device_arch.as_str();
        let tarball = plugin.tarballs.get(arch);
        if tarball.is_none() && override_tarball_url.is_none() {
            return Err(format!(
                "distribution download URL is not defined for CPU architecture '{}'",
                arch
            ));
        }

        let tarball_url = override_tarball_url
            .map(str::to_string)
            .or_else(|| tarball.map(|t| t.url.clone()))
            .unwrap_or_default();
        let tarball_sha256 = override_tarball_sha256
            .map(str::to_string)
            .or_else(|| tarball.map(|t| t.sha256.clone()))
            .unwrap_or_default();

        if tarball_url.is_empty() {
            msg_error(&format!(
                "distribution download URL is not defined for CPU architecture '{}'",
                arch
            ));
            return Err("no tarball URL".to_string());
        }

        // Download
        let archive_name = tarball_url
            .rsplit('/')
            .next()
            .unwrap_or("rootfs.tar.xz")
            .to_string();
        fs::create_dir_all(&download_cache_dir).map_err(|e| format!("create cache dir: {}", e))?;
        let archive_path = format!("{}/{}", download_cache_dir, archive_name);

        if Path::new(&archive_path).exists() {
            msg_status("Using cached rootfs archive...");
        } else {
            msg_status("Downloading rootfs archive...");
            msg_status(&format!("URL: {}", tarball_url));
            println!();
            if let Err(e) = download_file(&tarball_url, &archive_path, 3) {
                println!();
                msg_error("Download failure, please check your network connection.");
                return Err(format!("download failed: {}", e));
            }
            println!();
        }

        // SHA256 verification
        if !tarball_sha256.is_empty() {
            msg_status("Checking integrity, please wait...");
            if let Err(e) = verify_sha256(&tarball_sha256, &archive_path) {
                msg_error("Integrity checking failed. Try to redo installation again.");
                let _ = fs::remove_file(&archive_path);
                return Err(format!("sha256 verification failed: {}", e));
            }
        } else if override_tarball_url.is_some() {
            msg_error("Integrity checking of downloaded rootfs has been disabled.");
        }

        // Extract
        msg_status("Extracting rootfs, please wait...");

        if let Err(e) = extract_tarball(&archive_path, &rootfs, 1, &["dev"]) {
            msg_error(&format!("Extracting the rootfs failed: {}", e));
            let _ = fs::remove_file(&archive_path);
            return Err(e);
        }

        // Validate rootfs structure
        if !Path::new(&format!("{}/etc", rootfs)).exists() {
            println!();
            msg_error(&format!(
                "rootfs of distribution '{}' has unexpected structure (no /etc directory).",
                distro_name
            ));
            return Err("no /etc in rootfs".to_string());
        }

        // Write config files
        write_config_files(&rootfs, Some(&requested_distro))
    };
    install_container(
        Path::new(&installed_rootfs_dir),
        &distro_name,
        Path::new(&rootfs),
        || Path::new(&rootfs).is_dir(),
        populate,
        || cleanup_on_failure(&rootfs, &distro_name),
    )?;

    msg_status("Finished.");
    println!();
    println!(
        "{}Log in with: {}pr-cli login {}{}",
        CYAN, GREEN, distro_name, RESET
    );
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_marker::BEING_INSTALLED;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static std::sync::Mutex<()> {
        crate::shared::global_test_env_lock()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos))
    }

    enum RawEntry<'a> {
        Dir(&'a str),
        DirMode(&'a str, u32),
        File(&'a str, &'a [u8], u32),
        Symlink(&'a str, &'a str),
        Hardlink(&'a str, &'a str),
    }

    fn write_raw_xz_tar(path: &Path, entries: &[RawEntry]) {
        let file = fs::File::create(path).expect("create archive");
        let encoder = xz2::write::XzEncoder::new(file, 1);
        let mut builder = tar::Builder::new(encoder);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            let (name, link, kind, data, mode): (&str, Option<&str>, _, &[u8], u32) = match entry {
                RawEntry::Dir(n) => (n, None, tar::EntryType::Directory, &[], 0o755),
                RawEntry::DirMode(n, m) => (n, None, tar::EntryType::Directory, &[], *m),
                RawEntry::File(n, d, m) => (n, None, tar::EntryType::Regular, d, *m),
                RawEntry::Symlink(n, t) => (n, Some(t), tar::EntryType::Symlink, &[], 0o777),
                RawEntry::Hardlink(n, t) => (n, Some(t), tar::EntryType::Link, &[], 0o644),
            };
            let gnu = header.as_gnu_mut().expect("gnu header");
            gnu.name[..name.len()].copy_from_slice(name.as_bytes());
            if let Some(link) = link {
                gnu.linkname[..link.len()].copy_from_slice(link.as_bytes());
            }
            header.set_entry_type(kind);
            header.set_mode(mode);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).expect("append entry");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish xz");
    }

    #[test]
    fn extract_tarball_unpacks_a_rootfs_below_its_top_directory() {
        let tmp = unique_temp_dir("pr-cli-extract-normal");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        let archive = tmp.join("rootfs.tar.xz");
        write_raw_xz_tar(
            &archive,
            &[
                RawEntry::Dir("top/"),
                RawEntry::Dir("top/etc/"),
                RawEntry::File("top/etc/os-release", b"ID=test\n", 0o644),
                RawEntry::File("top/bin/sh", b"\x7fELFsh", 0o755),
                RawEntry::Hardlink("top/bin/ash", "top/bin/sh"),
                RawEntry::Symlink("top/sbin", "bin"),
                RawEntry::File("top/dev/null", b"", 0o644),
            ],
        );
        let rootfs = tmp.join("rootfs");

        extract_tarball(
            archive.to_str().unwrap(),
            rootfs.to_str().unwrap(),
            1,
            &["dev"],
        )
        .expect("extract");

        assert_eq!(
            fs::read(rootfs.join("etc/os-release")).unwrap(),
            b"ID=test\n"
        );
        assert_eq!(fs::read(rootfs.join("bin/ash")).unwrap(), b"\x7fELFsh");
        assert_eq!(
            fs::read_link(rootfs.join("sbin")).unwrap(),
            Path::new("bin")
        );
        let mode = fs::metadata(rootfs.join("bin/sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(!rootfs.join("dev").exists(), "dev/ is excluded");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_tarball_rejects_an_entry_that_climbs_out_of_the_rootfs() {
        let tmp = unique_temp_dir("pr-cli-extract-dotdot");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        let archive = tmp.join("rootfs.tar.xz");
        write_raw_xz_tar(&archive, &[RawEntry::File("top/../escape", b"x", 0o644)]);
        let rootfs = tmp.join("rootfs");

        let result = extract_tarball(archive.to_str().unwrap(), rootfs.to_str().unwrap(), 1, &[]);

        assert!(result.is_err());
        assert!(!tmp.join("escape").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_tarball_does_not_write_through_a_symlink_it_created() {
        let tmp = unique_temp_dir("pr-cli-extract-symlink");
        let outside = tmp.join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        let archive = tmp.join("rootfs.tar.xz");
        let outside_str = outside.to_str().unwrap().to_string();
        write_raw_xz_tar(
            &archive,
            &[
                RawEntry::Symlink("top/foo", &outside_str),
                RawEntry::File("top/foo/planted", b"x", 0o644),
            ],
        );
        let rootfs = tmp.join("rootfs");

        extract_tarball(archive.to_str().unwrap(), rootfs.to_str().unwrap(), 1, &[])
            .expect("extract");

        assert!(!outside.join("planted").exists());
        let rerooted = rootfs
            .join(outside.strip_prefix("/").unwrap())
            .join("planted");
        assert!(rerooted.is_file());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_tarball_writes_into_a_read_only_directory_it_creates() {
        let tmp = unique_temp_dir("pr-cli-extract-read-only-dir");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        let archive = tmp.join("rootfs.tar.xz");
        write_raw_xz_tar(
            &archive,
            &[
                RawEntry::DirMode("top/ro/", 0o555),
                RawEntry::File("top/ro/f", b"ok", 0o644),
            ],
        );
        let rootfs = tmp.join("rootfs");

        extract_tarball(archive.to_str().unwrap(), rootfs.to_str().unwrap(), 1, &[])
            .expect("extract");

        assert_eq!(fs::read(rootfs.join("ro/f")).unwrap(), b"ok");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_tarball_writes_below_an_absolute_symlink_inside_the_rootfs() {
        let tmp = unique_temp_dir("pr-cli-extract-absolute-symlink");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        let archive = tmp.join("rootfs.tar.xz");
        write_raw_xz_tar(
            &archive,
            &[
                RawEntry::Dir("top/run/"),
                RawEntry::Dir("top/var/"),
                RawEntry::Symlink("top/var/run", "/run"),
                RawEntry::File("top/var/run/lock", b"x", 0o644),
                RawEntry::Hardlink("top/var/run/lock2", "top/var/run/lock"),
            ],
        );
        let rootfs = tmp.join("rootfs");

        extract_tarball(archive.to_str().unwrap(), rootfs.to_str().unwrap(), 1, &[])
            .expect("extract");

        assert_eq!(fs::read(rootfs.join("run/lock")).unwrap(), b"x");
        assert_eq!(fs::read(rootfs.join("run/lock2")).unwrap(), b"x");
        assert!(fs::symlink_metadata(rootfs.join("var/run"))
            .unwrap()
            .is_symlink());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn extract_tarball_rejects_a_hardlink_to_a_file_outside_the_rootfs() {
        let tmp = unique_temp_dir("pr-cli-extract-hardlink");
        fs::create_dir_all(&tmp).expect("mkdir tmp");
        fs::write(tmp.join("secret"), b"secret").expect("seed secret");
        let archive = tmp.join("rootfs.tar.xz");
        write_raw_xz_tar(
            &archive,
            &[RawEntry::Hardlink("top/stolen", "top/../secret")],
        );
        let rootfs = tmp.join("rootfs");

        let result = extract_tarball(archive.to_str().unwrap(), rootfs.to_str().unwrap(), 1, &[]);

        assert!(result.is_err());
        assert!(!rootfs.join("stolen").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolves_short_reference_to_docker_hub_library() {
        let resolved = resolve_oci_reference("alpine:latest").expect("resolve reference");
        assert_eq!(resolved.registry_base, "https://registry-1.docker.io");
        assert_eq!(resolved.repository, "library/alpine");
        assert_eq!(resolved.reference, "latest");
    }

    #[test]
    fn resolves_registry_qualified_reference() {
        let resolved =
            resolve_oci_reference("ghcr.io/example/app:1.2.3").expect("resolve reference");
        assert_eq!(resolved.registry_base, "https://ghcr.io");
        assert_eq!(resolved.repository, "example/app");
        assert_eq!(resolved.reference, "1.2.3");
    }

    #[test]
    fn resolves_digest_reference() {
        let resolved = resolve_oci_reference(
            "docker.io/library/debian@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("resolve digest reference");
        assert_eq!(resolved.registry_base, "https://registry-1.docker.io");
        assert_eq!(resolved.repository, "library/debian");
        assert_eq!(
            resolved.reference,
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(resolved.digest_reference);
    }

    #[test]
    fn resolves_explicit_docker_hub_single_repo_to_library_namespace() {
        let resolved = resolve_oci_reference("docker.io/alpine:latest").expect("resolve reference");
        assert_eq!(resolved.registry_base, "https://registry-1.docker.io");
        assert_eq!(resolved.repository, "library/alpine");
        assert_eq!(resolved.reference, "latest");
        assert!(!resolved.digest_reference);
    }

    #[test]
    fn defaults_install_name_from_repository_leaf() {
        let resolved = ResolvedOciReference {
            registry_base: "https://registry-1.docker.io".to_string(),
            repository: "library/ubuntu".to_string(),
            reference: "24.04".to_string(),
            digest_reference: false,
        };
        assert_eq!(default_install_name_for_oci(&resolved), "ubuntu");
    }

    #[test]
    fn normalizes_oci_reference_with_tag_and_digest() {
        let tag_ref = ResolvedOciReference {
            registry_base: "https://registry-1.docker.io".to_string(),
            repository: "library/debian".to_string(),
            reference: "stable".to_string(),
            digest_reference: false,
        };
        assert_eq!(
            normalized_oci_reference(&tag_ref),
            "registry-1.docker.io/library/debian:stable"
        );

        let digest_ref = ResolvedOciReference {
            registry_base: "https://ghcr.io".to_string(),
            repository: "example/app".to_string(),
            reference: "sha256:abc".to_string(),
            digest_reference: true,
        };
        assert_eq!(
            normalized_oci_reference(&digest_ref),
            "ghcr.io/example/app@sha256:abc"
        );
    }

    #[test]
    fn rejects_alias_with_path_separators_or_traversal() {
        assert!(validate_alias_format("../escape").is_err());
        assert!(validate_alias_format("name/with/slash").is_err());
        assert!(validate_alias_format("name\\with\\slash").is_err());
    }

    #[test]
    fn detects_rust_owned_setup_profiles() {
        assert!(has_rust_owned_distro_setup("debian"));
        assert!(has_rust_owned_distro_setup("ubuntu"));
        assert!(!has_rust_owned_distro_setup("alpine"));
    }

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn guest_setup_runs_the_program_without_a_shell() {
        let rootfs = unique_temp_dir("pr-cli-guest-argv");
        let rootfs = rootfs.to_str().expect("utf-8 path");

        let args = args_of(&guest_command(
            rootfs,
            &["dpkg-reconfigure", "locales"],
            &[],
        ));

        assert_eq!(
            args[args.len() - 4..],
            [
                format!("--rootfs={}", rootfs),
                "--cwd=/".to_string(),
                "dpkg-reconfigure".to_string(),
                "locales".to_string()
            ]
        );
        assert!(!args.iter().any(|a| a == "/bin/sh" || a == "-c"));
    }

    #[test]
    fn guest_setup_runs_with_the_container_mounts_like_login() {
        let rootfs = unique_temp_dir("pr-cli-guest-binds");
        let rootfs = rootfs.to_str().expect("utf-8 path");

        let args = args_of(&guest_command(rootfs, &["locale-gen"], &[]));

        for expected in [
            "--bind=/dev",
            "--bind=/proc",
            "--bind=/sys",
            "--kill-on-exit",
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {}", expected);
        }
        assert!(args.iter().any(|a| a.ends_with(":/tmp")));
    }

    #[test]
    fn guest_setup_uses_the_guest_environment() {
        let rootfs = unique_temp_dir("pr-cli-guest-env");
        let cmd = guest_command(rootfs.to_str().expect("utf-8 path"), &["locale-gen"], &[]);

        assert_eq!(
            env_of(&cmd, "PATH").as_deref(),
            Some(crate::shared::DEFAULT_PATH_ENV_SUFFIX)
        );
        assert_eq!(env_of(&cmd, "HOME").as_deref(), Some("/root"));
        assert_eq!(env_of(&cmd, "TMPDIR").as_deref(), Some("/tmp"));
        assert_eq!(env_of(&cmd, "LANG").as_deref(), Some("C.UTF-8"));
    }

    #[test]
    fn guest_setup_passes_the_extra_environment() {
        let rootfs = unique_temp_dir("pr-cli-guest-extra");
        let cmd = guest_command(
            rootfs.to_str().expect("utf-8 path"),
            &["dpkg-reconfigure", "locales"],
            &[("DEBIAN_FRONTEND", "noninteractive")],
        );

        assert_eq!(
            env_of(&cmd, "DEBIAN_FRONTEND").as_deref(),
            Some("noninteractive")
        );
    }

    #[test]
    fn uncomments_en_us_locale_entry() {
        let tmp_dir = unique_temp_dir("pr-cli-locale");
        fs::create_dir_all(tmp_dir.join("etc")).expect("create etc");
        let locale_gen = tmp_dir.join("etc/locale.gen");
        fs::write(&locale_gen, "# en_US.UTF-8 UTF-8\n# de_DE.UTF-8 UTF-8\n")
            .expect("write locale.gen");

        uncomment_en_us_locale(tmp_dir.to_str().expect("tmp path")).expect("uncomment locale");
        let updated = fs::read_to_string(&locale_gen).expect("read locale.gen");
        assert!(updated.contains("en_US.UTF-8 UTF-8"));
        assert!(updated.contains("# de_DE.UTF-8 UTF-8"));

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn derives_safe_default_install_name_when_repository_leaf_is_invalid() {
        let resolved = ResolvedOciReference {
            registry_base: "https://registry-1.docker.io".to_string(),
            repository: "library/..".to_string(),
            reference: "latest".to_string(),
            digest_reference: false,
        };

        let name = derive_oci_install_name(&resolved, None).expect("derive install name");
        assert_eq!(name, "container");
    }

    #[test]
    fn parses_bearer_challenge_fields() {
        let challenge = parse_bearer_challenge(
            r#"Bearer realm="https://auth.example/token",service="registry.example",scope="repository:library/alpine:pull""#,
        )
        .expect("parse challenge");
        assert_eq!(challenge.realm, "https://auth.example/token");
        assert_eq!(challenge.service.as_deref(), Some("registry.example"));
        assert_eq!(
            challenge.scope.as_deref(),
            Some("repository:library/alpine:pull")
        );
    }

    #[test]
    fn rejects_non_bearer_challenge() {
        assert!(parse_bearer_challenge(r#"Basic realm="example""#).is_none());
    }

    #[test]
    fn sanitize_install_name_keeps_allowed_chars_and_lowercases() {
        assert_eq!(sanitize_install_name("My.Image+Test"), "my.image+test");
        assert_eq!(sanitize_install_name("___"), "___");
    }

    #[test]
    fn verify_sha256_matches_file_content() {
        let tmp_dir = unique_temp_dir("pr-cli-sha256");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let file_path = tmp_dir.join("payload.txt");
        fs::write(&file_path, b"hello coverage").expect("write payload");
        let digest = format!("{:x}", Sha256::digest(b"hello coverage"));

        verify_sha256(&digest, file_path.to_str().expect("path")).expect("sha matches");
        assert!(verify_sha256(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            file_path.to_str().expect("path")
        )
        .is_err());

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn setup_fake_sysdata_writes_expected_files() {
        let tmp_dir = unique_temp_dir("pr-cli-fake-sysdata");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        setup_fake_sysdata(tmp_dir.to_str().expect("path")).expect("setup fake sysdata");

        assert!(tmp_dir.join("proc/.loadavg").is_file());
        assert!(tmp_dir.join("proc/.stat").is_file());
        assert!(tmp_dir.join("proc/.uptime").is_file());
        assert!(tmp_dir.join("proc/.version").is_file());
        assert!(tmp_dir.join("proc/.vmstat").is_file());
        assert!(tmp_dir.join("sys/.empty").is_dir());

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn write_config_files_populates_rootfs_basics_without_distro_hook() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        fs::write(rootfs.join("etc/environment"), "").expect("seed environment");
        fs::write(rootfs.join("etc/hosts"), "").expect("seed hosts");
        fs::write(rootfs.join("etc/passwd"), "").expect("seed passwd");
        fs::write(rootfs.join("etc/shadow"), "").expect("seed shadow");
        fs::write(rootfs.join("etc/group"), "").expect("seed group");
        fs::write(rootfs.join("etc/gshadow"), "").expect("seed gshadow");

        write_config_files(rootfs.to_str().expect("path"), None).expect("write config");

        let resolv = fs::read_to_string(rootfs.join("etc/resolv.conf")).expect("read resolv");
        assert!(resolv.contains("nameserver 8.8.8.8"));
        assert!(resolv.contains("nameserver 8.8.4.4"));

        let env = fs::read_to_string(rootfs.join("etc/environment")).expect("read env");
        assert!(env.contains("LANG=en_US.UTF-8"));
        assert!(env.contains("MOZ_FAKE_NO_SANDBOX=1"));

        assert!(rootfs.join("proc/.version").is_file());
        assert!(rootfs.join("sys/.empty").is_dir());

        let _ = fs::remove_dir_all(tmp_dir);
    }

    fn plugin_with_alias(alias: &str) -> DistroPlugin {
        DistroPlugin {
            alias: alias.to_string(),
            name: alias.to_string(),
            comment: None,
            tarballs: std::collections::HashMap::new(),
            has_setup: false,
        }
    }

    #[test]
    fn a_plugin_alias_wins_over_oci_classification() {
        let plugins = [plugin_with_alias("debian.old")];
        assert_eq!(oci_install_reference("debian.old", &plugins), None);
    }

    #[test]
    fn a_name_without_a_plugin_is_still_an_oci_reference() {
        let plugins = [plugin_with_alias("debian")];
        assert_eq!(
            oci_install_reference("alpine:3.20", &plugins),
            Some(("alpine:3.20".to_string(), None))
        );
        assert_eq!(oci_install_reference("debian", &plugins), None);
    }

    #[test]
    fn a_catalog_name_installs_the_app_image_under_that_name() {
        for (name, reference) in [
            ("debian", "docker.io/library/debian:stable"),
            ("manjaro", "docker.io/manjarolinux/base:latest"),
            ("fedora", "registry.fedoraproject.org/fedora:latest"),
            ("alpine-edge", "docker.io/library/alpine:edge"),
        ] {
            assert_eq!(
                oci_install_reference(name, &[]),
                Some((reference.to_string(), Some(name.to_string()))),
                "{}",
                name
            );
        }
    }

    #[test]
    fn detect_device_arch_reads_the_running_binary() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("DISTRO_ARCH");
        let expected = match std::env::consts::ARCH {
            "x86" => "i686",
            other => other,
        };
        assert_eq!(detect_device_arch(), expected);
    }

    #[test]
    fn write_config_files_keeps_the_app_bin_out_of_the_container_path() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config-path");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");

        write_config_files(rootfs.to_str().expect("path"), None).expect("write config");

        let env = fs::read_to_string(rootfs.join("etc/environment")).expect("read env");
        let path = env
            .lines()
            .find_map(|l| l.strip_prefix("PATH="))
            .expect("PATH line");
        assert_eq!(path, crate::shared::DEFAULT_PATH_ENV_SUFFIX);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn write_config_files_replaces_symlinked_files_instead_of_following_them() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config-symlinks");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        for name in [
            "passwd",
            "shadow",
            "group",
            "gshadow",
            "hosts",
            "environment",
        ] {
            fs::write(outside.join(name), "host-data\n").expect("seed host file");
            std::os::unix::fs::symlink(outside.join(name), rootfs.join("etc").join(name))
                .expect("symlink etc file");
        }

        write_config_files(rootfs.to_str().expect("path"), None).expect("write config");

        for name in [
            "passwd",
            "shadow",
            "group",
            "gshadow",
            "hosts",
            "environment",
        ] {
            assert_eq!(
                fs::read_to_string(outside.join(name)).expect("read host file"),
                "host-data\n",
                "{} outside the rootfs changed",
                name
            );
            let inside = rootfs.join("etc").join(name);
            assert!(fs::symlink_metadata(&inside).expect("stat").is_file());
            assert!(!fs::read_to_string(&inside).unwrap().contains("host-data"));
        }
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn write_config_files_keeps_an_absolute_etc_symlink_inside_the_rootfs() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config-etc-symlink");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        let rootfs = tmp_dir.join("rootfs");
        let rerooted = rootfs.join(outside.strip_prefix("/").unwrap());
        fs::create_dir_all(&rerooted).expect("create re-rooted etc");
        std::os::unix::fs::symlink(&outside, rootfs.join("etc")).expect("symlink etc");

        write_config_files(rootfs.to_str().expect("path"), None).expect("write config");
        assert_eq!(fs::read_dir(&outside).expect("read outside").count(), 0);
        assert!(rerooted.join("hosts").is_file());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn setup_fake_sysdata_keeps_an_absolute_proc_symlink_inside_the_rootfs() {
        let tmp_dir = unique_temp_dir("pr-cli-fake-sysdata-symlink");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&rootfs).expect("create rootfs");
        std::os::unix::fs::symlink(&outside, rootfs.join("proc")).expect("symlink proc");

        setup_fake_sysdata(rootfs.to_str().expect("path")).expect("setup");
        assert_eq!(fs::read_dir(&outside).expect("read outside").count(), 0);
        let rerooted = rootfs.join(outside.strip_prefix("/").unwrap());
        assert!(rerooted.join(".loadavg").is_file());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn write_config_files_keeps_the_entries_of_an_unreadable_shadow() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config-shadow-0000");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        for (name, line) in [
            ("passwd", "root:x:0:0:root:/root:/bin/sh\n"),
            ("group", "root:x:0:\n"),
            ("shadow", "root:*:19000:0:99999:7:::\n"),
            ("gshadow", "root:::\n"),
        ] {
            let path = rootfs.join("etc").join(name);
            fs::write(&path, line).expect("seed");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
        }

        write_config_files(rootfs.to_str().expect("path"), None).expect("write config");

        let shadow = fs::read_to_string(rootfs.join("etc/shadow")).expect("read shadow");
        assert!(shadow.starts_with("root:*:19000:"), "{}", shadow);
        let gshadow = fs::read_to_string(rootfs.join("etc/gshadow")).expect("read gshadow");
        assert!(gshadow.starts_with("root:::"), "{}", gshadow);
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn append_line_follows_an_absolute_symlink_inside_the_rootfs() {
        let tmp_dir = unique_temp_dir("pr-cli-append-absolute-symlink");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc/pam.d")).expect("create pam.d");
        fs::create_dir_all(rootfs.join("etc/authselect")).expect("create authselect");
        fs::write(
            rootfs.join("etc/authselect/system-auth"),
            "auth required x\n",
        )
        .expect("seed");
        std::os::unix::fs::symlink(
            "/etc/authselect/system-auth",
            rootfs.join("etc/pam.d/system-auth"),
        )
        .expect("symlink");

        append_line_if_missing(&rootfs, "etc/pam.d/system-auth", "session y").expect("append");

        assert_eq!(
            fs::read_to_string(rootfs.join("etc/authselect/system-auth")).unwrap(),
            "auth required x\nsession y\n"
        );
        assert!(fs::symlink_metadata(rootfs.join("etc/pam.d/system-auth"))
            .unwrap()
            .is_symlink());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_install_refuses_a_plugin_alias_that_is_not_a_valid_name() {
        let plugin =
            "DISTRO_NAME=\"Test\"\nTARBALL_URL_aarch64=\"https://example.invalid/a.tar.xz\"\n";
        let _guard = env_lock().lock().expect("lock env");
        let prefix = unique_temp_dir("pr-cli-install-bad-alias").join("usr");
        fs::create_dir_all(prefix.join("etc/pr")).expect("create plugins dir");
        fs::write(prefix.join("etc/pr/x@y.sh"), plugin).expect("write plugin");
        let existing = prefix.join("var/lib/pr/installed-rootfs/x@y");
        fs::create_dir_all(&existing).expect("create existing rootfs");
        fs::write(existing.join("keep"), b"data").expect("seed existing");
        std::env::set_var("APP_PREFIX", &prefix);
        std::env::set_var("DISTRO_ARCH", "x86_64");

        let result = command_install("x@y", None, None, None);

        std::env::remove_var("APP_PREFIX");
        std::env::remove_var("DISTRO_ARCH");
        assert!(result.is_err());
        assert!(existing.join("keep").exists());
        let _ = fs::remove_dir_all(prefix.parent().expect("base"));
    }

    #[test]
    fn distro_setup_edits_do_not_follow_symlinks_out_of_the_rootfs() {
        let tmp_dir = unique_temp_dir("pr-cli-distro-setup-symlink");
        let outside = tmp_dir.join("outside");
        fs::create_dir_all(&outside).expect("create outside");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc/pam.d")).expect("create pam.d");
        fs::write(outside.join("locale.gen"), "# en_US.UTF-8 UTF-8\n").expect("seed");
        fs::write(outside.join("su"), "host\n").expect("seed");
        std::os::unix::fs::symlink(outside.join("locale.gen"), rootfs.join("etc/locale.gen"))
            .expect("symlink locale.gen");
        std::os::unix::fs::symlink(outside.join("su"), rootfs.join("etc/pam.d/su"))
            .expect("symlink su");
        let rootfs_str = rootfs.to_str().expect("path");

        let _ = uncomment_en_us_locale(rootfs_str);
        let _ = append_line_if_missing(&rootfs, "etc/pam.d/su", "session required x");

        assert_eq!(
            fs::read_to_string(outside.join("locale.gen")).unwrap(),
            "# en_US.UTF-8 UTF-8\n"
        );
        assert_eq!(fs::read_to_string(outside.join("su")).unwrap(), "host\n");
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn validate_alias_format_accepts_and_rejects_expected_values() {
        assert!(validate_alias_format("debian").is_ok());
        assert!(validate_alias_format("Ubuntu_24.04+custom").is_ok());

        assert!(validate_alias_format("").is_err());
        assert!(validate_alias_format(".hidden").is_err());
        assert!(validate_alias_format("bad/name").is_err());
        assert!(validate_alias_format("bad\\name").is_err());
        assert!(validate_alias_format("bad..name").is_err());
        assert!(validate_alias_format("name.sh").is_err());
        assert!(validate_alias_format("name with space").is_err());
    }

    #[test]
    fn resolve_oci_reference_rejects_invalid_inputs() {
        assert!(resolve_oci_reference("").is_err());
        assert!(resolve_oci_reference("   ").is_err());
        assert!(resolve_oci_reference("@sha256:abc").is_err());
        assert!(resolve_oci_reference("repo@").is_err());
        assert!(resolve_oci_reference("repo:").is_err());
    }

    #[test]
    fn resolves_index_docker_io_host_to_registry_1() {
        let resolved =
            resolve_oci_reference("index.docker.io/library/alpine:3.20").expect("resolve");
        assert_eq!(resolved.registry_base, "https://registry-1.docker.io");
        assert_eq!(resolved.repository, "library/alpine");
        assert_eq!(resolved.reference, "3.20");
    }

    #[test]
    fn derive_oci_install_name_uses_valid_override_alias() {
        let resolved = ResolvedOciReference {
            registry_base: "https://ghcr.io".to_string(),
            repository: "example/app".to_string(),
            reference: "latest".to_string(),
            digest_reference: false,
        };
        let alias =
            derive_oci_install_name(&resolved, Some("custom.alias-1")).expect("derive alias");
        assert_eq!(alias, "custom.alias-1");
    }

    #[test]
    fn append_line_if_missing_is_idempotent_and_preserves_newlines() {
        let tmp_dir = unique_temp_dir("pr-cli-append-line");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let file_path = tmp_dir.join("config.txt");
        fs::write(&file_path, "first=line").expect("seed file");

        append_line_if_missing(&tmp_dir, "config.txt", "session required pam_env.so")
            .expect("append first time");
        append_line_if_missing(&tmp_dir, "config.txt", "session required pam_env.so")
            .expect("append second time");

        let content = fs::read_to_string(&file_path).expect("read file");
        let expected = "first=line\nsession required pam_env.so\n";
        assert_eq!(content, expected);

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn fix_elf_execute_bits_sets_exec_for_elf_files_only() {
        let tmp_dir = unique_temp_dir("pr-cli-fix-elf");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let elf_path = tmp_dir.join("tool");
        let txt_path = tmp_dir.join("note.txt");

        fs::write(&elf_path, b"\x7fELF\x02\x01\x01\x00dummy").expect("write elf");
        fs::write(&txt_path, b"plain text").expect("write txt");
        fs::set_permissions(&elf_path, fs::Permissions::from_mode(0o644)).expect("chmod elf");
        fs::set_permissions(&txt_path, fs::Permissions::from_mode(0o644)).expect("chmod txt");

        fix_elf_execute_bits(tmp_dir.to_str().expect("tmp path"));

        let elf_mode = fs::metadata(&elf_path)
            .expect("elf metadata")
            .permissions()
            .mode();
        let txt_mode = fs::metadata(&txt_path)
            .expect("txt metadata")
            .permissions()
            .mode();
        assert_ne!(elf_mode & 0o111, 0);
        assert_eq!(txt_mode & 0o111, 0);

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn cleanup_oci_on_failure_ignores_unsafe_paths() {
        let _guard = env_lock().lock().expect("lock env");
        let _prev_prefix = std::env::var("APP_PREFIX").ok();
        let tmp_dir = unique_temp_dir("pr-cli-cleanup-oci");
        let prefix = tmp_dir.join("usr");
        let containers_root = prefix.join("var/lib/pr/containers");
        fs::create_dir_all(&containers_root).expect("create containers root");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let outside = tmp_dir.join("outside/debian");
        fs::create_dir_all(&outside).expect("create outside dir");
        cleanup_oci_on_failure(&outside);
        assert!(outside.exists());

        let invalid_leaf = containers_root.join(".bad");
        fs::create_dir_all(&invalid_leaf).expect("create invalid alias dir");
        cleanup_oci_on_failure(&invalid_leaf);
        assert!(invalid_leaf.exists());

        if let Some(prev) = _prev_prefix {
            std::env::set_var("APP_PREFIX", prev);
        } else {
            std::env::remove_var("APP_PREFIX");
        }
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn command_install_rejects_tarball_overrides_for_oci_source() {
        let err = command_install(
            "docker.io/library/alpine:latest",
            None,
            Some("https://example.invalid/rootfs.tar.xz"),
            None,
        )
        .expect_err("must reject tarball overrides for OCI installs");
        assert!(err.contains("not supported for OCI image installs"));
    }

    #[test]
    fn command_install_rejects_invalid_override_alias_before_plugin_checks() {
        let err = command_install("debian", Some("../escape"), None, None)
            .expect_err("must reject invalid alias");
        assert!(err.contains("--override-alias"));
    }

    fn with_test_prefix(label: &str, plugin: &str, body: impl FnOnce(&Path)) {
        let _guard = env_lock().lock().expect("lock env");
        let prefix = unique_temp_dir(label).join("usr");
        fs::create_dir_all(prefix.join("etc/pr")).expect("create plugins dir");
        fs::write(prefix.join("etc/pr/testdistro.sh"), plugin).expect("write plugin");
        std::env::set_var("APP_PREFIX", &prefix);
        std::env::set_var("DISTRO_ARCH", "x86_64");
        body(&prefix);
        std::env::remove_var("APP_PREFIX");
        std::env::remove_var("DISTRO_ARCH");
        let _ = fs::remove_dir_all(prefix.parent().expect("base"));
    }

    #[test]
    fn the_marker_is_held_before_the_container_exists_and_until_it_is_done() {
        let parent = unique_temp_dir("pr-cli-install-marker-held");
        let dir = parent.join("box");

        let result = install_container(
            &parent,
            "box",
            &dir,
            || false,
            || {
                assert!(!dir.exists());
                assert_eq!(
                    pending_install(&parent, "box"),
                    Some(PendingInstall::InProgress)
                );
                fs::create_dir_all(&dir).map_err(|e| e.to_string())
            },
            || panic!("no cleanup on success"),
        );

        assert!(result.is_ok());
        assert!(dir.is_dir());
        assert_eq!(pending_install(&parent, "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_failed_install_cleans_up_and_drops_the_marker() {
        let parent = unique_temp_dir("pr-cli-install-marker-failed");
        let dir = parent.join("box");

        let result = install_container(
            &parent,
            "box",
            &dir,
            || false,
            || {
                fs::create_dir_all(dir.join("etc")).expect("partial");
                Err("layer failed".to_string())
            },
            || force_remove_dir_all(&dir),
        );

        assert_eq!(result, Err("layer failed".to_string()));
        assert!(!dir.exists());
        assert_eq!(pending_install(&parent, "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn a_failed_install_whose_cleanup_fails_stays_interrupted() {
        let parent = unique_temp_dir("pr-cli-install-marker-stuck");
        let dir = parent.join("box");

        let result = install_container(
            &parent,
            "box",
            &dir,
            || false,
            || {
                fs::create_dir_all(dir.join("etc")).expect("partial");
                Err("layer failed".to_string())
            },
            || {},
        );

        assert!(result.is_err());
        assert_eq!(
            pending_install(&parent, "box"),
            Some(PendingInstall::Interrupted)
        );
        let _ = fs::remove_dir_all(parent);
    }

    const GOOD_PLUGIN: &str =
        "DISTRO_NAME=\"Test\"\nTARBALL_URL_x86_64=\"https://example.invalid/good.tar.xz\"\n";

    fn seed_good_archive(prefix: &Path) {
        let cache = prefix.join("var/lib/pr/dlcache");
        fs::create_dir_all(&cache).expect("create cache");
        write_raw_xz_tar(
            &cache.join("good.tar.xz"),
            &[
                RawEntry::Dir("top/"),
                RawEntry::Dir("top/etc/"),
                RawEntry::File("top/etc/os-release", b"ID=test\n", 0o644),
                RawEntry::Dir("top/bin/"),
                RawEntry::File("top/bin/sh", b"\x7fELFsh", 0o755),
            ],
        );
    }

    #[test]
    fn a_container_finished_by_someone_else_meanwhile_is_left_alone() {
        let parent = unique_temp_dir("pr-cli-install-marker-raced");
        let dir = parent.join("box");
        fs::create_dir_all(dir.join("rootfs")).expect("finished elsewhere");

        let result = install_container(
            &parent,
            "box",
            &dir,
            || dir.join("rootfs").is_dir(),
            || panic!("must not populate"),
            || panic!("must not clean up"),
        );

        assert!(result.expect_err("refused").contains("already installed"));
        assert!(dir.join("rootfs").is_dir());
        assert_eq!(pending_install(&parent, "box"), None);
        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn an_oci_install_reuses_a_container_directory_without_a_rootfs() {
        with_test_prefix("pr-cli-install-oci-leftover", GOOD_PLUGIN, |prefix| {
            let dir = prefix.join("var/lib/pr/containers/box");
            fs::create_dir_all(&dir).expect("leftover from an older pr-cli");

            install_oci_container("box", || {
                fs::create_dir_all(dir.join("rootfs/etc")).map_err(|e| e.to_string())
            })
            .expect("install");

            assert!(resolve_installed_rootfs("box").is_some());
        });
    }

    #[test]
    fn a_finished_legacy_install_has_no_marker() {
        with_test_prefix("pr-cli-install-finished", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");

            command_install("testdistro", None, None, None).expect("install");

            assert!(legacy.join("testdistro/bin/sh").is_file());
            assert_eq!(pending_install(&legacy, "testdistro"), None);
            assert!(resolve_installed_rootfs("testdistro").is_some());
        });
    }

    #[test]
    fn install_replaces_an_interrupted_legacy_install() {
        with_test_prefix("pr-cli-install-interrupted", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            drop(InstallMarker::claim(&legacy, "testdistro").expect("mark"));
            fs::create_dir_all(legacy.join("testdistro")).expect("partial rootfs");
            fs::write(legacy.join("testdistro/half-written"), b"x").expect("partial file");

            command_install("testdistro", None, None, None).expect("install");

            assert!(legacy.join("testdistro/bin/sh").is_file());
            assert!(!legacy.join("testdistro/half-written").exists());
            assert_eq!(pending_install(&legacy, "testdistro"), None);
        });
    }

    #[test]
    fn an_interrupted_override_alias_install_can_be_retried() {
        with_test_prefix("pr-cli-install-alias-retry", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            fs::write(prefix.join("etc/pr/mydeb.override.sh"), GOOD_PLUGIN).expect("plugin");
            drop(InstallMarker::claim(&legacy, "mydeb").expect("mark"));
            fs::create_dir_all(legacy.join("mydeb")).expect("partial rootfs");

            command_install("testdistro", Some("mydeb"), None, None).expect("install");

            assert!(legacy.join("mydeb/bin/sh").is_file());
            assert!(fs::read_to_string(prefix.join("etc/pr/mydeb.override.sh"))
                .unwrap()
                .contains("DISTRO_NAME=\"Test - mydeb\""));
            assert_eq!(pending_install(&legacy, "mydeb"), None);
        });
    }

    #[test]
    fn an_interrupted_install_does_not_let_an_alias_shadow_a_plugin() {
        with_test_prefix("pr-cli-install-alias-shadow", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            fs::write(prefix.join("etc/pr/other.sh"), GOOD_PLUGIN).expect("plugin");
            fs::write(prefix.join("etc/pr/other.override.sh"), GOOD_PLUGIN).expect("override");
            drop(InstallMarker::claim(&legacy, "other").expect("mark"));

            let err =
                command_install("testdistro", Some("other"), None, None).expect_err("must refuse");

            assert!(err.contains("already exists"), "{}", err);
            assert_eq!(
                fs::read(prefix.join("etc/pr/other.override.sh")).unwrap(),
                GOOD_PLUGIN.as_bytes()
            );
        });
    }

    #[test]
    fn an_oci_install_refuses_a_name_with_an_unfinished_legacy_install() {
        with_test_prefix("pr-cli-install-oci-over-legacy", GOOD_PLUGIN, |prefix| {
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            drop(InstallMarker::claim(&legacy, "box").expect("mark"));
            fs::create_dir_all(legacy.join("box")).expect("partial legacy");

            let err =
                install_oci_container("box", || panic!("must not pull")).expect_err("must refuse");

            assert!(err.contains("unfinished"), "{}", err);
            assert!(!prefix.join("var/lib/pr/containers/box").exists());
        });
    }

    #[test]
    fn a_legacy_install_refuses_a_name_with_an_unfinished_oci_install() {
        with_test_prefix("pr-cli-install-legacy-over-oci", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let containers = prefix.join("var/lib/pr/containers");
            drop(InstallMarker::claim(&containers, "testdistro").expect("mark"));

            let err = command_install("testdistro", None, None, None).expect_err("must refuse");

            assert!(err.contains("unfinished"), "{}", err);
            assert!(!prefix
                .join("var/lib/pr/installed-rootfs/testdistro")
                .exists());
        });
    }

    #[test]
    fn a_refused_retry_leaves_the_override_plugin_alone() {
        with_test_prefix("pr-cli-install-alias-refused", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            let containers = prefix.join("var/lib/pr/containers");
            let before = "DISTRO_NAME=\"Before\"\n";
            fs::write(prefix.join("etc/pr/mydeb.override.sh"), before).expect("plugin");
            drop(InstallMarker::claim(&legacy, "mydeb").expect("mark legacy"));
            drop(InstallMarker::claim(&containers, "mydeb").expect("mark oci"));

            assert!(command_install("testdistro", Some("mydeb"), None, None).is_err());

            assert_eq!(
                fs::read_to_string(prefix.join("etc/pr/mydeb.override.sh")).unwrap(),
                before
            );
        });
    }

    #[test]
    fn install_refuses_a_name_another_install_is_using() {
        with_test_prefix("pr-cli-install-running", GOOD_PLUGIN, |prefix| {
            seed_good_archive(prefix);
            let legacy = prefix.join("var/lib/pr/installed-rootfs");
            let running = InstallMarker::claim(&legacy, "testdistro").expect("mark");
            fs::create_dir_all(legacy.join("testdistro")).expect("rootfs");
            fs::write(legacy.join("testdistro/in-progress"), b"x").expect("partial file");

            let err = command_install("testdistro", None, None, None).expect_err("refused");

            assert_eq!(err, BEING_INSTALLED);
            assert!(legacy.join("testdistro/in-progress").exists());
            drop(running);
        });
    }

    #[test]
    fn an_oci_install_refuses_a_name_another_install_is_using() {
        with_test_prefix("pr-cli-install-oci-running", GOOD_PLUGIN, |prefix| {
            let containers = prefix.join("var/lib/pr/containers");
            let running = InstallMarker::claim(&containers, "box").expect("mark");

            let err = install_from_oci_reference("docker.io/library/alpine:latest", Some("box"))
                .expect_err("refused");

            assert_eq!(err, BEING_INSTALLED);
            assert_eq!(
                pending_install(&containers, "box"),
                Some(PendingInstall::InProgress)
            );
            drop(running);
        });
    }

    #[test]
    fn an_oci_install_holds_the_marker_while_it_pulls() {
        with_test_prefix("pr-cli-install-oci-marker", GOOD_PLUGIN, |prefix| {
            let containers = prefix.join("var/lib/pr/containers");
            let dir = containers.join("box");

            install_oci_container("box", || {
                assert!(!dir.exists());
                assert_eq!(
                    pending_install(&containers, "box"),
                    Some(PendingInstall::InProgress)
                );
                fs::create_dir_all(dir.join("rootfs/etc")).map_err(|e| e.to_string())
            })
            .expect("install");

            assert!(resolve_installed_rootfs("box").is_some());
            assert_eq!(pending_install(&containers, "box"), None);
        });
    }

    #[test]
    fn a_failed_oci_install_removes_the_container() {
        with_test_prefix("pr-cli-install-oci-failed", GOOD_PLUGIN, |prefix| {
            let containers = prefix.join("var/lib/pr/containers");
            let dir = containers.join("box");

            let result = install_oci_container("box", || {
                fs::create_dir_all(dir.join("rootfs/etc")).expect("partial rootfs");
                Err("layer failed".to_string())
            });

            assert_eq!(result, Err("layer failed".to_string()));
            assert!(!dir.exists());
            assert_eq!(pending_install(&containers, "box"), None);
        });
    }

    #[test]
    fn an_oci_install_replaces_an_interrupted_one() {
        with_test_prefix("pr-cli-install-oci-interrupted", GOOD_PLUGIN, |prefix| {
            let containers = prefix.join("var/lib/pr/containers");
            let dir = containers.join("box");
            drop(InstallMarker::claim(&containers, "box").expect("mark"));
            fs::create_dir_all(dir.join("rootfs/etc")).expect("partial rootfs");
            fs::write(dir.join("rootfs/half-written"), b"x").expect("partial file");

            install_oci_container("box", || {
                fs::create_dir_all(dir.join("rootfs/etc")).map_err(|e| e.to_string())
            })
            .expect("install");

            assert!(!dir.join("rootfs/half-written").exists());
            assert!(resolve_installed_rootfs("box").is_some());
        });
    }

    #[test]
    fn command_install_leaves_no_rootfs_when_the_arch_has_no_tarball() {
        let plugin =
            "DISTRO_NAME=\"Test\"\nTARBALL_URL_aarch64=\"https://example.invalid/a.tar.xz\"\n";
        with_test_prefix("pr-cli-install-no-arch", plugin, |prefix| {
            assert!(command_install("testdistro", None, None, None).is_err());
            assert!(!prefix
                .join("var/lib/pr/installed-rootfs/testdistro")
                .exists());
        });
    }

    #[test]
    fn a_tarball_override_installs_a_catalog_name_from_the_tarball() {
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let prefix = unique_temp_dir("pr-cli-install-catalog-tarball").join("usr");
        let cache = prefix.join("var/lib/pr/dlcache");
        fs::create_dir_all(&cache).expect("create cache");
        fs::write(cache.join("broken.tar.xz"), b"not an archive").expect("seed archive");
        std::env::set_var("APP_PREFIX", &prefix);
        std::env::set_var("DISTRO_ARCH", "x86_64");

        let result = command_install(
            "debian",
            None,
            Some("https://example.invalid/broken.tar.xz"),
            None,
        );

        std::env::remove_var("APP_PREFIX");
        std::env::remove_var("DISTRO_ARCH");
        let err = result.expect_err("the archive is broken");
        assert!(!err.contains("not supported"), "{}", err);
        assert!(
            !cache.join("broken.tar.xz").exists(),
            "extraction was not tried"
        );
        assert!(!prefix.join("var/lib/pr/installed-rootfs/debian").exists());
        let _ = fs::remove_dir_all(prefix.parent().expect("base"));
    }

    #[test]
    fn command_install_removes_the_rootfs_when_extraction_fails() {
        let plugin =
            "DISTRO_NAME=\"Test\"\nTARBALL_URL_x86_64=\"https://example.invalid/broken.tar.xz\"\n";
        with_test_prefix("pr-cli-install-broken-archive", plugin, |prefix| {
            let cache = prefix.join("var/lib/pr/dlcache");
            fs::create_dir_all(&cache).expect("create cache");
            fs::write(cache.join("broken.tar.xz"), b"not an archive").expect("seed archive");

            assert!(command_install("testdistro", None, None, None).is_err());
            assert!(!prefix
                .join("var/lib/pr/installed-rootfs/testdistro")
                .exists());
            assert!(!cache.join("broken.tar.xz").exists());
        });
    }

    #[test]
    fn derive_oci_install_name_rejects_invalid_override_alias() {
        let resolved = ResolvedOciReference {
            registry_base: "https://ghcr.io".to_string(),
            repository: "example/app".to_string(),
            reference: "latest".to_string(),
            digest_reference: false,
        };
        let err = derive_oci_install_name(&resolved, Some("../bad"))
            .expect_err("invalid override alias must fail");
        assert!(err.contains("--override-alias"));
    }

    #[test]
    fn normalized_oci_reference_trims_registry_trailing_slash() {
        let resolved = ResolvedOciReference {
            registry_base: "https://ghcr.io/".to_string(),
            repository: "example/app".to_string(),
            reference: "latest".to_string(),
            digest_reference: false,
        };
        assert_eq!(
            normalized_oci_reference(&resolved),
            "ghcr.io/example/app:latest"
        );
    }

    #[test]
    fn detect_device_arch_prefers_distro_arch_env_override() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var("DISTRO_ARCH", "armv7");
        assert_eq!(detect_device_arch(), "armv7");
        std::env::remove_var("DISTRO_ARCH");
    }

    #[test]
    fn registry_host_helpers_cover_common_cases() {
        assert!(looks_like_registry_host("ghcr.io"));
        assert!(looks_like_registry_host("localhost:5000"));
        assert!(!looks_like_registry_host("library"));

        assert_eq!(normalize_registry_host("DOCKER.IO"), "registry-1.docker.io");
        assert_eq!(normalize_registry_host("ghcr.io"), "ghcr.io");

        assert!(is_docker_hub_host("registry-1.docker.io"));
        assert!(!is_docker_hub_host("ghcr.io"));
    }

    #[test]
    fn map_descriptor_preserves_digest_size_and_platform() {
        let descriptor = RegistryDescriptor {
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            digest: "sha256:abc123".to_string(),
            size: Some(42),
            urls: vec!["https://example.invalid/blob".to_string()],
            annotations: std::collections::BTreeMap::from([(
                "org.opencontainers.image.ref.name".to_string(),
                "latest".to_string(),
            )]),
            platform: Some(RegistryPlatform {
                os: "linux".to_string(),
                architecture: "arm64".to_string(),
                variant: Some("v8".to_string()),
                os_version: Some("1".to_string()),
                os_features: vec!["feat".to_string()],
            }),
        };

        let mapped = map_descriptor(descriptor);
        assert_eq!(
            mapped.media_type,
            Some("application/vnd.oci.image.manifest.v1+json".to_string())
        );
        assert_eq!(mapped.digest, "sha256:abc123");
        assert_eq!(mapped.size, Some(42));
        assert_eq!(mapped.urls.len(), 1);
        assert_eq!(mapped.annotations.len(), 1);
        let platform = mapped.platform.expect("platform");
        assert_eq!(platform.os, "linux");
        assert_eq!(platform.architecture, "arm64");
        assert_eq!(platform.variant.as_deref(), Some("v8"));
    }

    #[test]
    fn parse_bearer_challenge_requires_realm() {
        assert!(parse_bearer_challenge(r#"Bearer service="registry.example""#).is_none());
    }

    #[test]
    fn map_descriptor_handles_missing_platform() {
        let descriptor = RegistryDescriptor {
            media_type: None,
            digest: "sha256:def456".to_string(),
            size: None,
            urls: Vec::new(),
            annotations: std::collections::BTreeMap::new(),
            platform: None,
        };
        let mapped = map_descriptor(descriptor);
        assert_eq!(mapped.digest, "sha256:def456");
        assert!(mapped.platform.is_none());
        assert!(mapped.media_type.is_none());
    }

    #[test]
    fn append_line_if_missing_creates_new_file() {
        let tmp_dir = unique_temp_dir("pr-cli-append-line-new-file");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let file_path = tmp_dir.join("new.conf");

        append_line_if_missing(&tmp_dir, "new.conf", "line=value").expect("append line");
        let content = fs::read_to_string(&file_path).expect("read file");
        assert_eq!(content, "line=value\n");

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn uncomment_en_us_locale_returns_error_when_file_missing() {
        let tmp_dir = unique_temp_dir("pr-cli-locale-missing");
        fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let err = uncomment_en_us_locale(tmp_dir.to_str().expect("tmp path"))
            .expect_err("must fail when locale.gen is missing");
        assert!(err.contains("locale.gen"));
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn write_config_files_with_non_rust_owned_distro_skips_profile_setup() {
        let tmp_dir = unique_temp_dir("pr-cli-write-config-alpine");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).expect("create etc");
        fs::write(rootfs.join("etc/environment"), "").expect("seed environment");
        fs::write(rootfs.join("etc/hosts"), "").expect("seed hosts");
        fs::write(rootfs.join("etc/passwd"), "").expect("seed passwd");
        fs::write(rootfs.join("etc/shadow"), "").expect("seed shadow");
        fs::write(rootfs.join("etc/group"), "").expect("seed group");
        fs::write(rootfs.join("etc/gshadow"), "").expect("seed gshadow");

        write_config_files(rootfs.to_str().expect("path"), Some("alpine"))
            .expect("write config files");
        assert!(rootfs.join("etc/resolv.conf").is_file());

        let _ = fs::remove_dir_all(tmp_dir);
    }

    fn gzip_layer(path_in_layer: &str, content: &[u8]) -> (Vec<u8>, String) {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(content.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, path_in_layer, content)
            .expect("append layer file");
        let bytes = builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip");
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        (bytes, digest)
    }

    #[test]
    fn install_oci_layers_downloads_again_over_a_truncated_cached_blob() {
        let tmp_dir = unique_temp_dir("pr-cli-oci-truncated-blob");
        let cache = tmp_dir.join("dlcache");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&cache).expect("create cache");
        let (layer_bytes, digest) = gzip_layer("etc/os-release", b"ID=debian\n");
        let blob = blob_path(&cache, &digest).expect("blob path");
        fs::write(&blob, &layer_bytes[..layer_bytes.len() / 2]).expect("seed truncated blob");

        let mut fetches = 0;
        install_oci_layers(
            &[OciDescriptor::new(digest.clone())],
            &cache,
            &rootfs,
            |layer, dest| {
                fetches += 1;
                assert_eq!(layer.digest, digest);
                assert!(!dest.exists());
                fs::write(dest, &layer_bytes).map_err(|e| e.to_string())
            },
        )
        .expect("install layers");

        assert_eq!(fetches, 1);
        assert_eq!(fs::read(&blob).expect("read blob"), layer_bytes);
        assert_eq!(
            fs::read(rootfs.join("etc/os-release")).expect("read os-release"),
            b"ID=debian\n"
        );
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn install_oci_layers_reuses_a_cached_blob_that_matches_its_digest() {
        let tmp_dir = unique_temp_dir("pr-cli-oci-cached-blob");
        let cache = tmp_dir.join("dlcache");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&cache).expect("create cache");
        let (layer_bytes, digest) = gzip_layer("etc/os-release", b"ID=alpine\n");
        let blob = blob_path(&cache, &digest).expect("blob path");
        fs::write(&blob, &layer_bytes).expect("seed blob");

        let mut fetches = 0;
        install_oci_layers(&[OciDescriptor::new(digest)], &cache, &rootfs, |_, _| {
            fetches += 1;
            Err("unexpected download".to_string())
        })
        .expect("install layers");

        assert_eq!(fetches, 0);
        assert!(rootfs.join("etc/os-release").is_file());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn install_oci_layers_drops_a_corrupt_cached_blob_when_the_download_fails() {
        let tmp_dir = unique_temp_dir("pr-cli-oci-corrupt-blob");
        let cache = tmp_dir.join("dlcache");
        let rootfs = tmp_dir.join("rootfs");
        fs::create_dir_all(&cache).expect("create cache");
        let (mut layer_bytes, digest) = gzip_layer("etc/os-release", b"ID=debian\n");
        let last = layer_bytes.len() - 1;
        layer_bytes[last] ^= 0xff;
        let blob = blob_path(&cache, &digest).expect("blob path");
        fs::write(&blob, &layer_bytes).expect("seed corrupt blob");

        let err = install_oci_layers(&[OciDescriptor::new(digest)], &cache, &rootfs, |_, _| {
            Err("network is unreachable".to_string())
        })
        .expect_err("install must fail");

        assert_eq!(err, "network is unreachable");
        assert!(!blob.exists());
        assert!(!rootfs.join("etc/os-release").exists());
        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn cleanup_oci_on_failure_removes_a_partially_extracted_container() {
        let guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev_prefix = std::env::var("APP_PREFIX").ok();
        let tmp_dir = unique_temp_dir("pr-cli-cleanup-partial-oci");
        let prefix = tmp_dir.join("usr");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());
        let container = PathBuf::from(get_oci_container_dir("debian"));
        let rootfs = container.join("rootfs");
        fs::create_dir_all(rootfs.join(".l2s")).expect("create .l2s");
        fs::create_dir_all(rootfs.join("usr/bin")).expect("create usr/bin");
        fs::write(rootfs.join("usr/bin/perl"), b"\x7fELF").expect("write partial perl");
        std::os::unix::fs::symlink("usr/bin", rootfs.join("bin")).expect("symlink bin");
        let read_only = rootfs.join("usr/share/locked");
        fs::create_dir_all(&read_only).expect("create read-only dir");
        fs::write(read_only.join("file"), b"x").expect("write locked file");
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555)).expect("chmod 0555");
        let sealed = rootfs.join("apex");
        fs::create_dir_all(&sealed).expect("create sealed dir");
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).expect("chmod 0000");

        cleanup_oci_on_failure(&container);
        let left = container.exists();

        if left {
            let _ = fs::set_permissions(&read_only, fs::Permissions::from_mode(0o755));
            let _ = fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755));
        }
        if let Some(prev) = prev_prefix {
            std::env::set_var("APP_PREFIX", prev);
        } else {
            std::env::remove_var("APP_PREFIX");
        }
        let _ = fs::remove_dir_all(&tmp_dir);
        drop(guard);
        assert!(!left, "partial container must be removed");
    }
}
