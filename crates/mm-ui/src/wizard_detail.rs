//! Cache for lazily-loaded inode details in wizard panes.
//!
//! When a wizard pane opens, the platform requests detail for the inodes shown.
//! As details arrive asynchronously, the cache stores them for rendering.

use std::collections::{HashMap, HashSet};

use mm_meta::db_types::Zone;
use mm_meta::views::inode_detail::InodeDetail;

/// Cache for lazily-loaded inode details, shared across wizard panes.
pub struct WizardDetailCache {
    details: HashMap<i64, InodeDetail>,
    pending: HashSet<i64>,
    zone: Zone,
}

impl WizardDetailCache {
    pub fn new(zone: Zone) -> Self {
        Self {
            details: HashMap::new(),
            pending: HashSet::new(),
            zone,
        }
    }

    /// Request details for a set of inodes. Returns the subset not yet cached or pending.
    pub fn request(&mut self, inodes: &[i64]) -> Vec<i64> {
        let mut needed = Vec::new();
        for &inode in inodes {
            if !self.details.contains_key(&inode) && self.pending.insert(inode) {
                needed.push(inode);
            }
        }
        needed
    }

    /// Receive loaded details. Clears them from pending.
    pub fn receive(&mut self, details: Vec<InodeDetail>) {
        for detail in details {
            self.pending.remove(&detail.inode);
            self.details.insert(detail.inode, detail);
        }
    }

    /// Get cached detail for an inode, if loaded.
    pub fn get(&self, inode: i64) -> Option<&InodeDetail> {
        self.details.get(&inode)
    }

    /// Whether any requests are still pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Zone for the fetch request.
    pub fn zone(&self) -> Zone {
        self.zone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_detail(inode: i64) -> InodeDetail {
        InodeDetail {
            inode,
            path: format!("corpus/track_{inode}.flac"),
            file_type: "FLAC".into(),
            duration_ms: Some(180_000),
            bitrate_kbps: Some(900),
            sample_rate: Some(44100),
            file_size: 30_000_000,
            tags: vec![("ARTIST".into(), "Test".into())],
        }
    }

    #[test]
    fn request_returns_all_on_first_call() {
        let mut cache = WizardDetailCache::new(Zone::Corpus);
        let needed = cache.request(&[1, 2, 3]);
        assert_eq!(needed, vec![1, 2, 3]);
        assert!(cache.has_pending());
    }

    #[test]
    fn request_deduplicates_pending() {
        let mut cache = WizardDetailCache::new(Zone::Corpus);
        cache.request(&[1, 2]);
        let needed = cache.request(&[2, 3]);
        assert_eq!(needed, vec![3]);
    }

    #[test]
    fn request_skips_already_cached() {
        let mut cache = WizardDetailCache::new(Zone::Corpus);
        cache.receive(vec![make_detail(1)]);
        let needed = cache.request(&[1, 2]);
        assert_eq!(needed, vec![2]);
    }

    #[test]
    fn receive_clears_pending_and_caches() {
        let mut cache = WizardDetailCache::new(Zone::Corpus);
        cache.request(&[1, 2]);
        assert!(cache.has_pending());
        assert!(cache.get(1).is_none());

        cache.receive(vec![make_detail(1), make_detail(2)]);
        assert!(!cache.has_pending());
        assert_eq!(cache.get(1).unwrap().inode, 1);
        assert_eq!(cache.get(2).unwrap().path, "corpus/track_2.flac");
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let cache = WizardDetailCache::new(Zone::Corpus);
        assert!(cache.get(999).is_none());
    }

    #[test]
    fn zone_accessor() {
        let cache = WizardDetailCache::new(Zone::Inbox);
        assert_eq!(cache.zone(), Zone::Inbox);
    }

    #[test]
    fn empty_cache_not_pending() {
        let cache = WizardDetailCache::new(Zone::Corpus);
        assert!(!cache.has_pending());
    }
}
