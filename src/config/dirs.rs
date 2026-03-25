//! dirs.kdl — Source directory configuration parsing and serialization.

use super::path_schema::parse_path_schema;
use super::types::{CoverArtSanctity, SourceDir};
use anyhow::Result;
use mm_utils::get_config_dir;
use std::fs;
use std::path::PathBuf;

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
    let doc: kdl::KdlDocument = content
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse dirs.kdl: {}", e))?;

    let mut dirs = Vec::new();
    for node in doc.nodes() {
        if node.name().value() != "dir" {
            continue;
        }

        let path = node
            .entries()
            .first()
            .and_then(|e| e.value().as_string())
            .map(PathBuf::from);

        if let Some(path) = path {
            let mut source = SourceDir {
                path,
                libraries: Vec::new(),
                can_stash_dupes: None,
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
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
                                source.can_stash_dupes = entry.value().as_bool();
                            }
                        }
                        "interior-dupes" => {
                            if let Some(entry) = child.entries().first() {
                                source.interior_dupes = entry.value().as_bool();
                            }
                        }
                        "enable-acoustid" => {
                            if let Some(entry) = child.entries().first() {
                                source.enable_acoustid = entry.value().as_bool();
                            }
                        }
                        "pinned-release" => {
                            if let Some(entry) = child.entries().first() {
                                if let Some(s) = entry.value().as_string() {
                                    source.pinned_release = Some(s.to_string());
                                }
                            }
                        }
                        "cover-art-sanctity" => {
                            if let Some(entry) = child.entries().first() {
                                if let Some(s) = entry.value().as_string() {
                                    source.cover_art_sanctity = CoverArtSanctity::from_str(s);
                                }
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
///
/// Default entries (no libraries, all bools at defaults, no schema) are elided —
/// an empty config blob carries no information and needn't be persisted.
fn serialize_dirs_kdl(dirs: &[SourceDir]) -> String {
    let mut out = String::new();
    let non_default: Vec<_> = dirs.iter().filter(|d| !d.is_default()).collect();
    for (i, dir) in non_default.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("dir \"{}\" {{\n", dir.path.display()));
        if !dir.libraries.is_empty() {
            let libs: Vec<String> = dir.libraries.iter().map(|l| format!("\"{}\"", l)).collect();
            out.push_str(&format!("    library {}\n", libs.join(" ")));
        }
        if let Some(v) = dir.can_stash_dupes {
            out.push_str(&format!("    can-stash-dupes {}\n", v));
        }
        if let Some(v) = dir.interior_dupes {
            out.push_str(&format!("    interior-dupes {}\n", v));
        }
        if let Some(v) = dir.enable_acoustid {
            out.push_str(&format!("    enable-acoustid {}\n", v));
        }
        if let Some(ref schema) = dir.path_schema {
            out.push_str(&format!("    path-schema \"{}\"\n", schema.template));
        }
        if let Some(ref release_id) = dir.pinned_release {
            out.push_str(&format!("    pinned-release \"{}\"\n", release_id));
        }
        if let Some(sanctity) = dir.cover_art_sanctity {
            out.push_str(&format!("    cover-art-sanctity \"{}\"\n", sanctity.as_str()));
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
    use mm_utils::t;

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

        let dirs = t!(parse_dirs_kdl(kdl));
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].path, PathBuf::from("web/releases/bandcamp"));
        assert_eq!(dirs[0].libraries, vec!["music".to_string()]);
        assert_eq!(dirs[0].can_stash_dupes, Some(false)); // explicitly false
        assert_eq!(dirs[0].interior_dupes, Some(false)); // explicitly false
        assert_eq!(dirs[1].path, PathBuf::from("web/releases/indie"));
        assert_eq!(
            dirs[1].libraries,
            vec!["music".to_string(), "soundtracks".to_string()]
        );
        assert_eq!(dirs[1].can_stash_dupes, None); // not set = inherit
        assert_eq!(dirs[1].interior_dupes, None); // not set = inherit
    }

    #[test]
    fn test_dirs_kdl_round_trip() {
        let dirs = vec![
            SourceDir {
                path: PathBuf::from("web/releases/bandcamp"),
                libraries: vec!["music".to_string()],
                can_stash_dupes: Some(false),
                interior_dupes: Some(false),
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
            SourceDir {
                path: PathBuf::from("web/releases/indie"),
                libraries: vec!["music".to_string(), "soundtracks".to_string()],
                can_stash_dupes: None,
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
        ];

        let serialized = serialize_dirs_kdl(&dirs);
        let reparsed = t!(parse_dirs_kdl(&serialized));
        assert_eq!(dirs, reparsed);
    }

    #[test]
    fn test_dirs_kdl_round_trip_explicit_true() {
        // Explicitly-set true must survive round-trip (not collapse to None)
        let dirs = vec![SourceDir {
            path: PathBuf::from("web/releases/bandcamp"),
            libraries: vec!["music".to_string()],
            can_stash_dupes: Some(true),
            interior_dupes: Some(true),
            path_schema: None,
            enable_acoustid: Some(true),
            pinned_release: None,
            cover_art_sanctity: None,
        }];

        let serialized = serialize_dirs_kdl(&dirs);
        assert!(serialized.contains("can-stash-dupes true"));
        assert!(serialized.contains("interior-dupes true"));
        assert!(serialized.contains("enable-acoustid true"));
        let reparsed = t!(parse_dirs_kdl(&serialized));
        assert_eq!(dirs, reparsed);
    }

    #[test]
    fn test_dirs_kdl_round_trip_with_schema() {
        let dirs = vec![SourceDir {
            path: PathBuf::from("web/releases/bandcamp"),
            libraries: vec!["music".to_string()],
            can_stash_dupes: None,
            interior_dupes: None,
            path_schema: Some(t!(parse_path_schema("$LABEL/$CATALOGNUMBER/$ARTIST - $TITLE"))),
            enable_acoustid: None,
            pinned_release: None,
            cover_art_sanctity: None,
        }];

        let serialized = serialize_dirs_kdl(&dirs);
        assert!(serialized.contains("path-schema"));
        let reparsed = t!(parse_dirs_kdl(&serialized));
        assert_eq!(dirs, reparsed);
    }

    #[test]
    fn test_default_entries_elided_on_serialize() {
        let dirs = vec![
            // Non-default: has libraries
            SourceDir {
                path: PathBuf::from("web/releases/bandcamp"),
                libraries: vec!["music".to_string()],
                can_stash_dupes: None,
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
            // Default: all None, carries no information
            SourceDir {
                path: PathBuf::from("web/releases/empty"),
                libraries: vec![],
                can_stash_dupes: None,
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
            // Non-default: has an explicit bool
            SourceDir {
                path: PathBuf::from("web/releases/nodupe"),
                libraries: vec![],
                can_stash_dupes: Some(false),
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
        ];

        assert!(dirs[1].is_default());
        assert!(!dirs[0].is_default());
        assert!(!dirs[2].is_default());

        let serialized = serialize_dirs_kdl(&dirs);
        assert!(serialized.contains("bandcamp"));
        assert!(
            !serialized.contains("empty"),
            "default entry should be elided"
        );
        assert!(serialized.contains("nodupe"));

        let reparsed = t!(parse_dirs_kdl(&serialized));
        assert_eq!(reparsed.len(), 2);
    }

    /// Helper: build a minimal Config with the given source_dirs for testing resolution.
    fn config_with_dirs(dirs: Vec<SourceDir>) -> crate::config::Config {
        crate::config::Config {
            storage_root: PathBuf::from("/test"),
            libraries_root: None,
            stash_root: None,
            source_dirs: dirs,
            opinions: mm_meta::config::Opinions::default(),
        }
    }

    #[test]
    fn test_resolve_inherits_bool_from_parent() {
        // Parent sets can_stash_dupes = false.
        // Child only sets interior_dupes = false, doesn't set can_stash_dupes.
        // Resolution for child should inherit can_stash_dupes = false from parent.
        let config = config_with_dirs(vec![
            SourceDir {
                path: PathBuf::from("incoming"),
                libraries: vec!["music".into()],
                can_stash_dupes: Some(false),
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
            SourceDir {
                path: PathBuf::from("incoming/subdir"),
                libraries: vec![],
                can_stash_dupes: None,
                interior_dupes: Some(false),
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
        ]);

        let resolved = t!(config
            .resolve_source_config(std::path::Path::new("incoming/subdir/album/track.flac")));

        assert_eq!(resolved.source_path, PathBuf::from("incoming/subdir"));
        assert_eq!(
            resolved.can_stash_dupes, false,
            "should inherit parent's false"
        );
        assert_eq!(resolved.interior_dupes, false, "explicitly set on child");
        assert_eq!(
            resolved.libraries,
            vec!["music".to_string()],
            "should inherit parent's libraries"
        );
    }

    #[test]
    fn test_resolve_child_overrides_parent() {
        // Parent sets can_stash_dupes = false.
        // Child explicitly sets can_stash_dupes = true.
        // Resolution should use child's explicit true.
        let config = config_with_dirs(vec![
            SourceDir {
                path: PathBuf::from("incoming"),
                libraries: vec!["music".into()],
                can_stash_dupes: Some(false),
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
            SourceDir {
                path: PathBuf::from("incoming/override"),
                libraries: vec![],
                can_stash_dupes: Some(true),
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
                cover_art_sanctity: None,
            },
        ]);

        let resolved = t!(config
            .resolve_source_config(std::path::Path::new("incoming/override/album/track.flac")));

        assert_eq!(
            resolved.can_stash_dupes, true,
            "child's explicit true overrides parent's false"
        );
    }

    #[test]
    fn test_resolve_defaults_when_no_explicit_value() {
        // Single dir with no explicit bools — should get system defaults (true).
        let config = config_with_dirs(vec![SourceDir {
            path: PathBuf::from("web"),
            libraries: vec![],
            can_stash_dupes: None,
            interior_dupes: None,
            path_schema: None,
            enable_acoustid: None,
            pinned_release: None,
            cover_art_sanctity: None,
        }]);

        let resolved = t!(config
            .resolve_source_config(std::path::Path::new("web/releases/track.flac")));

        assert_eq!(resolved.can_stash_dupes, true, "system default");
        assert_eq!(resolved.interior_dupes, true, "system default");
    }

    #[test]
    fn test_resolve_no_match() {
        let config = config_with_dirs(vec![SourceDir {
            path: PathBuf::from("web"),
            libraries: vec![],
            can_stash_dupes: None,
            interior_dupes: None,
            path_schema: None,
            enable_acoustid: None,
            pinned_release: None,
            cover_art_sanctity: None,
        }]);

        assert!(config
            .resolve_source_config(std::path::Path::new("other/track.flac"))
            .is_none());
    }

    #[test]
    fn test_resolve_db_path_zone_relative() {
        let config = config_with_dirs(vec![SourceDir {
            path: PathBuf::from("web"),
            libraries: vec!["music".into()],
            can_stash_dupes: None,
            interior_dupes: None,
            path_schema: None,
            enable_acoustid: None,
            pinned_release: None,
            cover_art_sanctity: None,
        }]);

        // DB paths are now zone-relative (no "corpus/" prefix)
        let resolved = t!(config
            .resolve_source_config_for_db_path("web/track.flac"));
        assert_eq!(resolved.source_path, PathBuf::from("web"));
        assert_eq!(resolved.libraries, vec!["music".to_string()]);
    }
}
