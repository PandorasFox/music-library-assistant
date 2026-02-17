//! Recomputation scope for selective content analysis.
//!
//! After a mutation completes, it declares which domains it dirtied via
//! `RecomputationScope`. The Witch accumulates scopes across a session,
//! and `ScheduleContentAnalysis` uses the accumulated scope to filter
//! which content analysis computations to spawn.
//!
//! This avoids the wasteful "re-run everything" pattern where a tag edit
//! would trigger fingerprint overlap detection, or a config tweak to the
//! vacuum threshold would trigger full re-awakening.

use std::ops::{BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

/// Bitmask of domains that a mutation dirtied.
///
/// Used by `ScheduleContentAnalysis` to filter which computations to spawn:
/// - `None` scope (startup) → run all computations
/// - `Some(scope)` → run only computations touching the flagged domains
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecomputationScope(u8);

impl RecomputationScope {
    /// No domains dirtied. Mutations with EMPTY scope skip re-awakening entirely.
    pub const EMPTY: Self = Self(0);
    /// Tag data changed (tag edits, canonicalization, tag sync).
    pub const TAGS: Self = Self(1 << 0);
    /// File presence, paths, or audio identity changed (indexing, moves, transcodes).
    pub const FILES: Self = Self(1 << 1);
    /// Library deployment state changed (hard links, library moves, stashing).
    pub const DEPLOY: Self = Self(1 << 2);
    /// Inbox state changed (inbox-to-corpus moves, inbox tag edits).
    pub const INBOX: Self = Self(1 << 3);

    /// True if no domains are flagged.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True if `self` contains all bits in `other`.
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// True if `self` overlaps with any of the given flags.
    pub fn touches_any(self, flags: &[Self]) -> bool {
        flags.iter().any(|f| self.contains(*f))
    }
}

impl BitOr for RecomputationScope {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for RecomputationScope {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        assert!(RecomputationScope::EMPTY.is_empty());
        assert!(!RecomputationScope::TAGS.is_empty());
    }

    #[test]
    fn test_contains() {
        let scope = RecomputationScope::TAGS | RecomputationScope::FILES;
        assert!(scope.contains(RecomputationScope::TAGS));
        assert!(scope.contains(RecomputationScope::FILES));
        assert!(!scope.contains(RecomputationScope::DEPLOY));
    }

    #[test]
    fn test_touches_any() {
        let scope = RecomputationScope::TAGS;
        assert!(scope.touches_any(&[RecomputationScope::TAGS, RecomputationScope::FILES]));
        assert!(!scope.touches_any(&[RecomputationScope::FILES, RecomputationScope::DEPLOY]));
    }

    #[test]
    fn test_bitor_assign() {
        let mut scope = RecomputationScope::TAGS;
        scope |= RecomputationScope::DEPLOY;
        assert!(scope.contains(RecomputationScope::TAGS));
        assert!(scope.contains(RecomputationScope::DEPLOY));
        assert!(!scope.contains(RecomputationScope::FILES));
    }
}
