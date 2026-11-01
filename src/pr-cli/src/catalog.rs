pub struct CatalogEntry {
    pub alias: &'static str,
    pub name: &'static str,
    pub reference: &'static str,
}

pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        alias: "alpine",
        name: "Alpine Linux",
        reference: "docker.io/library/alpine:latest",
    },
    CatalogEntry {
        alias: "alpine-edge",
        name: "Alpine Linux (edge)",
        reference: "docker.io/library/alpine:edge",
    },
    CatalogEntry {
        alias: "debian",
        name: "Debian (stable)",
        reference: "docker.io/library/debian:stable",
    },
    CatalogEntry {
        alias: "debian-testing",
        name: "Debian (testing)",
        reference: "docker.io/library/debian:testing",
    },
    CatalogEntry {
        alias: "fedora",
        name: "Fedora",
        reference: "registry.fedoraproject.org/fedora:latest",
    },
    CatalogEntry {
        alias: "manjaro",
        name: "Manjaro",
        reference: "docker.io/manjarolinux/base:latest",
    },
    CatalogEntry {
        alias: "opensuse",
        name: "openSUSE Tumbleweed",
        reference: "registry.opensuse.org/opensuse/tumbleweed:latest",
    },
    CatalogEntry {
        alias: "rockylinux",
        name: "Rocky Linux",
        reference: "docker.io/rockylinux/rockylinux:9",
    },
    CatalogEntry {
        alias: "ubuntu",
        name: "Ubuntu",
        reference: "docker.io/library/ubuntu:latest",
    },
];

pub fn catalog_entry(alias: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.alias == alias)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn quoted(line: &str) -> Vec<&str> {
        line.split('"').skip(1).step_by(2).collect()
    }

    #[test]
    fn offers_only_images_with_an_arm64_build() {
        assert!(catalog_entry("archlinux").is_none());
        assert_eq!(
            catalog_entry("rockylinux").map(|e| e.reference),
            Some("docker.io/rockylinux/rockylinux:9")
        );
    }

    #[test]
    fn matches_the_app_catalog() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../android/app/src/main/java/id/or/oo/pr/MainActivity.kt"
        ))
        .expect("read MainActivity.kt");
        let mut app = BTreeSet::new();
        for line in source.lines().map(str::trim) {
            if line.starts_with("DistroCatalogEntry(\"") {
                let q = quoted(line);
                app.insert((q[0].to_string(), q[2].to_string()));
            } else if line.starts_with("OciImageSuggestion(\"") {
                let q = quoted(line);
                app.insert((q[2].to_string(), q[1].to_string()));
            }
        }
        let ours: BTreeSet<_> = CATALOG
            .iter()
            .map(|e| (e.alias.to_string(), e.reference.to_string()))
            .collect();
        assert!(
            !app.is_empty(),
            "no catalog entries found in MainActivity.kt"
        );
        assert_eq!(ours, app);
    }
}
