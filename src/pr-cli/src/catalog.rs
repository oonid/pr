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
        alias: "archlinux",
        name: "Arch Linux",
        reference: "docker.io/library/archlinux:latest",
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
        reference: "docker.io/library/rockylinux:latest",
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
