//! The release name, in one place.
//!
//! Baked in from the repo's `version.txt` at compile time so there's exactly
//! one place to bump it and no file to find at runtime. It is **trimmed here,
//! once, in const** rather than at every use: the file ends with a newline, and
//! a caller that forgets to strip it writes a subtly corrupt record — a PNG
//! text chunk with an embedded newline is the case that motivated this, and
//! nothing downstream would catch it. `str::trim_ascii_end` is const-stable, so
//! the invariant is the compiler's rather than a comment's.
//!
//! **Not `env!("CARGO_PKG_VERSION")`.** That reads `0.1.0` and has never been
//! bumped; `version.txt` is the version this project actually publishes under.

/// The release name, e.g. `α-0.5`. Already trimmed — never call `.trim()` on it.
pub const VERSION: &str = include_str!("../version.txt").trim_ascii_end();

/// The git commit this was built from, with `-dirty` when the compiled-in
/// source had uncommitted changes, or `unknown` without git. From `build.rs`.
///
/// The release name alone can't tell two builds apart — `α-0.4` stamped every
/// render for 102 commits — so anything that records what made a picture
/// records this too.
pub const COMMIT: &str = env!("FRACTURIZE_COMMIT");

/// That commit's date, `YYYY-MM-DD` — the commit's, not the build's, so the
/// same commit always stamps the same. Empty without git.
pub const COMMIT_DATE: &str = env!("FRACTURIZE_COMMIT_DATE");

/// Everything at once, for people: `α-0.5 (15698f9, 2026-09-27)`.
pub const BUILD: &str = env!("FRACTURIZE_BUILD");

#[cfg(test)]
mod tests {
    use super::{BUILD, COMMIT, COMMIT_DATE, VERSION};

    /// The whole point of trimming in const: no caller has to remember to.
    #[test]
    fn version_carries_no_whitespace() {
        assert_eq!(VERSION, VERSION.trim());
        assert!(!VERSION.is_empty(), "version.txt is empty");
    }

    #[test]
    fn the_build_stamp_names_the_release_and_the_commit() {
        assert!(BUILD.starts_with(VERSION), "{BUILD}");
        assert!(BUILD.contains(COMMIT), "{BUILD}");
        assert!(!COMMIT.is_empty());
        assert!(COMMIT_DATE.is_empty() || COMMIT_DATE.len() == 10, "{COMMIT_DATE}");
    }
}
