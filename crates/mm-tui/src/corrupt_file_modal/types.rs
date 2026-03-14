//! Corrupt File Resolution Modal Types — re-exports from mm-ui.

pub use mm_meta::views::health_modals::{CorruptFileEntry, CorruptFileModalData};
pub use mm_ui::resolutions::corrupt_file::{
    CorruptButton, CorruptButtonCtx, CorruptFileAction, CorruptFileData, CorruptFileState,
    stash_and_drop_mutations,
};
