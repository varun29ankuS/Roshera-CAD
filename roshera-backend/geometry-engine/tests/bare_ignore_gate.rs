// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Gate: every `#[ignore]` in the workspace must carry a reason string.
//!
//! A bare `#[ignore]` is an unratcheted back door around the `KNOWN_REDS`
//! red-gate: `red-gate.ps1` only compares FAILURES against the allowlist, so
//! a test that is simply never run is invisible to it — it can rot silently
//! forever with no diagnosis and no ratchet entry. Requiring
//! `#[ignore = "…"]` doesn't stop anyone from ignoring a test; it stops them
//! from doing it without leaving a trail.
//!
//! # Scope and method
//!
//! Walks every `*.rs` file in the Cargo workspace (found by walking up from
//! `CARGO_MANIFEST_DIR` to the directory containing the workspace
//! `Cargo.toml`), skipping `target/` build output and this gate's own file
//! (see the self-exclusion note on [`NEEDLE`]).
//!
//! For each remaining file, each line is checked (this is a **line-based**
//! check, not a real Rust parser — it does not understand block comments
//! `/* … */` or the attribute appearing inside a string literal; neither
//! pattern occurs in this codebase today, verified by an independent
//! shell-loop cross-check during the audit that produced this gate):
//!
//! - A line whose first non-whitespace characters are `//` (covers `//`,
//!   `///`, `//!`) is skipped — it's prose, not an attribute, and this
//!   codebase's docs frequently *talk about* `#[ignore]` in the abstract.
//! - Any remaining line containing the literal substring `#[ignore]`
//!   (built via [`NEEDLE`] so this file doesn't trip its own gate) is a
//!   bare ignore with no reason. `#[ignore = "…"]` does NOT match this
//!   substring (there is a space and `=` between `ignore` and `]`), so
//!   reasoned ignores correctly pass.
//!
//! This exact method (anchored regex vs. line-content classification) was
//! cross-checked during the audit that added this gate: both agreed on
//! every file in `roshera-backend`. `roshera-eval/` and `verdict-harness/`
//! were included in the same `find`-from-repo-root sweep and contributed no
//! hits (verified: workspace has 0 bare ignores outside the one this gate's
//! commit fixes).

use std::path::{Path, PathBuf};

/// The bare-ignore attribute text, assembled at compile time so this file's
/// own source never contains the literal substring `#[ignore]` — otherwise
/// the gate would flag itself the moment it's compiled into this workspace.
fn needle() -> String {
    concat!("#[", "ignore]").to_string()
}

/// Walk up from `start` until a directory containing `Cargo.toml` with a
/// `[workspace]` table is found. `CARGO_MANIFEST_DIR` for this crate is
/// `.../roshera-backend/geometry-engine`; the workspace root one level up
/// is `.../roshera-backend`.
fn find_workspace_root(start: &Path) -> PathBuf {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).unwrap_or_default();
            if text.contains("[workspace]") {
                return dir;
            }
        }
        if !dir.pop() {
            panic!(
                "walked up from {} to filesystem root without finding a workspace Cargo.toml",
                start.display()
            );
        }
    }
}

/// Recursively collect every `*.rs` file under `dir`, skipping `target/`
/// build output directories (which can contain generated/vendored source
/// with attributes this gate has no business policing).
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("target") {
                continue;
            }
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// A bare `#[ignore]` site: file path (relative to the workspace root, for
/// a stable/portable report) and 1-based line number.
struct BareIgnore {
    file: String,
    line: usize,
}

/// Scan the whole workspace and return every bare `#[ignore]` site.
fn find_bare_ignores() -> Vec<BareIgnore> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = find_workspace_root(manifest_dir);
    // Self-exclusion: canonicalize this gate's own path so it never flags
    // itself regardless of the directory `cargo test` was invoked from.
    let self_path = manifest_dir
        .join("tests")
        .join("bare_ignore_gate.rs")
        .canonicalize()
        .ok();

    let mut rs_files = Vec::new();
    collect_rs_files(&workspace_root, &mut rs_files);

    let needle = needle();
    let mut hits = Vec::new();
    for path in rs_files {
        if let (Some(self_p), Ok(this_p)) = (&self_path, path.canonicalize()) {
            if *self_p == this_p {
                continue;
            }
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains(&needle) {
                let rel = path
                    .strip_prefix(&workspace_root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                hits.push(BareIgnore {
                    file: rel,
                    line: idx + 1,
                });
            }
        }
    }
    hits
}

#[test]
fn no_bare_ignore_attributes_in_workspace() {
    let hits = find_bare_ignores();
    assert!(
        hits.is_empty(),
        "{} bare `#[ignore]` attribute(s) found with no reason string. \
         Every `#[ignore]` must be `#[ignore = \"…\"]` so a skipped test \
         leaves a trail instead of rotting invisibly outside the \
         KNOWN_REDS gate:\n{}",
        hits.len(),
        hits.iter()
            .map(|h| format!("  {}:{}", h.file, h.line))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Proves the gate can actually fail — a gate that only ever passes is
/// indistinguishable from one that never runs. `needle()` is exercised
/// directly against a synthetic line rather than smuggling a real bare
/// `#[ignore]` into this source file (which would trip the gate above).
#[test]
fn needle_matches_bare_ignore_but_not_reasoned_ignore() {
    let needle = needle();
    let bare_line = "    #[ignore]";
    let reasoned_line = "    #[ignore = \"flaky under load\"]";
    assert!(
        bare_line.contains(&needle),
        "needle must match a genuine bare #[ignore] line"
    );
    assert!(
        !reasoned_line.contains(&needle),
        "needle must NOT match a reasoned #[ignore = \"…\"] line"
    );
}
