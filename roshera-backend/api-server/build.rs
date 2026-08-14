//! Capture the build's git identity so the server can report what it actually is.
//!
//! THE CACHING TRAP: cargo runs a build script once and reuses its output until
//! something it DECLARED (via `rerun-if-changed`) changes. Get the declared set
//! wrong and the sha captured on the first build is baked into every later
//! binary — the server then reports a build identity that is confidently
//! wrong, which is precisely the class of defect this whole feature exists to
//! remove. Four paths are watched, each for a distinct git write that must not
//! go unnoticed:
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
//!   own ref file record that the tip moved. Without this line, committing
//!   after staging is invisible to cargo and the binary reports the
//!   pre-commit sha with a stale `dirty: true` on a now-clean tree — the
//!   exact defect this feature exists to eliminate, reintroduced by its own
//!   implementation. (This bug shipped once; this comment and the extra
//!   `rerun-if-changed` lines are the fix.)
//! - `.git/refs/heads` (directory, watched recursively per cargo's own docs) —
//!   `.git/refs/heads/<branch>` is the file an ordinary commit on that branch
//!   actually updates. Watching the directory catches that update directly,
//!   independent of whether reflogs are enabled (`core.logAllRefUpdates`),
//!   and also covers merges/rebases that fast-forward a ref without an
//!   intervening checkout. Narrowed from the whole of `.git/refs`, which is
//!   watched recursively and therefore also covers `.git/refs/remotes/*`: a
//!   routine `git fetch` moves remote-tracking refs that have nothing to do
//!   with this binary's HEAD and forced a `build.rs` re-run plus a crate
//!   rebuild every time. `refs/heads` is what the argument above actually
//!   needs, so nothing stated here is lost by narrowing to it. (Local tags
//!   under `refs/tags` are equally unwatched now — tagging does not move HEAD
//!   and does not change what `git rev-parse HEAD` reports, so it is not a
//!   capture this script has any reason to re-trigger on.)
//!
//! KNOWN GAP, stated rather than hidden: if refs get packed by `git gc` /
//! `git pack-refs`, a *packed* ref lives in `.git/packed-refs`, outside
//! `.git/refs`, and that file is not watched here. This does not reopen the
//! commit-on-current-branch case above — git always writes/updates a loose
//! ref for the branch you actually commit on, packed-refs or not — but a
//! rebuild triggered purely by some *other* ref being packed (no commit of
//! ours involved) would not be observed. That case does not change this
//! binary's own HEAD/sha, so it is not a correctness gap for what this
//! script reports, only a note for anyone auditing coverage.
//!
//! `dirty` is `git status --porcelain` run with cwd = this crate's directory
//! and NO pathspec, so git walks up to the repository root and reports
//! WHOLE-REPOSITORY dirty state (any file anywhere in the repo), not just
//! files that feed this binary. That is deliberate — a dirty tree anywhere
//! means the checkout as a whole is not exactly what `sha` names — but it
//! means `dirty: true` does not imply the api-server crate's own sources
//! changed.
//!
//! When git is unavailable (a source tarball, a vendored build) the variables are
//! emitted EMPTY, and the handler turns that into a stated absence rather than a
//! fabricated value.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");

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
