use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub corpus_root: PathBuf,
    pub libraries_root: PathBuf,
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
pub struct DeployMapping {
    pub corpus_relative_paths: Vec<PathBuf>, // Multiple paths, relative to corpus-root
    pub library_names: Vec<String>,          // Target library names
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
            name: "corpus".to_string(),
            path: self.corpus_root.clone(),
            source_id: "corpus".to_string(),
        }];

        // Enumerate library subdirectories dynamically
        if let Ok(entries) = fs::read_dir(&self.libraries_root) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            // Skip hidden directories
                            if !name.starts_with('.') {
                                sources.push(ScanSource {
                                    name: format!("Library: {}", name),
                                    path: entry.path(),
                                    source_id: name.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        if let Some(legacy) = &self.legacy_library {
            sources.push(ScanSource {
                name: "legacy".to_string(),
                path: legacy.clone(),
                source_id: "legacy".to_string(),
            });
        }

        sources
    }

    /// Validate that all configured paths exist and support required operations
    /// Tests hard link capability for deployment and atomic moves for lost-files
    pub fn validate_same_filesystem(&self) -> Result<()> {
        use std::fs;
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Step 1: Validate corpus_root exists
        if !self.corpus_root.exists() {
            anyhow::bail!(
                "Validation failed: corpus-root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.corpus_root
            );
        }

        // Step 2: Validate libraries_root exists
        if !self.libraries_root.exists() {
            anyhow::bail!(
                "Validation failed: libraries-root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.libraries_root
            );
        }

        // Step 3: Validate lost_files_dir exists (if configured)
        if let Some(lost_path) = &self.lost_files_dir {
            if !lost_path.exists() {
                anyhow::bail!(
                    "Validation failed: lost-files directory does not exist\n\
                     Path: {:?}\n\
                     \n\
                     Please create this directory or update config.kdl",
                    lost_path
                );
            }
        }

        // Step 4: Test hard link capability (corpus → libraries)
        let temp_file = NamedTempFile::new_in(&self.corpus_root)
            .context("Failed to create test file in corpus-root")?;
        temp_file.as_file().write_all(b"hardlink_test")?;

        let link_name = format!(
            "mla-hardlink-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let link_path = self.libraries_root.join(&link_name);

        if let Err(e) = fs::hard_link(temp_file.path(), &link_path) {
            anyhow::bail!(
                "Validation failed: Cannot create hard links\n\
                 \n\
                 Corpus root: {:?}\n\
                 Libraries root: {:?}\n\
                 \n\
                 Error: {}\n\
                 \n\
                 EXPLANATION:\n\
                 MLA's deployment requires hard link support between these directories.\n\
                 \n\
                 COMMON CAUSES:\n\
                 - Directories are on different filesystems/volumes\n\
                 - Filesystem doesn't support hard links (FAT32, exFAT, network mounts)\n\
                 \n\
                 SOLUTION:\n\
                 Move both directories to the same filesystem/volume.",
                self.corpus_root,
                self.libraries_root,
                e
            );
        }
        let _ = fs::remove_file(&link_path); // Cleanup

        // Step 5: Test atomic move capability (corpus → lost-files)
        if let Some(lost_path) = &self.lost_files_dir {
            let temp_file2 = NamedTempFile::new_in(&self.corpus_root)
                .context("Failed to create test file in corpus-root")?;

            // Persist the temp file so it doesn't get deleted when we convert it
            let source_path = temp_file2.into_temp_path();

            let move_name = format!(
                "mla-move-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let move_path = lost_path.join(&move_name);

            if let Err(e) = fs::rename(&source_path, &move_path) {
                // Cleanup source file if rename failed
                let _ = fs::remove_file(&source_path);

                anyhow::bail!(
                    "Validation failed: Cannot atomically move files\n\
                     \n\
                     Corpus root: {:?}\n\
                     Lost files: {:?}\n\
                     \n\
                     Error: {}\n\
                     \n\
                     EXPLANATION:\n\
                     MLA needs atomic moves between these directories.\n\
                     Atomic moves only work on the same filesystem.\n\
                     \n\
                     SOLUTION:\n\
                     Move lost-files to the same filesystem as corpus-root.",
                    self.corpus_root,
                    lost_path,
                    e
                );
            }
            let _ = fs::remove_file(&move_path); // Cleanup
        }

        // Step 6: Log success
        log_message("Filesystem validation passed")?;
        Ok(())
    }

    /// Validate deployment configuration
    pub fn validate(&self) -> Result<()> {
        // Validate filesystem consistency for hard links
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate deploy mappings
        for mapping in &self.deploy_mappings {
            // Note: We can't validate library names here because libraries are
            // discovered at runtime by scanning libraries_root subdirectories

            // Check that deploy paths are within corpus root
            for corpus_path in &mapping.corpus_relative_paths {
                let full_path = self.corpus_root.join(corpus_path);
                if !full_path.starts_with(&self.corpus_root) {
                    anyhow::bail!("Deploy path escapes corpus root: {:?}", corpus_path);
                }
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

/// Get the data directory path ($XDG_DATA_HOME/mla/ or ~/.local/share/mla/)
pub fn get_data_dir() -> Result<PathBuf> {
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .context("Failed to determine data directory")?;

    let data_dir = data_home.join("mla");

    // Ensure the directory exists
    if !data_dir.exists() {
        fs::create_dir_all(&data_dir).context("Failed to create data directory")?;
    }

    Ok(data_dir)
}

/// Get the database path (~/.local/share/mla/mla.db)
pub fn get_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("mla.db"))
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

    // Check for old format with library blocks
    let has_old_library_format = doc.nodes().iter().any(|n| n.name().value() == "library");

    if has_old_library_format {
        anyhow::bail!(
            "Old config format detected: 'library' blocks are no longer supported\n\
             \n\
             MIGRATION REQUIRED:\n\
             \n\
             OLD FORMAT:\n\
             library \"music\" {{\n\
                 path \"/archive/libraries/music\"\n\
             }}\n\
             \n\
             NEW FORMAT:\n\
             libraries-root \"/archive/libraries\"\n\
             \n\
             Libraries are now discovered by scanning subdirectories under libraries-root.\n\
             Directory names become library names (e.g., /archive/libraries/music → 'music').\n\
             \n\
             DEPLOY FORMAT ALSO CHANGED to support multiple corpus paths:\n\
             \n\
             OLD:\n\
             deploy \"web/releases/bandcamp\" {{\n\
                 library \"music\"\n\
             }}\n\
             \n\
             NEW:\n\
             deploy \"web/releases/bandcamp\" \"web/releases/itunes\" {{\n\
                 library \"music\"\n\
             }}"
        );
    }

    let mut config = Config {
        corpus_root: PathBuf::new(),
        libraries_root: PathBuf::new(),
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
            "libraries-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.libraries_root = PathBuf::from(path_str);
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
                let mut corpus_paths = Vec::new();

                // Collect all path arguments (multiple corpus paths supported)
                for entry in node.entries() {
                    if let Some(path_str) = entry.value().as_string() {
                        corpus_paths.push(PathBuf::from(path_str));
                    }
                }

                // Parse library names from children
                let mut library_names = Vec::new();
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        if child.name().value() == "library" {
                            if let Some(lib_entry) = child.entries().first() {
                                if let Some(lib_name) = lib_entry.value().as_string() {
                                    library_names.push(lib_name.to_string());
                                }
                            }
                        }
                    }
                }

                if !corpus_paths.is_empty() {
                    config.deploy_mappings.push(DeployMapping {
                        corpus_relative_paths: corpus_paths,
                        library_names,
                    });
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

    if config.libraries_root.as_os_str().is_empty() {
        anyhow::bail!("libraries-root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_new_format() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
libraries-root "/Volumes/cerberus/library"
lost-files "/Volumes/cerberus/archive/lost"

deploy "web/releases/bandcamp" "web/releases/itunes" {
    library "main"
}

legacy-library "/Volumes/cerberus/archive/working/legacy"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(
            config.corpus_root,
            PathBuf::from("/Volumes/cerberus/archive/music")
        );
        assert_eq!(
            config.libraries_root,
            PathBuf::from("/Volumes/cerberus/library")
        );
        assert_eq!(config.deploy_mappings.len(), 1);
        assert_eq!(config.deploy_mappings[0].corpus_relative_paths.len(), 2);
        assert_eq!(config.deploy_mappings[0].library_names[0], "main");
        assert!(config.legacy_library.is_some());
        assert!(config.lost_files_dir.is_some());
    }

    #[test]
    fn test_old_format_rejected() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"

library "main" {
    path "/Volumes/cerberus/library/main"
}
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Old config format"));
        assert!(err_msg.contains("MIGRATION REQUIRED"));
    }

    #[test]
    fn test_missing_libraries_root() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("libraries-root not specified"));
    }
}
