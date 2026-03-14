//! Browser entry marker types.
//!
//! DeployMarker and PackingMarker are used by the corpus browser variant
//! to decorate BrowserEntry (mm-ui) entries at render time.

use mm_meta::signals::packing_category::PackingCategory;

/// Deploy marker for a browser entry, distinguishing source roots from inherited dirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployMarker {
    /// Not under any configured deployment source.
    None,
    /// Exact source directory root — C opens config panel here.
    SourceRoot,
    /// Under a source directory (inherited deployment config).
    Inherited,
}

/// Packing marker for a browser entry, showing MusicBrainz release match status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PackingMarker {
    #[default]
    None,
    /// File is mapped to a MusicBrainz recording via release packing.
    Matched,
    /// Directory contains matched files with this best category.
    Directory(PackingCategory),
}
