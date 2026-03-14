//! Missing File Resolution Modal Types
//!
//! Button, action, and context types are defined in mm-ui. This module
//! re-exports them and provides the mutation helper that needs PathResolver.

pub use mm_meta::views::health_modals::*;
pub use mm_ui::resolutions::missing_file::{
    MissingFileAction, MissingFileButton, MissingFileButtonCtx,
};

use mm_meta::paths::PathResolver;

/// Generate HardLink mutations for restorable files.
pub fn restore_mutations(data: &MissingFileModalData, resolver: &PathResolver) -> Vec<mm_meta::mutations::Mutation> {
    data.restorable
        .iter()
        .map(|f| {
            let source = resolver.resolve(std::path::Path::new(&f.library_path));
            let destination = resolver.resolve(std::path::Path::new(&f.corpus_path));
            mm_meta::mutations::Mutation::HardLink(
                mm_meta::mutations::file_ops::HardLinkMutation {
                    source,
                    destination,
                },
            )
        })
        .collect()
}
