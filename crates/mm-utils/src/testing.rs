//! Test helper utilities.
//!
//! Provides the [`t!`] macro as a drop-in replacement for `.unwrap()` in tests.
//! On failure, `t!` panics with the source expression, file, and line number,
//! making test failures immediately diagnosable without a backtrace.
//!
//! ```rust,ignore
//! use mm_utils::t;
//!
//! let schema = t!(parse_path_schema("$ARTIST/$ALBUM"));
//! let encoded = t!(bincode::serialize(&schema));
//! let node = t!(doc.get_mut("opinions")); // also works on Option
//! ```

/// Trait powering the [`t!`] macro. Implemented for `Result<T, E>` and `Option<T>`.
pub trait TestUnwrap {
    type Output;
    fn test_unwrap(self, expr: &str, file: &str, line: u32) -> Self::Output;
}

impl<T, E: std::fmt::Debug> TestUnwrap for Result<T, E> {
    type Output = T;
    fn test_unwrap(self, expr: &str, file: &str, line: u32) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{}:{}: `{}` failed: {:?}", file, line, expr, e),
        }
    }
}

impl<T> TestUnwrap for Option<T> {
    type Output = T;
    fn test_unwrap(self, expr: &str, file: &str, line: u32) -> T {
        match self {
            Some(v) => v,
            None => panic!("{}:{}: `{}` was None", file, line, expr),
        }
    }
}

/// Unwrap for tests. Replaces `.unwrap()` with better panic diagnostics.
///
/// On failure, panics with the stringified expression, file path, and line number.
/// Works on both `Result<T, E>` and `Option<T>`.
///
/// ```rust,ignore
/// let val = t!(fallible_operation());
/// // Panic message on Err: "src/foo.rs:42: `fallible_operation()` failed: SomeError(...)"
/// // Panic message on None: "src/foo.rs:42: `fallible_operation()` was None"
/// ```
#[macro_export]
macro_rules! t {
    ($expr:expr) => {
        $crate::testing::TestUnwrap::test_unwrap($expr, stringify!($expr), file!(), line!())
    };
}
