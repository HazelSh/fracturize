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

/// The release name, e.g. `α-0.4`. Already trimmed — never call `.trim()` on it.
pub const VERSION: &str = include_str!("../version.txt").trim_ascii_end();

#[cfg(test)]
mod tests {
    use super::VERSION;

    /// The whole point of trimming in const: no caller has to remember to.
    #[test]
    fn version_carries_no_whitespace() {
        assert_eq!(VERSION, VERSION.trim());
        assert!(!VERSION.is_empty(), "version.txt is empty");
    }
}
