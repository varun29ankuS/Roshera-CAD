//! Curated branch-name pool — pop-culture references with timeline /
//! branching-narrative flavor.
//!
//! Anonymous auto-names like `Branch-1234` make a multi-branch DAG
//! hard to read at a glance. This module exposes a small, stable pool
//! of memorable names — every entry evokes a branching timeline or
//! alternate reality (Marvel TVA, Doctor Who, Back to the Future,
//! The Matrix, *Dark*, Star Trek, etc.). Both humans and agents draw
//! from it: `suggest_branch_names` returns up to N pool entries that
//! aren't already in use, and the caller picks one.
//!
//! The pool is intentionally short (20 names) and stable; rotating it
//! per release would defeat the point of memorable names.

use std::collections::HashSet;

/// The 20-name pool. Order is the priority — the front of the list is
/// what gets handed out first when no other names are taken. Names are
/// Title-Case so they render unmodified in the timeline UI and stay
/// shell-friendly (ASCII-only).
pub const BRANCH_NAME_POOL: &[&str] = &[
    "Eon", "Mobius", "Sylvie", "Variant", "Nexus", "Tardis", "Delorean", "Skynet", "Neo",
    "Morpheus", "Trinity", "Jonas", "Adam", "Ender", "Hyperion", "Kelvin", "Mirror", "Kairos",
    "Chronos", "Echo",
];

/// Pick up to `count` names from [`BRANCH_NAME_POOL`] that don't appear
/// in `existing` (case-insensitive). Returns at most
/// `BRANCH_NAME_POOL.len()` names; if every pool entry is taken,
/// returns an empty `Vec` and the caller must fall back to its own
/// auto-name scheme.
///
/// The returned order matches the pool's stable order — a caller that
/// always picks `suggestions[0]` will get the same name on a fresh
/// timeline every time.
pub fn suggest_branch_names<S: AsRef<str>>(count: usize, existing: &[S]) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let used: HashSet<String> = existing
        .iter()
        .map(|s| s.as_ref().to_ascii_lowercase())
        .collect();
    BRANCH_NAME_POOL
        .iter()
        .filter(|n| !used.contains(&n.to_ascii_lowercase()))
        .take(count)
        .map(|n| (*n).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_has_twenty_names() {
        assert_eq!(BRANCH_NAME_POOL.len(), 20);
    }

    #[test]
    fn pool_has_no_duplicates() {
        let mut seen = HashSet::new();
        for n in BRANCH_NAME_POOL {
            assert!(
                seen.insert(n.to_ascii_lowercase()),
                "duplicate pool entry: {n}"
            );
        }
    }

    #[test]
    fn pool_entries_are_non_empty_and_simple() {
        for n in BRANCH_NAME_POOL {
            assert!(!n.is_empty(), "empty pool entry");
            // Pool names must be ASCII-letter-only so they round-trip
            // through URL params, JSON, and shell-friendly contexts
            // without escaping.
            assert!(
                n.chars().all(|c| c.is_ascii_alphabetic()),
                "non-alphabetic char in {n}"
            );
        }
    }

    #[test]
    fn suggest_with_no_existing_returns_pool_prefix() {
        let s = suggest_branch_names(3, &[] as &[&str]);
        assert_eq!(s, vec!["Eon", "Mobius", "Sylvie"]);
    }

    #[test]
    fn suggest_skips_used_names_case_insensitive() {
        let s = suggest_branch_names(2, &["eon", "MOBIUS"]);
        assert_eq!(s, vec!["Sylvie", "Variant"]);
    }

    #[test]
    fn suggest_caps_at_pool_size_when_count_is_huge() {
        let s = suggest_branch_names(100, &[] as &[&str]);
        assert_eq!(s.len(), BRANCH_NAME_POOL.len());
    }

    #[test]
    fn suggest_returns_empty_when_count_is_zero() {
        let s = suggest_branch_names(0, &[] as &[&str]);
        assert!(s.is_empty());
    }

    #[test]
    fn suggest_returns_empty_when_all_taken() {
        let used: Vec<&str> = BRANCH_NAME_POOL.to_vec();
        let s = suggest_branch_names(3, &used);
        assert!(
            s.is_empty(),
            "should return empty when whole pool is in use"
        );
    }
}
