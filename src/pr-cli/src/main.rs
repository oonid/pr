use clap::{Parser, Subcommand};
use pr_cli::cmd_test::command_test;
use pr_cli::color::*;
use pr_cli::commands_extra;
use pr_cli::install::command_install;
use pr_cli::install_marker::{marked_names, pending_install, PendingInstall};
use pr_cli::install_model::load_oci_install_metadata;
use pr_cli::login::command_login;
use pr_cli::plugin::{load_plugins, DistroPlugin};
use pr_cli::shared;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const VERSION: &str = shared::VERSION;

#[derive(Parser)]
#[command(
    name = "pr-cli",
    version = VERSION,
    about = "proot-distro replacement for Android"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Install {
        distro: String,
        #[arg(long)]
        override_alias: Option<String>,
        #[arg(long)]
        override_tarball_url: Option<String>,
        #[arg(long)]
        override_tarball_sha256: Option<String>,
    },
    Login {
        distro: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long)]
        isolated: bool,
        #[arg(long)]
        no_link2symlink: bool,
        #[arg(long)]
        custom_bind: Vec<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    Remove {
        distro: String,
    },
    List {
        #[arg(short, long)]
        verbose: bool,
    },
    Backup {
        distro: String,
        #[arg(long)]
        output: Option<String>,
    },
    Restore {
        distro: String,
    },
    Rename {
        old_alias: String,
        new_alias: String,
    },
    Reset {
        distro: String,
    },
    Copy {
        src_distro: String,
        dst_distro: String,
    },
    Test {
        distro: String,
        #[arg(short, long)]
        suite: Option<String>,
        #[arg(short, long)]
        verbose: bool,
    },
    #[command(name = "clear-cache")]
    ClearCache {},
}

fn get_plugins_dir() -> PathBuf {
    PathBuf::from(shared::get_plugins_dir())
}

fn get_installed_rootfs_dir() -> PathBuf {
    PathBuf::from(shared::get_installed_rootfs_dir())
}

fn get_oci_containers_dir() -> PathBuf {
    PathBuf::from(shared::get_oci_containers_dir())
}

fn is_installed(alias: &str) -> bool {
    shared::resolve_installed_rootfs(alias).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum InstalledSource {
    Legacy,
    Oci,
}

impl InstalledSource {
    fn as_str(self) -> &'static str {
        match self {
            InstalledSource::Legacy => "legacy",
            InstalledSource::Oci => "oci",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct InstalledEntry {
    alias: String,
    source: InstalledSource,
    install_name: Option<String>,
    source_reference: Option<String>,
    selected_architecture: Option<String>,
    pending: Option<PendingInstall>,
}

fn source_label(entry: &InstalledEntry) -> String {
    let source = entry.source.as_str();
    match entry.pending {
        None => source.to_string(),
        Some(PendingInstall::InProgress) => format!("{}, installing", source),
        Some(PendingInstall::Interrupted) => format!("{}, interrupted", source),
    }
}

fn list_subdirs(path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = fs::read_dir(path) else {
        return names;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.file_type() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.is_empty() {
            names.push(name);
        }
    }
    names.sort();
    names
}

fn collect_installed_entries_with_dirs(
    installed_rootfs_dir: &Path,
    oci_containers_dir: &Path,
) -> Vec<InstalledEntry> {
    let mut entries = Vec::new();

    for alias in list_subdirs(installed_rootfs_dir) {
        if !shared::is_valid_container_name(&alias) {
            continue;
        }
        entries.push(InstalledEntry {
            pending: pending_install(installed_rootfs_dir, &alias),
            alias,
            source: InstalledSource::Legacy,
            install_name: None,
            source_reference: None,
            selected_architecture: None,
        });
    }

    for alias in list_subdirs(oci_containers_dir) {
        if !shared::is_valid_container_name(&alias) {
            continue;
        }
        let pending = pending_install(oci_containers_dir, &alias);
        if pending.is_some() || oci_containers_dir.join(&alias).join("rootfs").is_dir() {
            let metadata_path = oci_containers_dir.join(&alias).join("manifest.json");
            let metadata = load_oci_install_metadata(&metadata_path).ok();
            entries.push(InstalledEntry {
                alias,
                source: InstalledSource::Oci,
                install_name: metadata.as_ref().map(|meta| meta.install_name.clone()),
                source_reference: metadata
                    .as_ref()
                    .map(|meta| meta.normalized_source_reference.clone()),
                selected_architecture: metadata
                    .as_ref()
                    .map(|meta| meta.selected_architecture.clone()),
                pending,
            });
        }
    }

    for (parent, source) in [
        (installed_rootfs_dir, InstalledSource::Legacy),
        (oci_containers_dir, InstalledSource::Oci),
    ] {
        for alias in marked_names(parent) {
            if entries
                .iter()
                .any(|e| e.alias == alias && e.source == source)
            {
                continue;
            }
            entries.push(InstalledEntry {
                pending: pending_install(parent, &alias),
                alias,
                source,
                install_name: None,
                source_reference: None,
                selected_architecture: None,
            });
        }
    }

    entries.sort();
    entries
}

fn collect_installed_entries() -> Vec<InstalledEntry> {
    collect_installed_entries_with_dirs(&get_installed_rootfs_dir(), &get_oci_containers_dir())
}

fn supported_distributions(plugins: &[DistroPlugin]) -> Vec<(String, String)> {
    let mut listed: Vec<(String, String)> = pr_cli::catalog::CATALOG
        .iter()
        .filter(|entry| !plugins.iter().any(|p| p.alias == entry.alias))
        .map(|entry| (entry.name.to_string(), entry.alias.to_string()))
        .collect();
    listed.extend(plugins.iter().map(|p| (p.name.clone(), p.alias.clone())));
    listed.sort_by(|a, b| a.1.cmp(&b.1));
    listed
}

fn command_list(verbose: bool) {
    let dir = get_plugins_dir();
    let plugins = load_plugins(&dir);
    let installed = collect_installed_entries();
    let plugin_name_by_alias: BTreeMap<String, String> = supported_distributions(&plugins)
        .into_iter()
        .map(|(name, alias)| (alias, name))
        .collect();

    println!();

    if verbose {
        println!("{}Supported distributions:{}", CYAN, RESET);
    } else {
        println!(
            "{}Supported distributions (format: name < alias >):{}",
            CYAN, RESET
        );
        println!();
    }

    for (name, alias) in supported_distributions(&plugins) {
        if !verbose {
            println!(
                "  {}* {}{} {}< {}{}>",
                CYAN, YELLOW, name, GREEN, alias, RESET
            );
            continue;
        }
        println!();
        println!("  {}* {}{}{}", CYAN, YELLOW, name, RESET);
        println!();
        println!("    {}Alias: {}{}{}", CYAN, GREEN, alias, RESET);
        if is_installed(&alias) {
            println!("    {}Installed: {}yes{}", CYAN, GREEN, RESET);
        } else {
            println!("    {}Installed: {}no{}", CYAN, RED, RESET);
        }
        if let Some(p) = plugins.iter().find(|p| p.alias == alias) {
            if let Some(ref comment) = p.comment {
                println!("    {}Comment: {}{}{}", CYAN, RESET, comment, RESET);
            }
            let archs = p.supported_architectures().join(", ");
            println!("    {}Architectures: {}{}{}", CYAN, RESET, archs, RESET);
        } else if let Some(entry) = pr_cli::catalog::catalog_entry(&alias) {
            println!("    {}Image: {}{}{}", CYAN, RESET, entry.reference, RESET);
        }
    }

    if verbose {
        println!();
        println!("{}Installed distributions:{}", CYAN, RESET);
        if installed.is_empty() {
            println!("  {}(none){}", YELLOW, RESET);
        } else {
            for entry in &installed {
                let display_name = plugin_name_by_alias
                    .get(&entry.alias)
                    .cloned()
                    .unwrap_or_else(|| entry.alias.clone());
                println!();
                println!("  {}* {}{}{}", CYAN, YELLOW, display_name, RESET);
                println!("    {}Alias: {}{}{}", CYAN, GREEN, entry.alias, RESET);
                if let Some(install_name) = entry.install_name.as_deref() {
                    if install_name != entry.alias {
                        println!(
                            "    {}Recorded install name: {}{}{}",
                            CYAN, GREEN, install_name, RESET
                        );
                    }
                }
                println!(
                    "    {}Source: {}{}{}",
                    CYAN,
                    GREEN,
                    source_label(entry),
                    RESET
                );
                if let Some(source_reference) = entry.source_reference.as_deref() {
                    println!(
                        "    {}Reference: {}{}{}",
                        CYAN, GREEN, source_reference, RESET
                    );
                }
                if let Some(selected_architecture) = entry.selected_architecture.as_deref() {
                    println!(
                        "    {}Architecture: {}{}{}",
                        CYAN, GREEN, selected_architecture, RESET
                    );
                }
            }
        }
    } else {
        println!();
        println!(
            "{}Installed distributions (format: alias < source >):{}",
            CYAN, RESET
        );
        println!();
        if installed.is_empty() {
            println!("  {}* {}(none){}", CYAN, YELLOW, RESET);
        } else {
            for entry in &installed {
                println!(
                    "  {}* {}{} {}< {}{}>{}",
                    CYAN,
                    YELLOW,
                    entry.alias,
                    GREEN,
                    source_label(entry),
                    RESET,
                    ""
                );
            }
        }
    }

    println!();
    println!(
        "{}Install selected one with: {}pr-cli install <alias>{}",
        CYAN, GREEN, RESET
    );
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static std::sync::Mutex<()> {
        pr_cli::shared::global_test_env_lock()
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time ok")
            .as_nanos();
        std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos))
    }

    #[test]
    fn test_cli_android_setup_args() {
        // This test ensures the complex arguments passed by Android TerminalActivity.kt
        // are correctly parsed by clap, preventing regressions like the "expected argument" error.

        // 1. Test Android INSTALL args
        let install_args = vec![
            "pr-cli",
            "install",
            "docker.io/library/alpine:latest",
            "--override-alias",
            "alpine",
        ];
        let cli_install = Cli::try_parse_from(install_args).expect("Failed to parse install args");
        match cli_install.command {
            Commands::Install {
                distro,
                override_alias,
                ..
            } => {
                assert_eq!(distro, "docker.io/library/alpine:latest");
                assert_eq!(override_alias.as_deref(), Some("alpine"));
            }
            _ => panic!("Expected Install command"),
        }

        // 2. Test Android LOGIN args with trailing command containing spaces
        let login_args = vec![
            "pr-cli",
            "login",
            "alpine",
            "--",
            "apk update && apk add rust cargo gcc && ln -s /usr/bin/gcc /usr/bin/cc 2>/dev/null; echo > /etc/.rust_setup; echo '\r\nSetup complete! You can close this terminal.'"
        ];
        let cli_login = Cli::try_parse_from(login_args).expect("Failed to parse login args");
        match cli_login.command {
            Commands::Login {
                distro, command, ..
            } => {
                assert_eq!(distro, "alpine");
                assert_eq!(command.len(), 1);
                assert!(command[0].starts_with("apk update"));
            }
            _ => panic!("Expected Login command"),
        }
    }

    #[test]
    fn collects_legacy_and_oci_entries() {
        let base = unique_temp_dir("pr-cli-list-entries");
        let legacy = base.join("installed-rootfs");
        let containers = base.join("containers");
        fs::create_dir_all(legacy.join("debian")).expect("create legacy");
        fs::create_dir_all(containers.join("ubuntu").join("rootfs")).expect("create oci");
        fs::create_dir_all(containers.join("broken")).expect("create broken");

        let entries = collect_installed_entries_with_dirs(&legacy, &containers);
        assert_eq!(
            entries,
            vec![
                InstalledEntry {
                    alias: "debian".to_string(),
                    source: InstalledSource::Legacy,
                    install_name: None,
                    source_reference: None,
                    selected_architecture: None,
                    pending: None
                },
                InstalledEntry {
                    alias: "ubuntu".to_string(),
                    source: InstalledSource::Oci,
                    install_name: None,
                    source_reference: None,
                    selected_architecture: None,
                    pending: None
                }
            ]
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn loads_oci_metadata_from_manifest_json() {
        let base = unique_temp_dir("pr-cli-list-metadata");
        let legacy = base.join("installed-rootfs");
        let containers = base.join("containers");
        fs::create_dir_all(containers.join("debian").join("rootfs")).expect("create oci rootfs");
        fs::write(
            containers.join("debian").join("manifest.json"),
            r#"{
  "install_name": "debian-oci",
  "source_kind": "oci-image",
  "original_source_reference": "docker.io/library/debian:stable",
  "normalized_source_reference": "registry-1.docker.io/library/debian:stable",
  "selected_architecture": "aarch64",
  "created_at": 1717176400
}"#,
        )
        .expect("write metadata");

        let entries = collect_installed_entries_with_dirs(&legacy, &containers);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].alias, "debian");
        assert_eq!(entries[0].install_name.as_deref(), Some("debian-oci"));
        assert_eq!(entries[0].source, InstalledSource::Oci);
        assert_eq!(
            entries[0].source_reference.as_deref(),
            Some("registry-1.docker.io/library/debian:stable")
        );
        assert_eq!(entries[0].selected_architecture.as_deref(), Some("aarch64"));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn list_subdirs_returns_sorted_directories_only() {
        let base = unique_temp_dir("pr-cli-list-subdirs");
        fs::create_dir_all(base.join("z-dir")).expect("create z-dir");
        fs::create_dir_all(base.join("a-dir")).expect("create a-dir");
        fs::write(base.join("not-a-dir.txt"), "x").expect("create file");

        let listed = list_subdirs(&base);
        assert_eq!(listed, vec!["a-dir".to_string(), "z-dir".to_string()]);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn falls_back_to_directory_alias_when_oci_metadata_is_invalid() {
        let base = unique_temp_dir("pr-cli-list-invalid-metadata");
        let legacy = base.join("installed-rootfs");
        let containers = base.join("containers");
        fs::create_dir_all(containers.join("debian").join("rootfs")).expect("create oci rootfs");
        fs::write(
            containers.join("debian").join("manifest.json"),
            "{invalid-json",
        )
        .expect("write invalid metadata");

        let entries = collect_installed_entries_with_dirs(&legacy, &containers);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].alias, "debian");
        assert_eq!(entries[0].install_name, None);
        assert_eq!(entries[0].source, InstalledSource::Oci);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn skips_directories_that_are_not_container_names() {
        let base = unique_temp_dir("pr-cli-list-staging");
        let legacy = base.join("installed-rootfs");
        let containers = base.join("containers");
        fs::create_dir_all(legacy.join(".old.pr-cli-restore")).expect("create legacy staging");
        fs::create_dir_all(containers.join(".box.pr-cli-restore/rootfs"))
            .expect("create oci staging");
        fs::create_dir_all(containers.join("box/rootfs")).expect("create oci rootfs");

        let entries = collect_installed_entries_with_dirs(&legacy, &containers);
        let aliases: Vec<_> = entries.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(aliases, vec!["box"]);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn supported_distributions_list_the_catalog_and_plugins() {
        let plugin = pr_cli::plugin::DistroPlugin {
            alias: "mine".to_string(),
            name: "My Distro".to_string(),
            comment: None,
            tarballs: std::collections::HashMap::new(),
            has_setup: false,
        };
        let listed = supported_distributions(&[plugin]);
        assert!(listed.contains(&("Debian (stable)".to_string(), "debian".to_string())));
        assert!(listed.contains(&("Manjaro".to_string(), "manjaro".to_string())));
        assert!(listed.contains(&("My Distro".to_string(), "mine".to_string())));
        assert_eq!(
            listed.iter().filter(|(_, alias)| alias == "debian").count(),
            1
        );
    }

    #[test]
    fn installed_source_as_str_matches_expected_labels() {
        assert_eq!(InstalledSource::Legacy.as_str(), "legacy");
        assert_eq!(InstalledSource::Oci.as_str(), "oci");
    }

    #[test]
    fn list_subdirs_returns_empty_for_missing_directory() {
        let listed = list_subdirs(Path::new("/path/that/should/not/exist-pr-cli"));
        assert!(listed.is_empty());
    }

    #[test]
    fn is_installed_detects_legacy_and_oci_layouts() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-is-installed");
        let prefix = base.join("usr");
        fs::create_dir_all(prefix.join("var/lib/pr/installed-rootfs/debian"))
            .expect("create legacy rootfs");
        fs::create_dir_all(prefix.join("var/lib/pr/containers/ubuntu/rootfs"))
            .expect("create oci rootfs");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        assert!(is_installed("debian"));
        assert!(is_installed("ubuntu"));
        assert!(!is_installed("fedora"));

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn an_unfinished_install_is_not_installed() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-is-installed-pending");
        let prefix = base.join("usr");
        let legacy = prefix.join("var/lib/pr/installed-rootfs");
        fs::create_dir_all(legacy.join("debian/etc")).expect("create legacy rootfs");
        drop(pr_cli::install_marker::InstallMarker::claim(&legacy, "debian").expect("mark"));
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        assert!(!is_installed("debian"));

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn list_shows_unfinished_installs_with_their_state() {
        use pr_cli::install_marker::InstallMarker;
        let base = unique_temp_dir("pr-cli-list-pending");
        let legacy = base.join("installed-rootfs");
        let containers = base.join("containers");
        fs::create_dir_all(legacy.join("deb/etc")).expect("create legacy");
        drop(InstallMarker::claim(&legacy, "deb").expect("mark legacy"));
        let running = InstallMarker::claim(&containers, "box").expect("mark oci");
        fs::create_dir_all(containers.join("box")).expect("create oci dir");
        drop(InstallMarker::claim(&legacy, "early").expect("mark early"));

        let entries = collect_installed_entries_with_dirs(&legacy, &containers);

        let labels: Vec<(String, String)> = entries
            .iter()
            .map(|e| (e.alias.clone(), source_label(e)))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("box".to_string(), "oci, installing".to_string()),
                ("deb".to_string(), "legacy, interrupted".to_string()),
                ("early".to_string(), "legacy, interrupted".to_string()),
            ]
        );
        drop(running);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn collect_installed_entries_reads_default_dirs_from_prefix() {
        let _guard = env_lock().lock().expect("lock env");
        let base = unique_temp_dir("pr-cli-collect-installed");
        let prefix = base.join("usr");
        fs::create_dir_all(prefix.join("var/lib/pr/installed-rootfs/debian"))
            .expect("create legacy rootfs");
        fs::create_dir_all(prefix.join("var/lib/pr/containers/ubuntu/rootfs"))
            .expect("create oci rootfs");
        std::env::set_var("APP_PREFIX", prefix.to_string_lossy().to_string());

        let entries = collect_installed_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].alias, "debian");
        assert_eq!(entries[0].source, InstalledSource::Legacy);
        assert_eq!(entries[1].alias, "ubuntu");
        assert_eq!(entries[1].source, InstalledSource::Oci);

        std::env::remove_var("APP_PREFIX");
        let _ = fs::remove_dir_all(base);
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Install {
            distro,
            override_alias,
            override_tarball_url,
            override_tarball_sha256,
        } => {
            if let Err(e) = command_install(
                &distro,
                override_alias.as_deref(),
                override_tarball_url.as_deref(),
                override_tarball_sha256.as_deref(),
            ) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Login {
            distro,
            user,
            isolated,
            no_link2symlink,
            custom_bind,
            command,
        } => {
            if let Err(e) = command_login(
                &distro,
                &user,
                isolated,
                no_link2symlink,
                &custom_bind,
                &command,
            ) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Remove { distro } => {
            if let Err(e) = commands_extra::command_remove(&distro, false) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::List { verbose } => {
            command_list(verbose);
        }
        Commands::Backup { distro, output } => {
            if let Err(e) = commands_extra::command_backup(&distro, output.as_deref()) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Restore { distro } => {
            if let Err(e) = commands_extra::command_restore(&distro) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Rename {
            old_alias,
            new_alias,
        } => {
            if let Err(e) = commands_extra::command_rename(&old_alias, &new_alias) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Reset { distro } => {
            if let Err(e) = commands_extra::command_reset(&distro) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Copy {
            src_distro,
            dst_distro,
        } => {
            if let Err(e) = commands_extra::command_copy(&src_distro, &dst_distro) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Test {
            distro,
            suite,
            verbose,
        } => {
            if let Err(e) = command_test(&distro, suite.as_deref(), verbose) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::ClearCache { .. } => {
            if let Err(e) = commands_extra::command_clear_cache() {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
    }
}
