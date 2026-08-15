//! Concern E (L3, 2026-08-15 closeout): a server-side gate refusal must
//! reach an MCP caller as a TYPED refusal, not indistinguishable prose.
//!
//! `roshera-mcp/src/core.ts::api()` embeds the raw HTTP response body text
//! verbatim into the thrown `ApiError.message`
//! (`` `${method} ${path} → ${res.status}: ${text}` ``); `fail()` then
//! wraps that as `content[0].text = "ERROR: ${msg}\nHINT: ..."` — so the
//! full JSON body, including this catalog's own `error` prose, ends up
//! inside the text `gates.ts::typedRefusalOf` inspects. That function
//! recognises a refusal two ways: a top-level JSON `refused` key (this
//! catalog's wire shape has none — adding one would not reach
//! `content[0].text` without ALSO changing `fail()`, which is
//! `roshera-mcp` territory, out of scope here), or `result.isError === true
//! && /\bREFUSED\b/.test(text)`. `fail()` always sets `isError: true` on
//! any thrown `ApiError`, so the second branch is reachable from the Rust
//! side ALONE: MOST gate-class constructors' `error` messages open with
//! the literal, word-bounded token `REFUSED` (see `error_catalog.rs`'s
//! module doc, "A deliberate REFUSAL is spelled `REFUSED:`"). `unsound_base`
//! is the deliberate exception — it is a `LIVE_FACT_GATES` member, and the
//! REFUSED-token branch returns `gate: undefined`, which `gates.ts`'s
//! cache-skip check cannot recognise as live-fact-exempt; carrying the
//! token there risks a repaired solid's retry answered from a stale
//! cache. See `unsound_base`'s own doc and
//! `unsound_base_deliberately_does_not_carry_the_refused_token` below.
//!
//! This module pins that contract from BOTH ends:
//!   1. `gates.ts`'s `typedRefusalOf` still recognises the shape
//!      (`isError` + `/\bREFUSED\b/`) — read from disk, not assumed;
//!   2. every gate-class `ApiError` constructor's live `error` field
//!      carries (or, for `unsound_base`, deliberately does NOT carry) the
//!      token.
//!
//! Does NOT claim the full round-trip is closed — that needs `fail()` (or
//! `typedRefusalOf`) on the `roshera-mcp` side to be re-verified against a
//! live MCP dispatch, which is out of this task's territory. This is the
//! REST-side half: what a raw HTTP client (or the existing, UNCHANGED
//! `roshera-mcp` `fail()` path) already receives today.

#![cfg(test)]

use crate::error_catalog::ApiError;

// =====================================================================
// 1. gates.ts's detector shape is still what this module assumes
// =====================================================================

#[tokio::test]
async fn gates_ts_typed_refusal_detector_still_checks_the_refused_token() {
    let gates_ts =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../roshera-mcp/src/gates.ts");
    let src = std::fs::read_to_string(&gates_ts).unwrap_or_else(|e| {
        panic!(
            "the MCP client refusal detector must be readable at {} (this \
             test exists to keep the REST-side REFUSED-token convention in \
             step with what typedRefusalOf actually checks; if the file \
             moved, re-point it rather than deleting the check): {e}",
            gates_ts.display()
        )
    });
    assert!(
        src.contains(r"\bREFUSED\b"),
        "gates.ts::typedRefusalOf no longer matches on a word-bounded \
         REFUSED token — the REST-side convention this module pins \
         (every gate-class ApiError message starts with \"REFUSED: \") \
         would stop being classified as a typed refusal by the MCP \
         surface. Re-check both sides together."
    );
    assert!(
        src.contains("isError === true"),
        "gates.ts::typedRefusalOf no longer gates the REFUSED-token check \
         on isError — re-check the detector's shape"
    );
}

// =====================================================================
// 2. Every gate-class constructor's message actually carries the token
// =====================================================================

/// A minimal, deliberately loose reimplementation of `typedRefusalOf`'s
/// second branch: `isError === true && /\bREFUSED\b/.test(text)`. `fail()`
/// always sets `isError: true` for a thrown `ApiError`, so pinning this
/// down to "does the message contain a word-bounded REFUSED" is the exact
/// REST-side half of the contract.
fn carries_the_refused_token(message: &str) -> bool {
    // Word-boundary check without a regex crate dependency: REFUSED must
    // not be immediately preceded or followed by an alphanumeric/underscore
    // character.
    let bytes = message.as_bytes();
    let needle = b"REFUSED";
    let is_word_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    message.match_indices("REFUSED").any(|(idx, _)| {
        let before_ok = idx == 0 || !is_word_byte(bytes[idx - 1]);
        let after = idx + needle.len();
        let after_ok = after >= bytes.len() || !is_word_byte(bytes[after]);
        before_ok && after_ok
    })
}

/// `unsound_base` deliberately does NOT carry the token — see its doc.
/// It is a `LIVE_FACT_GATES` member (`gates.ts:378`); the REFUSED-token
/// branch of `typedRefusalOf` returns `gate: undefined`, which
/// `recordDispatchOutcome`'s cache-skip check (keyed on `gate`,
/// `gates.ts:1320`) cannot recognise as live-fact-exempt — a cached
/// `unsound_base` refusal under the SAME solid id would survive a
/// legitimate repair, leaving `acknowledge_unsound` as the only apparent
/// exit. Pinned as a NEGATIVE so a future edit that "helpfully" adds the
/// prefix here fails loudly instead of silently reintroducing the risk.
#[test]
fn unsound_base_deliberately_does_not_carry_the_refused_token() {
    let err = ApiError::unsound_base("fillet", 7, "UNSOUND — test verdict");
    assert!(
        !carries_the_refused_token(&err.error),
        "unsound_base must NOT carry REFUSED — see error_catalog.rs's \
         unsound_base doc for the LIVE_FACT_GATES caching hazard this \
         avoids; error = {:?}",
        err.error
    );
}

#[test]
fn sheet_uncertified_carries_the_refused_token() {
    let err = ApiError::sheet_uncertified(uuid::Uuid::nil());
    assert!(
        carries_the_refused_token(&err.error),
        "error = {:?}",
        err.error
    );
}

#[test]
fn sheet_unsound_carries_the_refused_token() {
    let err = ApiError::sheet_unsound(uuid::Uuid::nil(), 1, 0);
    assert!(
        carries_the_refused_token(&err.error),
        "error = {:?}",
        err.error
    );
}

#[test]
fn sheet_quality_carries_the_refused_token() {
    let err = ApiError::sheet_quality(uuid::Uuid::nil(), 2);
    assert!(
        carries_the_refused_token(&err.error),
        "error = {:?}",
        err.error
    );
}

#[test]
fn intent_required_carries_the_refused_token() {
    let err = ApiError::intent_required("boolean");
    assert!(
        carries_the_refused_token(&err.error),
        "error = {:?}",
        err.error
    );
}

/// The detector must not false-positive on a word that merely CONTAINS
/// "REFUSED" as a substring (e.g. "UNREFUSED") — pins the word-boundary
/// half of the helper above against itself, so a future loosening of the
/// helper is caught here rather than by accident downstream.
#[test]
fn the_local_detector_respects_word_boundaries() {
    assert!(!carries_the_refused_token("this was UNREFUSEDLY fine"));
    assert!(!carries_the_refused_token("REFUSEDX and Xrefused"));
    assert!(carries_the_refused_token("REFUSED: plain case"));
    assert!(carries_the_refused_token("solid 3 (REFUSED) is bad"));
}
