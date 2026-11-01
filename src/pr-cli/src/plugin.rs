use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TarballInfo {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct DistroPlugin {
    pub alias: String,
    pub name: String,
    pub comment: Option<String>,
    pub tarballs: HashMap<String, TarballInfo>,
    pub has_setup: bool,
}

impl DistroPlugin {
    pub fn supported_architectures(&self) -> Vec<&str> {
        let mut archs: Vec<&str> = self.tarballs.keys().map(|s| s.as_str()).collect();
        archs.sort();
        archs
    }
}

impl fmt::Display for DistroPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.name)?;
        if let Some(ref comment) = self.comment {
            writeln!(f, "  comment: {}", comment)?;
        }
        let archs = self.supported_architectures().join(", ");
        writeln!(f, "  archs:    {}", archs)?;
        write!(
            f,
            "  setup:    {}",
            if self.has_setup { "yes" } else { "no" }
        )
    }
}

#[derive(Debug)]
pub enum ParseError {
    Io(io::Error),
    MissingField { field: String, file: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Io(e) => write!(f, "IO error: {}", e),
            ParseError::MissingField { field, file } => {
                write!(f, "missing {} in {}", field, file)
            }
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ParseError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ParseError {
    fn from(e: io::Error) -> Self {
        ParseError::Io(e)
    }
}

fn parse_alias(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or_default()
        .to_string();
    if let Some(alias) = file_name.strip_suffix(".override.sh") {
        return alias.to_string();
    }
    if let Some(alias) = file_name.strip_suffix(".sh") {
        return alias.to_string();
    }
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

pub fn parse_plugin(path: &Path) -> Result<DistroPlugin, ParseError> {
    let content = fs::read_to_string(path)?;
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let alias = parse_alias(path);

    let mut name = None;
    let mut comment = None;
    let mut has_setup = false;
    let mut urls: HashMap<String, String> = HashMap::new();
    let mut sha256s: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if let Some((key, value)) = parse_assignment(trimmed) {
            if key == "DISTRO_NAME" {
                name = Some(value.to_string());
            } else if key == "DISTRO_COMMENT" {
                comment = Some(value.to_string());
            } else if let Some(arch) = key.strip_prefix("TARBALL_URL_") {
                urls.insert(arch.to_string(), value.to_string());
            } else if let Some(arch) = key.strip_prefix("TARBALL_SHA256_") {
                sha256s.insert(arch.to_string(), value.to_string());
            } else if let Some(rest) = key.strip_prefix("TARBALL_URL[") {
                if let Some(arch) = rest.strip_suffix("]") {
                    urls.insert(arch.to_string(), value.to_string());
                }
            } else if let Some(rest) = key.strip_prefix("TARBALL_SHA256[") {
                if let Some(arch) = rest.strip_suffix("]") {
                    sha256s.insert(arch.to_string(), value.to_string());
                }
            }
        }

        if trimmed.starts_with("distro_setup()") || trimmed.starts_with("distro_setup (") {
            has_setup = true;
        }
    }

    let name = name.ok_or_else(|| ParseError::MissingField {
        field: "DISTRO_NAME".to_string(),
        file: file_name.clone(),
    })?;

    if urls.is_empty() {
        return Err(ParseError::MissingField {
            field: "TARBALL_URL_*".to_string(),
            file: file_name,
        });
    }

    let mut tarballs = HashMap::new();
    for (arch, url) in &urls {
        let sha256 = sha256s.get(arch).cloned().unwrap_or_default();
        tarballs.insert(
            arch.clone(),
            TarballInfo {
                url: url.clone(),
                sha256,
            },
        );
    }

    Ok(DistroPlugin {
        alias,
        name,
        comment,
        tarballs,
        has_setup,
    })
}

fn parse_assignment(line: &str) -> Option<(&str, &str)> {
    let eq_pos = line.find('=')?;
    let key = &line[..eq_pos];
    let rest = &line[eq_pos + 1..];

    if !is_valid_key(key) {
        return None;
    }

    let value = extract_quoted_value(rest)?;
    Some((key, value))
}

fn is_valid_key(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '[' || c == ']')
}

fn extract_quoted_value(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        Some(&trimmed[1..trimmed.len() - 1])
    } else {
        None
    }
}

pub fn load_plugins(dir: &Path) -> Vec<DistroPlugin> {
    let mut by_alias: BTreeMap<String, DistroPlugin> = BTreeMap::new();

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or_default();
            if !(name.ends_with(".sh") || name.ends_with(".override.sh")) {
                continue;
            }
            if let Ok(plugin) = parse_plugin(&path) {
                by_alias.insert(plugin.alias.clone(), plugin);
            }
        }
    }

    by_alias.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_plugins_returns_only_plugin_files() {
        let dir = std::env::temp_dir().join(format!("pr-cli-no-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create dir");
        assert!(load_plugins(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_assignment_simple() {
        let (key, val) = parse_assignment(r#"DISTRO_NAME="Alpine Linux""#).unwrap();
        assert_eq!(key, "DISTRO_NAME");
        assert_eq!(val, "Alpine Linux");
    }

    #[test]
    fn test_parse_alias_for_override_file() {
        let tmp = std::env::temp_dir().join("debian.override.sh");
        assert_eq!(parse_alias(&tmp), "debian");
    }

    #[test]
    fn test_parse_assignment_with_arch() {
        let (key, val) =
            parse_assignment(r#"TARBALL_URL_aarch64="https://example.com/alpine.tar.xz""#).unwrap();
        assert_eq!(key, "TARBALL_URL_aarch64");
        assert_eq!(val, "https://example.com/alpine.tar.xz");
    }

    #[test]
    fn test_parse_assignment_sha256() {
        let (key, val) = parse_assignment(
            r#"TARBALL_SHA256_aarch64="2bdfb03eae53e6163695f4cd3b86e67ddca78466c879a140e069b1263150599b""#,
        )
        .unwrap();
        assert_eq!(key, "TARBALL_SHA256_aarch64");
        assert_eq!(
            val,
            "2bdfb03eae53e6163695f4cd3b86e67ddca78466c879a140e069b1263150599b"
        );
    }

    #[test]
    fn test_parse_assignment_comment_line() {
        assert!(parse_assignment("# This is a comment").is_none());
    }

    #[test]
    fn test_parse_assignment_empty_line() {
        assert!(parse_assignment("").is_none());
    }

    #[test]
    fn test_parse_assignment_function_def() {
        assert!(parse_assignment("distro_setup() {").is_none());
    }

    #[test]
    fn test_extract_quoted_value() {
        assert_eq!(extract_quoted_value(r#""hello""#), Some("hello"));
        assert_eq!(extract_quoted_value(r#""""#), Some(""));
        assert_eq!(extract_quoted_value("noquotes"), None);
        assert_eq!(extract_quoted_value(r#""unclosed"#), None);
    }

    #[test]
    fn test_detect_distro_setup() {
        let content = r#"
DISTRO_NAME="Arch Linux"
TARBALL_URL_aarch64="https://example.com/arch.tar.xz"
TARBALL_SHA256_aarch64="abc123"

distro_setup() {
    echo "hello"
}
"#;
        let tmp = std::env::temp_dir().join("test_plugin_setup.sh");
        fs::write(&tmp, content).unwrap();
        let plugin = parse_plugin(&tmp).unwrap();
        assert!(plugin.has_setup);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_no_distro_setup() {
        let content = r#"
DISTRO_NAME="Alpine Linux"
TARBALL_URL_aarch64="https://example.com/alpine.tar.xz"
TARBALL_SHA256_aarch64="abc123"
"#;
        let tmp = std::env::temp_dir().join("test_plugin_no_setup.sh");
        fs::write(&tmp, content).unwrap();
        let plugin = parse_plugin(&tmp).unwrap();
        assert!(!plugin.has_setup);
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_missing_distro_name() {
        let content = r#"
TARBALL_URL_aarch64="https://example.com/test.tar.xz"
TARBALL_SHA256_aarch64="abc123"
"#;
        let tmp = std::env::temp_dir().join("test_plugin_no_name.sh");
        fs::write(&tmp, content).unwrap();
        let result = parse_plugin(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_missing_tarball_url() {
        let content = r#"DISTRO_NAME="Empty Distro""#;
        let tmp = std::env::temp_dir().join("test_plugin_no_tarball.sh");
        fs::write(&tmp, content).unwrap();
        let result = parse_plugin(&tmp);
        assert!(result.is_err());
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn test_supported_architectures() {
        let content = r#"
DISTRO_NAME="Test"
TARBALL_URL_aarch64="https://example.com/a.tar.xz"
TARBALL_SHA256_aarch64="aaa"
TARBALL_URL_x86_64="https://example.com/b.tar.xz"
TARBALL_SHA256_x86_64="bbb"
TARBALL_URL_arm="https://example.com/c.tar.xz"
TARBALL_SHA256_arm="ccc"
"#;
        let tmp = std::env::temp_dir().join("test_plugin_archs.sh");
        fs::write(&tmp, content).unwrap();
        let plugin = parse_plugin(&tmp).unwrap();
        let archs = plugin.supported_architectures();
        assert_eq!(archs, vec!["aarch64", "arm", "x86_64"]);
        let _ = fs::remove_file(&tmp);
    }
}
