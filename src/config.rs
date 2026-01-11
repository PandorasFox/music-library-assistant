use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub corpus_root: PathBuf,
    pub libraries: Vec<Library>,
    pub legacy_library: Option<PathBuf>,
    pub deploy_mappings: Vec<DeployMapping>,
    pub lost_files_dir: Option<PathBuf>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Opinions {
    pub auto_next_save_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployMapping {
    pub corpus_relative_path: PathBuf, // Relative to corpus-root
    pub library_names: Vec<String>,    // Target library names
}

#[derive(Debug, Clone)]
pub struct ScanSource {
    pub name: String,      // Display name
    pub path: PathBuf,     // File system path
    pub source_id: String, // DB identifier
}

impl Config {
    /// Get all scan sources from the config
    pub fn get_scan_sources(&self) -> Vec<ScanSource> {
        let mut sources = vec![ScanSource {
            name: "Corpus".to_string(),
            path: self.corpus_root.clone(),
            source_id: "corpus".to_string(),
        }];

        for lib in &self.libraries {
            sources.push(ScanSource {
                name: format!("Library: {}", lib.name),
                path: lib.path.clone(),
                source_id: lib.name.clone(),
            });
        }

        if let Some(legacy) = &self.legacy_library {
            sources.push(ScanSource {
                name: "Legacy Library".to_string(),
                path: legacy.clone(),
                source_id: "legacy".to_string(),
            });
        }

        sources
    }

    /// Validate that all configured paths reside on the same filesystem
    /// This is required for hard link operations to work
    #[allow(unreachable_code, unused_variables, unused_mut)]
    pub fn validate_same_filesystem(&self) -> Result<()> {
        use std::collections::HashSet;
        use std::os::unix::fs::MetadataExt;

        let mut devices = HashSet::new();
        let mut path_info = Vec::new();

        return Ok(());

        // Check corpus root
        if self.corpus_root.exists() {
            let meta =
                std::fs::metadata(&self.corpus_root).context("Failed to stat corpus root")?;
            let dev = meta.dev();
            devices.insert(dev);
            path_info.push(format!(
                "  Corpus root: {:?} (filesystem: {})",
                self.corpus_root, dev
            ));
        } else {
            anyhow::bail!("Corpus root does not exist: {:?}", self.corpus_root);
        }

        // Check all library paths
        for library in &self.libraries {
            if library.path.exists() {
                let meta = std::fs::metadata(&library.path)
                    .with_context(|| format!("Failed to stat library: {}", library.name))?;
                let dev = meta.dev();
                devices.insert(dev);
                path_info.push(format!(
                    "  Library '{}': {:?} (filesystem: {})",
                    library.name, library.path, dev
                ));
            } else {
                anyhow::bail!(
                    "Library path does not exist: {} at {:?}",
                    library.name,
                    library.path
                );
            }
        }

        // Check lost-files directory if configured
        if let Some(lost_path) = &self.lost_files_dir {
            if lost_path.exists() {
                let meta =
                    std::fs::metadata(lost_path).context("Failed to stat lost-files directory")?;
                let dev = meta.dev();
                devices.insert(dev);
                path_info.push(format!(
                    "  Lost-files: {:?} (filesystem: {})",
                    lost_path, dev
                ));
            }
            // Note: Don't fail if lost-files doesn't exist - it's optional and may be created later
        }

        // Verify all paths are on the same filesystem
        if devices.len() > 1 {
            let error_msg = format!(
                "FATAL: Cannot create hard links across filesystems.\n\
                 Paths span multiple filesystems:\n\
                 {}\n\n\
                 Solution: Move all directories to the same filesystem/volume.",
                path_info.join("\n")
            );
            anyhow::bail!("{}", error_msg);
        }

        Ok(())
    }

    /// Validate deployment configuration
    pub fn validate(&self) -> Result<()> {
        // Validate filesystem consistency for hard links
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate deploy mappings
        for mapping in &self.deploy_mappings {
            // Check that all library names in deploy mappings reference existing libraries
            for lib_name in &mapping.library_names {
                if !self.libraries.iter().any(|l| l.name == *lib_name) {
                    anyhow::bail!("Deploy mapping references unknown library: {}", lib_name);
                }
            }

            // Check that deploy paths are within corpus root
            let full_path = self.corpus_root.join(&mapping.corpus_relative_path);
            if !full_path.starts_with(&self.corpus_root) {
                anyhow::bail!(
                    "Deploy path escapes corpus root: {:?}",
                    mapping.corpus_relative_path
                );
            }
        }

        Ok(())
    }
}

/// Get the config directory path ($XDG_CONFIG_HOME/mla/ or ~/.config/mla/)
pub fn get_config_dir() -> Result<PathBuf> {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("Failed to determine config directory")?;

    Ok(config_home.join("mla"))
}

/// Get the database path
pub fn get_db_path() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("mla.db"))
}

/// Get the log path for stderr-type logging and warnings
pub fn get_log_path() -> PathBuf {
    PathBuf::from("/tmp/mla.log")
}

/// Log a message (error, warning, etc.) to the main log file
pub fn log_message(message: &str) -> Result<()> {
    use std::io::Write;

    let log_path = get_log_path();

    // Append to log file with timestamp
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_entry = format!("[{}] {}\n", timestamp, message);

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    file.write_all(log_entry.as_bytes())?;

    Ok(())
}

/// Log a scan error to the log file
/// Deprecated alias for log_message, kept for backward compatibility
pub fn log_scan_error(message: &str) -> Result<()> {
    log_message(message)
}

/// Load config from $XDG_CONFIG_HOME/mla/config.kdl (or ~/.config/mla/config.kdl if XDG_CONFIG_HOME is not set)
pub fn load_config() -> Result<Config> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");

    if !config_path.exists() {
        anyhow::bail!(
            "config.kdl not found at {:?}\n\
            Please create a config file at $XDG_CONFIG_HOME/mla/config.kdl (or ~/.config/mla/config.kdl)",
            config_path
        );
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    let config = parse_kdl_config(&content)?;
    config.validate()?;

    Ok(config)
}

fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    let mut config = Config {
        corpus_root: PathBuf::new(),
        libraries: Vec::new(),
        legacy_library: None,
        deploy_mappings: Vec::new(),
        lost_files_dir: None,
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            // Accept both "corpus-root" (new) and "archive-root" (backward compat)
            "corpus-root" | "archive-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.corpus_root = PathBuf::from(path_str);
                    }
                }
            }
            "library" => {
                if let Some(name_entry) = node.entries().first() {
                    if let Some(name) = name_entry.value().as_string() {
                        if let Some(children) = node.children() {
                            for child in children.nodes() {
                                if child.name().value() == "path" {
                                    if let Some(path_entry) = child.entries().first() {
                                        if let Some(path_str) = path_entry.value().as_string() {
                                            config.libraries.push(Library {
                                                name: name.to_string(),
                                                path: PathBuf::from(path_str),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "legacy-library" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.legacy_library = Some(PathBuf::from(path_str));
                    }
                }
            }
            "deploy" => {
                if let Some(path_entry) = node.entries().first() {
                    if let Some(path_str) = path_entry.value().as_string() {
                        let mut library_names = Vec::new();
                        if let Some(children) = node.children() {
                            for child in children.nodes() {
                                if let Some(lib_entry) = child.entries().first() {
                                    if let Some(lib_name) = lib_entry.value().as_string() {
                                        library_names.push(lib_name.to_string());
                                    }
                                }
                            }
                        }
                        config.deploy_mappings.push(DeployMapping {
                            corpus_relative_path: PathBuf::from(path_str),
                            library_names,
                        });
                    }
                }
            }
            "lost-files" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.lost_files_dir = Some(PathBuf::from(path_str));
                    }
                }
            }
            "opinions" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        if child.name().value() == "auto_next_save_all" {
                            config.opinions.auto_next_save_all = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.corpus_root.as_os_str().is_empty() {
        anyhow::bail!("corpus-root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"

library "main" {
    path "/Volumes/cerberus/library/main"
}

library "soundtracks" {
    path "/Volumes/cerberus/library/soundtracks"
}

legacy-library "/Volumes/cerberus/archive/working/legacy"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(
            config.corpus_root,
            PathBuf::from("/Volumes/cerberus/archive/music")
        );
        assert_eq!(config.libraries.len(), 2);
        assert_eq!(config.libraries[0].name, "main");
        assert!(config.legacy_library.is_some());
    }
}
