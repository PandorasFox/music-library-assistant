//! Automatic deployment of deploy-ready files and stale library fixes.
//!
//! Offloads the DB query + soft mutation building to a blocking task.
//! When results arrive via the offload channel, the handler queues
//! phased soft mutations directly — bypassing the transaction system.

use mm_meta::soft_mutations::SoftMutation;

impl super::Witch {
    /// Request an async check for deploy-ready files (audio + leftovers + stale + sidecars).
    ///
    /// Fires from `transition_to_idle` — gated behind LINGER_DURATION + priority chain.
    /// Offloads the DB query to a blocking task.
    pub(super) fn request_auto_deploy_check(&self) {
        let config = match self.read_config(|c| c.clone()) {
            Some(c) => c,
            None => return,
        };
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let soft_mutations = auto_deploy_query_blocking(&config);
            let _ = tx.send(super::types::OffloadResult::AutoDeployResult { soft_mutations });
        });
    }

    /// Request an async sidecar-only deploy check.
    ///
    /// Fires eagerly from `transition_to_completed` — no linger, no priority chain.
    /// Cover art symlinks are independent of metadata fetch and packing.
    pub(super) fn request_sidecar_deploy_check(&self) {
        let config = match self.read_config(|c| c.clone()) {
            Some(c) => c,
            None => return,
        };
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let soft_mutations = sidecar_deploy_query_blocking(&config);
            let _ = tx.send(super::types::OffloadResult::SidecarDeployResult { soft_mutations });
        });
    }
}

/// Query for deploy-ready files and build soft mutations (blocking, off main thread).
///
/// Opens its own read-only DB connection, queries deploy signals, and builds
/// SoftMutation instances with absolute paths via PathResolver.
fn auto_deploy_query_blocking(config: &crate::config::Config) -> Vec<SoftMutation> {
    use mm_meta::paths::PathResolver;
    use std::path::Path;

    let db_path = match crate::config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let db = match crate::db::Database::open_read_only(&db_path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let read_db = crate::db::ReadOnlyDb::new(&db);
    let resolver = PathResolver::from_config(config);

    let mut result = Vec::new();

    // 1. Leftover library files → StashLibrary
    if let Ok(leftovers) = read_db.get_library_leftover_files() {
        for file in &leftovers {
            let path_rel = Path::new("libraries").join(&file.library_path);
            let path = resolver.resolve(&path_rel);
            result.push(SoftMutation::StashLibrary { path });
        }
    }

    // 2. Stale library files → DeployMove
    if let Ok(stale) = read_db.get_library_stale_files() {
        for file in &stale {
            let source_rel = Path::new("libraries").join(&file.library_path);
            let source = resolver.resolve(&source_rel);
            let dest_rel = Path::new("libraries").join(&file.expected_path);
            let destination = resolver.resolve(&dest_rel);
            result.push(SoftMutation::DeployMove {
                source,
                destination,
            });
        }
    }

    // 3. Deploy-ready files → DeployLink
    //    If the destination is already occupied by a different inode (superseded
    //    deployment from a conflict tiebreak change), stash the old file first.
    if let Ok(new_files) = read_db.get_deploy_ready_files() {
        for file in &new_files {
            if file.deploy_path.is_empty() || file.library_name.is_empty() {
                continue;
            }
            let source = resolver.resolve(Path::new(&file.corpus_path));
            let dest_rel = Path::new("libraries")
                .join(&file.library_name)
                .join(&file.deploy_path);
            let destination = resolver.resolve(&dest_rel);

            // Detect superseded deployments: destination occupied by a different inode.
            // This happens when duplicate corpus tracks compute the same deploy path
            // and the conflict tiebreak winner changes (e.g. new corpus files ingested).
            // The old winner's hard link still occupies the path but is no longer
            // classified as a leftover (library-side sees it as Healthy).
            if destination.exists() {
                use std::os::unix::fs::MetadataExt;
                let src_ino = std::fs::metadata(&source).ok().map(|m| m.ino());
                let dst_ino = std::fs::metadata(&destination).ok().map(|m| m.ino());
                if src_ino.is_some() && dst_ino.is_some() && src_ino != dst_ino {
                    crate::logging::log_general(format!(
                        "[AUTO-DEPLOY] Superseded deployment at {}: stashing occupant (src_ino={}, dst_ino={})",
                        destination.display(),
                        src_ino.unwrap(),
                        dst_ino.unwrap(),
                    ));
                    result.push(SoftMutation::StashLibrary {
                        path: destination.clone(),
                    });
                }
            }

            result.push(SoftMutation::DeployLink {
                source,
                destination,
            });
        }
    }

    // 4. Sidecar images → DeployLink
    if let Ok(sidecars) = read_db.get_sidecar_deploy_ready_signals() {
        for sidecar in &sidecars {
            let source = resolver.resolve(Path::new(&sidecar.path));
            let dest_rel = Path::new("libraries")
                .join(&sidecar.library_name)
                .join(&sidecar.deploy_path);
            let destination = resolver.resolve(&dest_rel);
            result.push(SoftMutation::DeployLink {
                source,
                destination,
            });
        }
    }

    if !result.is_empty() {
        crate::logging::log_general(format!(
            "[AUTO-DEPLOY] Found {} operations to auto-deploy",
            result.len()
        ));
    }

    result
}

/// Query for sidecar-only deploy-ready signals (blocking, off main thread).
///
/// Subset of `auto_deploy_query_blocking` — only handles sidecar image
/// deployment, skipping audio deploy, leftovers, and stale fixes.
fn sidecar_deploy_query_blocking(config: &crate::config::Config) -> Vec<SoftMutation> {
    use mm_meta::paths::PathResolver;
    use std::path::Path;

    let db_path = match crate::config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let db = match crate::db::Database::open_read_only(&db_path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let read_db = crate::db::ReadOnlyDb::new(&db);
    let resolver = PathResolver::from_config(config);

    let mut result = Vec::new();

    if let Ok(sidecars) = read_db.get_sidecar_deploy_ready_signals() {
        for sidecar in &sidecars {
            let source = resolver.resolve(Path::new(&sidecar.path));
            let dest_rel = Path::new("libraries")
                .join(&sidecar.library_name)
                .join(&sidecar.deploy_path);
            let destination = resolver.resolve(&dest_rel);
            result.push(SoftMutation::DeployLink {
                source,
                destination,
            });
        }
    }

    if !result.is_empty() {
        crate::logging::log_general(format!(
            "[SIDECAR-DEPLOY] Found {} sidecar images to deploy",
            result.len()
        ));
    }

    result
}
