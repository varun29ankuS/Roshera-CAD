//! Item 6 (audit S8, 2026-08-15). The ONE check pinning `gates.ts`'s
//! `BASE_REFS` and the Rust unsound-base call sites together used to be
//! five hardcoded substring lookups in `unsound_base_gate_tests.rs` — pure
//! PRESENCE, unable to fail when the Rust side drops a route, when TS adds
//! a key with no Rust counterpart, or when the two surfaces simply
//! diverge. Measured at audit time, in both directions:
//!   - TS `BASE_REFS` had 8 keys the Rust side lacked exact-name matches
//!     for in places (`boolean_many`, `drill_pattern`, `make_drawing`);
//!   - Rust had 5 `refuse_unsound_base` call sites (`mirror`,
//!     `pattern/linear`, `pattern/circular`, `face/extrude`,
//!     `sketch/extrude_cut`) with no TS key at all.
//!
//! This module derives BOTH sets from the live source text of the files
//! that actually define them (never hardcoded twice — a second hand-synced
//! list would rot exactly like the check it replaces) and asserts SET
//! EQUALITY modulo an explicit, commented exemption list, checked in BOTH
//! directions: every measured divergence must be an exemption, and every
//! exemption must still correspond to a real, measured divergence (a
//! ratchet — an exemption whose gap has closed must be deleted, the same
//! discipline `geometry-engine/KNOWN_REDS.md` enforces).
//!
//! ## What "the same operation" means across the two languages
//!
//! The two sides do not always use the same token for the same route: the
//! MCP tool is named `fillet_edges`, the REST operation label passed to
//! `refuse_unsound_base` is `"fillet"`. `canonical()` maps the four known
//! naming differences onto one shape before comparing; everything else is
//! compared verbatim.

#![cfg(test)]

use regex::Regex;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn repo_file(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} must be readable (this test exists to keep gates.ts and the \
             Rust unsound-base call sites in step; if the file moved, \
             re-point it rather than deleting the check): {e}",
            path.display()
        )
    })
}

/// Every `BASE_REFS` key in `gates.ts`, read from the object's own source
/// text. Brace-counted rather than regex-matched end-to-end: the object's
/// values are multi-line arrow functions that themselves contain further
/// `{...}` literals (e.g. `boolean: (a) => [{ uuid: a?.object_a }, ...]`),
/// so a naive "find the closing brace" would stop at the first nested one.
fn ts_base_refs_keys(src: &str) -> BTreeSet<String> {
    let start = src
        .find("const BASE_REFS")
        .expect("gates.ts must still declare BASE_REFS — this test's whole premise");
    let after = &src[start..];
    // NOT the first '{' after the declaration — `BASE_REFS`'s TYPE
    // annotation (`Record<string, (args: any) => Array<{ uuid?: string; ...
    // }>>`) contains its own `{...}` before the object literal even starts.
    // The assignment operator `= {` is what actually opens the object.
    let assign = after
        .find("= {")
        .expect("BASE_REFS declaration must assign an object literal (`= {`)");
    let obj_start = start + assign + "= {".len();
    let bytes = src.as_bytes();
    let mut depth: i32 = 1;
    let mut i = obj_start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    let obj_end = i - 1; // index of the matching closing '}'
    let block = &src[obj_start..obj_end];
    // Only a top-level `key: (a) => ...` line counts — a nested arrow like
    // `(uuid: string) => ({ uuid })` inside export_part's body never starts
    // a line at column 0-of-indentation with `word: (a) =>`, so it cannot
    // false-positive here.
    let re = Regex::new(r"(?m)^\s*(\w+):\s*\(a\)\s*=>").expect("static regex");
    re.captures_iter(block).map(|c| c[1].to_string()).collect()
}

/// Every distinct operation string passed as the 3rd positional argument to
/// `refuse_unsound_base(model_handle, payload, "OPERATION", bases)`.
/// `(?s)` so a multi-line call (`sketch.rs`'s `crate::refuse_unsound_base`
/// spans several lines) still matches; `[^,]+` cannot itself consume a
/// comma, so — even under dotall — it cannot walk past the argument
/// boundary into the next one. Verified against the function's own
/// DEFINITION (`pub(crate) async fn refuse_unsound_base(model_handle: &Arc<
/// ...>, payload: &serde_json::Value, operation: &str, ...)`): the 3rd
/// parameter there is `operation: &str`, not a quoted literal, so the
/// required `\s*"` after the 2nd comma never matches the definition site.
fn rust_refuse_unsound_base_operations(src: &str) -> BTreeSet<String> {
    let re = Regex::new(r#"(?s)refuse_unsound_base\(\s*[^,]+,\s*[^,]+,\s*"([^"]+)""#)
        .expect("static regex");
    re.captures_iter(src).map(|c| c[1].to_string()).collect()
}

/// Every distinct operation string passed as the 1st argument to a DIRECT
/// `ApiError::unsound_base("OPERATION", solid_id, verdict)` construction —
/// item 8's shape in `export.rs`, which cannot go through the shared
/// `refuse_unsound_base` helper (that helper takes a WRITE lock and
/// RECOMPUTES via `certify_solid`; `export_mesh` already holds a READ guard
/// and must read the non-recomputing `soundness_reading`, so calling it
/// would both deadlock and re-launder the very verdict this gate exists to
/// catch). The helper's OWN internal call, `error_catalog::ApiError::
/// unsound_base(operation, solid_id, VERDICT_UNSOUND)` (`main.rs`), passes
/// a `&str` VARIABLE as the first argument, not a quoted literal, so it
/// cannot double-count against `rust_refuse_unsound_base_operations` above
/// — verified: the required `\s*"` immediately after `unsound_base(` never
/// matches an identifier.
fn rust_direct_unsound_base_operations(src: &str) -> BTreeSet<String> {
    let re = Regex::new(r#"(?s)ApiError::unsound_base\(\s*"([^"]+)""#).expect("static regex");
    re.captures_iter(src).map(|c| c[1].to_string()).collect()
}

/// Canonicalise a name so a deliberate, known naming difference between the
/// two languages does not read as a false divergence. Only the pairs
/// actually known to differ are listed; anything else passes through
/// unchanged.
fn canonical(name: &str) -> &str {
    match name {
        "fillet_edges" => "fillet",
        "chamfer_edges" => "chamfer",
        "export_part" => "export",
        other => other,
    }
}

#[tokio::test]
async fn the_two_base_ref_surfaces_are_equal_modulo_the_exemption_list() {
    let gates_ts = repo_file("../../roshera-mcp/src/gates.ts");
    let main_rs = repo_file("src/main.rs");
    let sketch_rs = repo_file("src/sketch.rs");
    let export_rs = repo_file("src/handlers/export.rs");

    let ts_keys = ts_base_refs_keys(&gates_ts);
    assert!(
        ts_keys.len() >= 5,
        "sanity: the BASE_REFS extractor found only {} key(s) ({:?}) — the \
         regex likely stopped matching gates.ts's real shape, which would \
         make every assertion below pass VACUOUSLY (an empty set is a \
         subset of anything)",
        ts_keys.len(),
        ts_keys
    );

    let mut rust_ops = rust_refuse_unsound_base_operations(&main_rs);
    rust_ops.extend(rust_refuse_unsound_base_operations(&sketch_rs));
    rust_ops.extend(rust_direct_unsound_base_operations(&main_rs));
    rust_ops.extend(rust_direct_unsound_base_operations(&sketch_rs));
    rust_ops.extend(rust_direct_unsound_base_operations(&export_rs));
    assert!(
        rust_ops.len() >= 5,
        "sanity: only {} Rust operation name(s) found ({:?}) — the regex \
         likely stopped matching, which would make every assertion below \
         pass VACUOUSLY",
        rust_ops.len(),
        rust_ops
    );

    let ts_canonical: BTreeSet<&str> = ts_keys.iter().map(|k| canonical(k)).collect();
    let rust_canonical: BTreeSet<&str> = rust_ops.iter().map(|k| canonical(k)).collect();

    // TS keys with no Rust counterpart under their canonical name.
    let ts_only: BTreeSet<&str> = ts_canonical.difference(&rust_canonical).copied().collect();
    // Rust operations with no TS `BASE_REFS` key under their canonical name.
    let rust_only: BTreeSet<&str> = rust_canonical.difference(&ts_canonical).copied().collect();

    // ── TS-only exemptions ──────────────────────────────────────────────
    // Each is either an MCP-side composition over an ALREADY-gated REST
    // primitive, or a genuinely open, documented gap — never a silent pass.
    let ts_only_exempt: &[(&str, &str)] = &[
        (
            "boolean_many",
            "MCP composition over per-step POST /api/geometry/boolean calls \
             (modify.ts) — each step is already gated under \"boolean\"; \
             boolean_many's own per-step certification halts on the first \
             unsound step, so there is nothing further to gate here.",
        ),
        (
            "drill_pattern",
            "MCP composition over per-hole POST /api/geometry/cylinder + \
             POST /api/geometry/boolean calls (modify.ts) — NOT the Rust \
             pattern/linear or pattern/circular routes, which are an \
             unrelated whole-solid-replication feature. Each boolean step \
             is already gated under \"boolean\".",
        ),
        (
            "make_drawing",
            "GENUINE, STILL-OPEN gap (audit S8/S5). POST /api/parts/{id}/\
             drawing (drawing_mgr::create_part_drawing) has no \
             refuse_unsound_base / ApiError::unsound_base call of its own. \
             Item 8 (2026-08-15) and 4b1ef771 gated the THREE EXPORT \
             routes downstream — a DIFFERENT gate (sheet staleness/quality, \
             not this one) — neither touched the CREATION route. Not fixed \
             on this branch; belongs in a future pass, not silently closed \
             by relabelling this exemption.",
        ),
    ];

    // ── Rust-only exemptions ────────────────────────────────────────────
    // Verified by grep across roshera-mcp/src (not assumed): NO MCP tool
    // calls any of these five REST routes, under any name. `BASE_REFS` is
    // keyed by MCP TOOL NAME, so there is structurally no TS key to add for
    // a route nothing in roshera-mcp ever dispatches to — the Rust gate is
    // the ONLY gate these operations need, because an agent cannot walk
    // around a client-side pre-flight that does not exist for a route it
    // cannot reach through MCP in the first place.
    let rust_only_exempt: &[(&str, &str)] = &[
        (
            "mirror",
            "POST /api/geometry/mirror has no MCP tool. psketch_op's own \
             \"mirror\" op targets POST /api/csketch/{id}/mirror — a 2D \
             SKETCH mirror, a different route entirely.",
        ),
        (
            "pattern/linear",
            "POST /api/geometry/pattern/linear (whole-SOLID replication) \
             has no MCP tool. psketch_op's \"linear_pattern\" targets \
             POST /api/csketch/{id}/pattern/linear — a 2D sketch-ENTITY \
             pattern, a different route.",
        ),
        (
            "pattern/circular",
            "Same as pattern/linear, circular case: POST /api/geometry/\
             pattern/circular (solid) has no MCP tool; psketch_op's \
             \"circular_pattern\" is the unrelated sketch-entity route.",
        ),
        (
            "face/extrude",
            "POST /api/geometry/face/extrude has no MCP tool — zero \
             references anywhere in roshera-mcp/src.",
        ),
        (
            "sketch/extrude_cut",
            "POST /api/csketch/{id}/extrude_cut (sketch.rs) has no MCP \
             tool — zero references anywhere in roshera-mcp/src (only \
             named in kb_data.ts's prose, never dispatched).",
        ),
    ];

    let ts_only_exempt_names: BTreeSet<&str> = ts_only_exempt.iter().map(|(n, _)| *n).collect();
    let rust_only_exempt_names: BTreeSet<&str> = rust_only_exempt.iter().map(|(n, _)| *n).collect();

    // Forward direction: every MEASURED divergence must be an EXPLICIT,
    // documented exemption — an unexempted name here is real, unnoticed
    // drift between the two surfaces.
    let unexempted_ts: Vec<&&str> = ts_only
        .iter()
        .filter(|n| !ts_only_exempt_names.contains(**n))
        .collect();
    assert!(
        unexempted_ts.is_empty(),
        "gates.ts::BASE_REFS has key(s) {unexempted_ts:?} with no Rust \
         refuse_unsound_base / ApiError::unsound_base counterpart and no \
         documented exemption — real coverage drift (audit S8); either add \
         the Rust-side gate or add a justified exemption entry"
    );
    let unexempted_rust: Vec<&&str> = rust_only
        .iter()
        .filter(|n| !rust_only_exempt_names.contains(**n))
        .collect();
    assert!(
        unexempted_rust.is_empty(),
        "Rust has unsound-base call site(s) {unexempted_rust:?} with no \
         gates.ts::BASE_REFS counterpart and no documented exemption — \
         either the two surfaces have drifted or this exemption list needs \
         an entry explaining why not"
    );

    // Ratchet direction: an exemption whose key is no longer part of the
    // MEASURED divergence means the gap it documents has been closed — the
    // entry is now stale and must be DELETED, exactly like
    // `KNOWN_REDS.md`'s RATCHET_VIOLATION rule (an allowlist entry that
    // now passes is an error, not a free pass).
    for (name, _) in ts_only_exempt {
        assert!(
            ts_only.contains(name),
            "the TS-only exemption for {name:?} is STALE — it now has a \
             Rust counterpart (or is no longer a BASE_REFS key at all); \
             delete this exemption entry rather than leaving it to cover a \
             gap that no longer exists"
        );
    }
    for (name, _) in rust_only_exempt {
        assert!(
            rust_only.contains(name),
            "the Rust-only exemption for {name:?} is STALE — it now has a \
             gates.ts::BASE_REFS counterpart (or the Rust call site is \
             gone); delete this exemption entry"
        );
    }

    // The exemption list should be as close to empty as the code actually
    // allows (brief, item 6). It is NOT empty: 3 TS-only + 5 Rust-only = 8
    // documented divergences remain, of which 7 are legitimate (compositions
    // over an already-gated primitive, or routes no MCP tool reaches) and 1
    // (`make_drawing`) is a real, open gap. See the lane-a report for the
    // honest accounting.
}
