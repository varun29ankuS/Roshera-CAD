//! Capture the build's git identity so the server can report what it actually is.
//!
//! THE CACHING TRAP: cargo runs a build script once and reuses its output until
//! something it DECLARED (via `rerun-if-changed`) changes. Get the declared set
//! wrong and the values captured on the first build are baked into every later
//! binary — the server then reports a build identity that is confidently
//! wrong, which is precisely the class of defect this whole feature exists to
//! remove.
//!
//! ─── THE SHA: four git paths, each for a distinct write ────────────────────
//!
//! - `.git/HEAD` — changes on checkout/switch to a different branch, and on
//!   any commit made while HEAD is *detached* (there HEAD holds the sha
//!   directly, so committing rewrites it in place).
//! - `.git/index` — changes on `git add` / `git commit -a` / `git rm`, i.e.
//!   whenever the staging area itself is touched.
//! - `.git/logs/HEAD` — the reflog. Appended on every operation that moves
//!   HEAD: commit, checkout, merge, reset, rebase, cherry-pick, amend. This
//!   is the one that closes the gap the other three miss: an ordinary
//!   `git commit` **on a branch** (not detached) updates neither `.git/HEAD`
//!   (which just holds `ref: refs/heads/<branch>`, unchanged) nor
//!   `.git/index` (nothing further staged) — only the reflog and the branch's
//!   own ref file record that the tip moved. (This bug shipped once.)
//! - `.git/refs/heads` (directory, watched recursively per cargo's own docs) —
//!   `.git/refs/heads/<branch>` is the file an ordinary commit on that branch
//!   actually updates. Watching the directory catches that update directly,
//!   independent of whether reflogs are enabled (`core.logAllRefUpdates`).
//!   Narrowed from the whole of `.git/refs`, which also covers
//!   `.git/refs/remotes/*`, where a routine `git fetch` forced a rebuild that
//!   had nothing to do with this binary's HEAD.
//!
//! KNOWN GAP, stated rather than hidden: a ref packed by `git gc` /
//! `git pack-refs` lives in `.git/packed-refs`, outside `.git/refs`, which is
//! not watched. This does not reopen the commit-on-current-branch case — git
//! always writes a loose ref for the branch you actually commit on — so it is
//! a coverage note, not a correctness gap for what this script reports.
//!
//! ─── THE DIRTY FLAG: the claim is exactly the watch set ───────────────────
//!
//! `dirty` used to be `git status --porcelain` with NO pathspec, i.e. a
//! WHOLE-REPOSITORY claim. **That claim could not be kept, and it was
//! measured lying.** No git path changes when you edit a working-tree file, so
//! none of the four watches above fires: the crate recompiled from the edited
//! source while `ROSHERA_BUILD_DIRTY` stayed at the value captured before the
//! edit. Reproduced directly — `git status` reporting `M …/main.rs`, cargo
//! reporting `Checking api-server`, and the script's own output still saying
//! `ROSHERA_BUILD_DIRTY=0`. A binary asserting a clean tree that is not clean
//! is the root of the trust chain for every trajectory attributed to it.
//!
//! Forcing the script to re-run on every build (a `rerun-if-changed` on a
//! deliberately-absent path) does fix it, and was REJECTED ON MEASUREMENT:
//! cargo invalidates a script's dependents whenever the script re-runs,
//! regardless of whether its output changed, so a no-op `cargo build` went
//! from 1.9 s to 22 s and then 141 s. A correctness fix that makes every
//! rebuild of this crate non-incremental is not a fix anyone will keep.
//!
//! So the claim is NARROWED to one the mechanism can actually keep: every
//! workspace crate's `src/` and `Cargo.toml`, the workspace manifest, and this
//! script. Those exact paths are watched AND are the pathspec `git status`
//! runs against — one list, `watched_paths()`, feeds both, so the claimed set
//! and the watched set are equal by construction rather than by discipline
//! (two hand-maintained lists that must agree is the drift class this repo has
//! an ontology gate for). The list is DISCOVERED by reading the workspace
//! directory for crates rather than hardcoded, so adding a member crate needs
//! no edit here.
//!
//! The cost is ~zero: any change to a watched source was already going to
//! recompile this binary, so the script's re-run rides along with a rebuild
//! that was happening anyway.
//!
//! WHAT `dirty` NOW MEANS, precisely: "a tracked or untracked change exists
//! under some workspace crate's `src/`, some `Cargo.toml`, or this script."
//! It no longer means "anything anywhere in the repository". That is the
//! correct dimension for this field — a change under `roshera-app/` or
//! `docs/` does not alter this binary, and the harness records the MCP dist
//! digest and its own dirty flag as separate provenance dimensions.
//!
//! RESIDUE, stated: `.cargo/config.toml`, toolchain files, and any
//! `include_str!` reaching outside a watched `src/` can change the build
//! without setting this flag. And `git status` runs microseconds before rustc
//! finishes, so an edit landing inside that window is missed — negligible, but
//! real, and named rather than implied.
//!
//! When git is unavailable (a source tarball, a vendored build) the variables
//! are emitted EMPTY, and the handler turns that into a stated absence rather
//! than a fabricated value.

use std::process::Command;

/// Every path whose contents this build's identity claims to cover.
///
/// Returned as ONE list because it has two consumers that must never disagree:
/// the `rerun-if-changed` declarations (when the script re-runs) and the
/// `git status` pathspec (what `dirty` asserts). Derived by reading the
/// workspace directory, so a new member crate is covered without an edit here.
///
/// Paths are relative to this crate's directory, which is both the build
/// script's cwd and the directory git resolves the pathspec from — so the
/// same strings serve both consumers verbatim.
///
/// An empty return means the workspace could not be read. The caller treats
/// that as "the scope is unknown" and emits an ABSENT dirty reading, never a
/// clean one: a claim this script cannot delimit is a claim it must not make.
fn watched_paths() -> Vec<String> {
    let mut paths = vec!["build.rs".to_string(), "../Cargo.toml".to_string()];
    let Ok(entries) = std::fs::read_dir("..") else {
        return Vec::new();
    };
    let mut crates: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir() && e.path().join("Cargo.toml").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    // Sorted so the emitted declarations are deterministic across machines;
    // `read_dir` order is filesystem-dependent.
    crates.sort();
    for c in crates {
        paths.push(format!("../{c}/src"));
        paths.push(format!("../{c}/Cargo.toml"));
    }
    paths
}

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

    let watched = watched_paths();
    for path in &watched {
        println!("cargo:rerun-if-changed={path}");
    }

    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // No watched paths means the workspace could not be enumerated, so there
    // is no scope to make a claim about. Stay `None` — an absence — rather
    // than running an unscoped `git status` and re-making the whole-repository
    // claim this script just abandoned.
    let dirty = if watched.is_empty() {
        None
    } else {
        Command::new("git")
            .args(["status", "--porcelain", "--"])
            .args(&watched)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| !o.stdout.is_empty())
    };

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
