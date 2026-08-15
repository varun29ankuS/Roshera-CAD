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

/// Every distinct operation string passed as the 2nd argument to a call of
/// `drawing_mgr::refuse_unsound_solid(model_handle, "OPERATION", ...)` —
/// concern A's (2026-08-15 closeout) sheet-surface twin of `refuse_unsound_
/// base`, added to close the `make_drawing` gap this module's own exemption
/// list used to carry as "GENUINE, STILL-OPEN". It cannot go through
/// `refuse_unsound_base` itself for the same reason `export.rs`'s item-8
/// fix cannot (see `rust_direct_unsound_base_operations`'s doc): several
/// call sites already hold a read guard on the same `model_handle`, and the
/// helper reads the non-recomputing `soundness_reading` rather than
/// `certify_solid`. The function's OWN internal call,
/// `ApiError::unsound_base(operation, solid_id, crate::VERDICT_UNSOUND)`,
/// passes a `&str` VARIABLE, not a quoted literal — verified: the required
/// `\s*"` immediately after `unsound_base(` never matches an identifier —
/// so it cannot double-count against `rust_direct_unsound_base_operations`
/// (which scans a disjoint file list) or against itself.
fn rust_drawing_solid_gate_operations(src: &str) -> BTreeSet<String> {
    let re =
        Regex::new(r#"(?s)refuse_unsound_solid\(\s*[^,]+,\s*"([^"]+)""#).expect("static regex");
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
    let drawing_mgr_rs = repo_file("src/drawing_mgr.rs");

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
    // Concern A, 2026-08-15 closeout: the sheet surface's own gate,
    // `drawing_mgr::refuse_unsound_solid` — closes `make_drawing`'s
    // exemption for real (below) rather than leaving the comment stale.
    rust_ops.extend(rust_drawing_solid_gate_operations(&drawing_mgr_rs));
    assert!(
        rust_ops.len() >= 5,
        "sanity: only {} Rust operation name(s) found ({:?}) — the regex \
         likely stopped matching, which would make every assertion below \
         pass VACUOUSLY",
        rust_ops.len(),
        rust_ops
    );

    // ── L4 (2026-08-15 whole-branch review), concern F of the closeout ──
    // This comparison proves NAME parity only: that both surfaces agree an
    // operation called "boolean" is gated. It does NOT prove they gate the
    // SAME REFS — `gates.ts`'s `boolean: (a) => [{ uuid: a?.object_a },
    // { uuid: a?.object_b }]` (gates.ts:321) checks BOTH operands, and
    // Rust's `refuse_unsound_base(&model_handle, &payload, "boolean",
    // &[solid_a, solid_b])` (main.rs) also checks both — but nothing here
    // reads either array's LENGTH or contents. A change that drops one
    // operand from either side's array — TS returning only `object_a`, or
    // Rust passing `&[solid_a]` — passes this test silently: canonical name
    // sets still match, and the dropped operand is invisible to a
    // string-level scan of an argument list. Left as a follow-up rather
    // than attempted here: a real arity check would need to parse each
    // side's array LITERAL (not just detect that one exists) — TS's is a
    // multi-line arrow-function body, Rust's is a `&[...]` slice literal —
    // and correlate operation names across two different literal syntaxes,
    // which is a meaningfully different (and more fragile) parser than the
    // presence-only regexes above. Per the review's own assessment: worth
    // stating the limit precisely rather than a real fix in this pass.
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
        // `make_drawing` REMOVED (concern A, 2026-08-15 closeout): CLOSED
        // for real, not relabelled. `drawing_mgr::create_part_drawing_
        // inner` now calls `refuse_unsound_solid(&model_handle,
        // "make_drawing", &[solid_id], q.acknowledge_unsound)` before
        // building the sheet — the canonical name matches gates.ts's own
        // `BASE_REFS` key (`gates.ts:349`) exactly, so it now falls out of
        // `ts_only` on its own; there is nothing left to exempt. See
        // `unsound_solid_sheet_gate_tests.rs` for the tests, and this
        // file's own mutation-proof (the gate call site was temporarily
        // forced open, confirmed RED, then restored).
    ];

    // ── Rust-only exemptions ────────────────────────────────────────────
    // Two different reasons appear below, both legitimate. `mirror` through
    // `sketch/extrude_cut` are verified by grep across roshera-mcp/src (not
    // assumed): NO MCP tool calls those five REST routes, under any name.
    // `BASE_REFS` is keyed by MCP TOOL NAME, so there is structurally no TS
    // key to add for a route nothing in roshera-mcp ever dispatches to — the
    // Rust gate is the ONLY gate those operations need, because an agent
    // cannot walk around a client-side pre-flight that does not exist for a
    // route it cannot reach through MCP in the first place.
    //
    // `drawing_export` and `drawing_svg` (concern A, 2026-08-15 closeout)
    // are the OTHER reason: an MCP tool DOES reach `drawing_export`
    // (`drawing_export_sheet`, `io.ts`), but its client-side gate is
    // `sheetExportGate` (gate 4) — a DIFFERENT mechanism from `BASE_REFS`
    // (gate 3), which is specifically what this module compares. `BASE_REFS`
    // genuinely has no key for either name; that is not drift, it is gate 4
    // not yet having grown a solid-soundness branch to mirror this one. See
    // each entry.
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
        (
            "drawing_export",
            "GENUINE gap, but between gate 3 and gate 4, not a missing \
             forwarding: `export_svg`/`export_pdf`/`export_dxf` \
             (drawing_mgr.rs) check the underlying SOLID's soundness \
             server-side via `refuse_unsound_solid` — a DIFFERENT \
             mechanism from the `refuse_unsound_base` call sites THIS \
             module's `BASE_REFS`/gate-3 comparison actually tracks — so \
             `drawing_export` structurally can never appear as a \
             `BASE_REFS` key, independent of what the MCP side forwards. \
             Reached from MCP via `drawing_export_sheet` (io.ts); as of \
             `348cfadb` (one commit after this exemption was first \
             written, same branch) that tool's `acknowledge_unsound` IS \
             forwarded (io.ts:245 schema, io.ts:256 handler) alongside \
             `sheetExportGate`'s own `acknowledge_layout_issues` (gate 4) \
             — the MCP half is closed, and a raw HTTP client was already \
             covered (`?acknowledge_unsound=true`). This entry documents \
             gate-3/gate-4 non-comparability, which `348cfadb` does not \
             and cannot change.",
        ),
        (
            "drawing_svg",
            "GENUINE, DELIBERATE gap: `GET /api/parts/{id}/drawing.svg` / \
             .../uuid/{uuid}/drawing.svg` (drawing_mgr::part_drawing_svg / \
             _by_uuid) now check the underlying solid's soundness \
             server-side, but no MCP tool calls this one-call route at \
             all — verified by grep, zero references to \"drawing.svg\" \
             anywhere in roshera-mcp/src. Same shape as `mirror` above: \
             there is structurally no TS key to add.",
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
    // allows (brief, item 6). It is NOT empty: 2 TS-only + 7 Rust-only = 9
    // documented divergences remain (2026-08-15 closeout, concern A —
    // `make_drawing` CLOSED for real, `drawing_export` and `drawing_svg`
    // ADDED as the sheet surface grew its own gate). Of the 9, 7 are
    // legitimate (compositions over an already-gated primitive, or routes
    // no MCP tool reaches) and 2 (`drawing_export`, `drawing_svg`) stand for
    // a structural reason, not an outstanding gap: this module compares
    // `BASE_REFS` (gate 3, the `refuse_unsound_base` call sites) against
    // gates.ts, but the sheet surface's solid-soundness check runs through
    // `refuse_unsound_solid` — a different gate entirely, client-gated by
    // `sheetExportGate` (gate 4) — so neither name can ever be a `BASE_REFS`
    // key regardless of what MCP forwards. `348cfadb`, the very next commit
    // on this branch after this exemption was first written, closed the MCP
    // half of gate 4's own forwarding (`io.ts:245`/`:256`); that closure
    // does not and cannot retire this exemption, because gate 3 and gate 4
    // were never the same comparison (M1, 2026-08-15 whole-branch review —
    // this file's exemption text previously described the now-closed gate-4
    // gap as the reason for a gate-3 exemption, which stopped being true the
    // moment `348cfadb` landed).
}
