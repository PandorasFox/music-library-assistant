//! Automatic deployment of deploy-ready files and stale library fixes.
//!
//! Offloads the DB query + soft mutation building to a blocking task.
//! When results arrive via the offload channel, the handler queues
//! phased soft mutations directly — bypassing the transaction system.

use mm_meta::soft_mutations::SoftMutation;

impl super::Witch {
    /// Request an async check for deploy-ready files.
    ///
    /// Offloads the DB query to a blocking task. When results arrive via the
    /// offload channel, the handler queues soft mutations for execution.
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
