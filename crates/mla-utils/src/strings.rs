//! UTF-8-safe string utilities.
//!
//! Rust strings are UTF-8, and byte-based slicing will panic if the index falls
//! inside a multi-byte character. These helpers provide safe alternatives.

/// Find the largest valid char boundary at or before `index`.
///
/// If `index >= s.len()`, returns `s.len()`.
/// If `index` is already a char boundary, returns `index`.
/// Otherwise, scans backward to find the nearest valid boundary.
#[inline]
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else if s.is_char_boundary(index) {
        index
    } else {
        // Scan backwards to find a valid char boundary
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}

/// Find the smallest valid char boundary at or after `index`.
///
/// If `index >= s.len()`, returns `s.len()`.
/// If `index` is already a char boundary, returns `index`.
/// Otherwise, scans forward to find the nearest valid boundary.
#[inline]
pub fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else if s.is_char_boundary(index) {
        index
    } else {
        let mut i = index;
        while i < s.len() && !s.is_char_boundary(i) {
            i += 1;
        }
        i
    }
}

/// Safely slice a string from the start up to (but not including) `end`.
///
/// If `end` falls inside a multi-byte character, the slice ends at the
/// character boundary before `end` (floor behavior).
#[inline]
pub fn safe_slice_to(s: &str, end: usize) -> &str {
    let safe_end = floor_char_boundary(s, end);
    &s[..safe_end]
}

/// Safely slice a string from `start` to the end.
///
/// If `start` falls inside a multi-byte character, the slice begins at the
/// character boundary after `start` (ceil behavior).
#[inline]
pub fn safe_slice_from(s: &str, start: usize) -> &str {
    let safe_start = ceil_char_boundary(s, start);
    &s[safe_start..]
}

/// Safely slice a string from `start` to `end`.
///
/// Uses floor behavior for `end` and ceil behavior for `start`.
#[inline]
pub fn safe_slice(s: &str, start: usize, end: usize) -> &str {
    let safe_start = ceil_char_boundary(s, start);
    let safe_end = floor_char_boundary(s, end);
    if safe_start >= safe_end {
        ""
    } else {
        &s[safe_start..safe_end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_floor_char_boundary_ascii() {
        let s = "hello";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 5);
        assert_eq!(floor_char_boundary(s, 10), 5); // Beyond end
    }

    #[test]
    fn test_floor_char_boundary_utf8() {
        // "café" = c(1) a(1) f(1) é(2) = 5 bytes
        let s = "café";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 3), 3); // Before 'é'
        assert_eq!(floor_char_boundary(s, 4), 3); // Inside 'é', floor to before
        assert_eq!(floor_char_boundary(s, 5), 5); // After 'é'
    }

    #[test]
    fn test_floor_char_boundary_multibyte() {
        // "日本語" = 3 chars, 9 bytes (3 bytes each)
        let s = "日本語";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 0); // Inside first char
        assert_eq!(floor_char_boundary(s, 2), 0); // Inside first char
        assert_eq!(floor_char_boundary(s, 3), 3); // Start of second char
        assert_eq!(floor_char_boundary(s, 4), 3); // Inside second char
        assert_eq!(floor_char_boundary(s, 6), 6); // Start of third char
        assert_eq!(floor_char_boundary(s, 9), 9); // End
    }

    #[test]
    fn test_ceil_char_boundary_utf8() {
        let s = "café";
        assert_eq!(ceil_char_boundary(s, 0), 0);
        assert_eq!(ceil_char_boundary(s, 3), 3); // Before 'é'
        assert_eq!(ceil_char_boundary(s, 4), 5); // Inside 'é', ceil to after
        assert_eq!(ceil_char_boundary(s, 5), 5); // After 'é'
    }

    #[test]
    fn test_safe_slice_to() {
        let s = "café";
        assert_eq!(safe_slice_to(s, 3), "caf");
        assert_eq!(safe_slice_to(s, 4), "caf"); // Inside 'é', floors to before
        assert_eq!(safe_slice_to(s, 5), "café");
    }

    #[test]
    fn test_safe_slice_from() {
        let s = "café";
        assert_eq!(safe_slice_from(s, 3), "é");
        assert_eq!(safe_slice_from(s, 4), ""); // Inside 'é', ceils to after
        assert_eq!(safe_slice_from(s, 5), "");
    }

    #[test]
    fn test_safe_slice() {
        let s = "日本語";
        assert_eq!(safe_slice(s, 0, 3), "日");
        assert_eq!(safe_slice(s, 1, 4), ""); // start inside char 1, end inside char 2 → empty
        assert_eq!(safe_slice(s, 3, 6), "本");
        assert_eq!(safe_slice(s, 0, 9), "日本語");
    }

    #[test]
    fn test_corrupted_looking_string() {
        // The string from the crash: multi-byte chars that could cause slicing issues
        let s = "4ÃÂÃÂÃÂÃÂ°`ÃÂ£o Migue";
        // This should not panic regardless of where we slice
        for i in 0..=s.len() + 5 {
            let _ = floor_char_boundary(s, i);
            let _ = ceil_char_boundary(s, i);
            let _ = safe_slice_to(s, i);
            let _ = safe_slice_from(s, i);
        }
    }
}
