//! Capture the build's git identity so the server can report what it actually is.
//!
//! THE CACHING TRAP: cargo runs a build script once and reuses its output until
//! something it declared changes. Without the `rerun-if-changed` lines below,
//! the sha captured on the first build is baked into every later binary — the
//! server then reports a build identity that is confidently wrong, which is
//! precisely the class of defect this whole feature exists to remove. `.git/HEAD`
//! changes on checkout; `.git/index` changes on stage, which is the cheapest
//! available proxy for "the tree moved".
//!
//! When git is unavailable (a source tarball, a vendored build) the variables are
//! emitted EMPTY, and the handler turns that into a stated absence rather than a
//! fabricated value.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty());

    println!("cargo:rustc-env=ROSHERA_BUILD_SHA={sha}");
    println!(
        "cargo:rustc-env=ROSHERA_BUILD_DIRTY={}",
        match dirty {
            Some(true) => "1",
            Some(false) => "0",
            None => "",
        }
    );
}
