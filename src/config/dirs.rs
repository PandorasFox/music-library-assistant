//! dirs.kdl — Source directory configuration parsing and serialization.

use anyhow::Result;
use std::path::PathBuf;
use std::fs;
use super::types::SourceDir;
use super::path_schema::parse_path_schema;
use mm_utils::get_config_dir;

/// Parse dirs.kdl into a Vec<SourceDir>.
///
/// Format:
/// ```kdl
/// dir "web/releases/bandcamp" {
///     library "music"
///     can-stash-dupes false
///     interior-dupes false
/// }
/// ```
pub fn parse_dirs_kdl(content: &str) -> Result<Vec<SourceDir>> {
    let doc: kdl::KdlDocument = content.parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse dirs.kdl: {}", e))?;

    let mut dirs = Vec::new();
    for node in doc.nodes() {
        if node.name().value() != "dir" {
            continue;
        }

        let path = node.entries().first()
            .and_then(|e| e.value().as_string())
            .map(PathBuf::from);

        if let Some(path) = path {
            let mut source = SourceDir {
                path,
                libraries: Vec::new(),
                can_stash_dupes: true,
                interior_dupes: true,
                path_schema: None,
                enable_acoustid: true,
            };

            if let Some(children) = node.children() {
                for child in children.nodes() {
                    match child.name().value() {
                        "library" => {
                            for entry in child.entries() {
                                if let Some(s) = entry.value().as_string() {
                                    source.libraries.push(s.to_string());
                                }
                            }
                        }
                        "can-stash-dupes" => {
                            if let Some(entry) = child.entries().first() {
                                source.can_stash_dupes = entry.value().as_bool().unwrap_or(true);
                            }
                        }
                        "interior-dupes" => {
                            if let Some(entry) = child.entries().first() {
                                source.interior_dupes = entry.value().as_bool().unwrap_or(true);
                            }
                        }
                        "enable-acoustid" => {
                            if let Some(entry) = child.entries().first() {
                                source.enable_acoustid = entry.value().as_bool().unwrap_or(true);
                            }
                        }
                        "path-schema" => {
                            if let Some(entry) = child.entries().first() {
                                if let Some(template) = entry.value().as_string() {
                                    match parse_path_schema(template) {
                                        Ok(schema) => source.path_schema = Some(schema),
                                        Err(e) => {
                                            crate::logging::log_general(format!(
                                                "[CONFIG] Warning: invalid path-schema for {:?}: {}",
                                                source.path, e
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }

            dirs.push(source);
        }
    }

    Ok(dirs)
}

/// Serialize source dirs to KDL text.
fn serialize_dirs_kdl(dirs: &[SourceDir]) -> String {
    let mut out = String::new();
    for (i, dir) in dirs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("dir \"{}\" {{\n", dir.path.display()));
        if !dir.libraries.is_empty() {
            let libs: Vec<String> = dir.libraries.iter().map(|l| format!("\"{}\"", l)).collect();
            out.push_str(&format!("    library {}\n", libs.join(" ")));
        }
        if !dir.can_stash_dupes {
            out.push_str("    can-stash-dupes false\n");
        }
        if !dir.interior_dupes {
            out.push_str("    interior-dupes false\n");
        }
        if !dir.enable_acoustid {
            out.push_str("    enable-acoustid false\n");
        }
        if let Some(ref schema) = dir.path_schema {
            out.push_str(&format!("    path-schema \"{}\"\n", schema.template));
        }
        out.push_str("}\n");
    }
    out
}

/// Write source dirs to dirs.kdl.
pub fn write_dirs_to_disk(dirs: &[SourceDir]) -> Result<()> {
    let config_dir = get_config_dir()?;
    let path = config_dir.join("dirs.kdl");
    let backup = config_dir.join("dirs.kdl.bak");
    if path.exists() {
        fs::copy(&path, &backup)?;
    }
    fs::write(&path, serialize_dirs_kdl(dirs))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dirs_kdl() {
        let kdl = r#"
dir "web/releases/bandcamp" {
    library "music"
    can-stash-dupes false
    interior-dupes false
}

dir "web/releases/indie" {
    library "music" "soundtracks"
}
"#;

        let dirs = parse_dirs_kdl(kdl).unwrap();
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].path, PathBuf::from("web/releases/bandcamp"));
        assert_eq!(dirs[0].libraries, vec!["music".to_string()]);
        assert!(!dirs[0].can_stash_dupes); // explicitly false
        assert!(!dirs[0].interior_dupes); // explicitly false
        assert_eq!(dirs[1].path, PathBuf::from("web/releases/indie"));
        assert_eq!(dirs[1].libraries, vec!["music".to_string(), "soundtracks".to_string()]);
        assert!(dirs[1].can_stash_dupes); // default true
        assert!(dirs[1].interior_dupes); // default true
    }

    #[test]
    fn test_dirs_kdl_round_trip() {
        let dirs = vec![
            SourceDir {
                path: PathBuf::from("web/releases/bandcamp"),
                libraries: vec!["music".to_string()],
                can_stash_dupes: false,
                interior_dupes: false,
                path_schema: None,
                enable_acoustid: true,
            },
            SourceDir {
                path: PathBuf::from("web/releases/indie"),
                libraries: vec!["music".to_string(), "soundtracks".to_string()],
                can_stash_dupes: true,
                interior_dupes: true,
                path_schema: None,
                enable_acoustid: true,
            },
        ];

        let serialized = serialize_dirs_kdl(&dirs);
        let reparsed = parse_dirs_kdl(&serialized).unwrap();
        assert_eq!(dirs, reparsed);
    }

    #[test]
    fn test_dirs_kdl_round_trip_with_schema() {
        let dirs = vec![
            SourceDir {
                path: PathBuf::from("web/releases/bandcamp"),
                libraries: vec!["music".to_string()],
                can_stash_dupes: true,
                interior_dupes: true,
                path_schema: Some(parse_path_schema("$LABEL/$CATALOGNUMBER/$ARTIST - $TITLE").unwrap()),
                enable_acoustid: true,
            },
        ];

        let serialized = serialize_dirs_kdl(&dirs);
        assert!(serialized.contains("path-schema"));
        let reparsed = parse_dirs_kdl(&serialized).unwrap();
        assert_eq!(dirs, reparsed);
    }
}
