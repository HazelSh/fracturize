//! Stamps the binary with the build it is: the release name from
//! `version.txt`, the git commit, whether the source had uncommitted changes,
//! and the commit's date. `src/version.rs` reads the results.
//!
//! The date is the *commit's*, not the build's, so building the same commit
//! twice stamps the same thing. "Dirty" counts only what is compiled in —
//! `src/`, `shaders/`, the manifests, `version.txt`, and this file — because a
//! scene edited in the working tree changes no render the binary makes.
//!
//! Without git (a source tarball) the commit reads `unknown` and nothing
//! fails: a stamp that says it doesn't know is still honest.

use std::process::Command;

/// What "dirty" and the rebuild triggers look at.
const SOURCES: &[&str] =
    &["src", "shaders", "Cargo.toml", "Cargo.lock", "version.txt", "build.rs"];

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    for path in SOURCES {
        println!("cargo:rerun-if-changed={path}");
    }
    // A commit, a checkout, or staging moves these without touching a source.
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
        println!("cargo:rerun-if-changed={dir}/packed-refs");
        if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{head}");
        }
    }

    let version = std::fs::read_to_string("version.txt").unwrap_or_default();
    let version = version.trim();
    let commit = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) => {
            let mut status = vec!["status", "--porcelain", "--"];
            status.extend_from_slice(SOURCES);
            let dirty = git(&status).is_some();
            if dirty { format!("{hash}-dirty") } else { hash }
        }
        None => "unknown".to_string(),
    };
    let date = git(&["log", "-1", "--format=%cs"]).unwrap_or_default();
    // Empty on a detached HEAD, which is also what a source tarball gets.
    let branch = git(&["symbolic-ref", "-q", "--short", "HEAD"]).unwrap_or_default();
    let full = if date.is_empty() {
        format!("{version} ({commit})")
    } else {
        format!("{version} ({commit}, {date})")
    };

    println!("cargo:rustc-env=FRACTURIZE_COMMIT={commit}");
    println!("cargo:rustc-env=FRACTURIZE_COMMIT_DATE={date}");
    println!("cargo:rustc-env=FRACTURIZE_BRANCH={branch}");
    println!("cargo:rustc-env=FRACTURIZE_BUILD={full}");
}
