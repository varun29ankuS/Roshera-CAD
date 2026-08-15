# roshera-backend KNOWN_DISCONNECTED -- built, correct, and connected to nothing
#
# THE FAILURE THIS EXISTS FOR
# Fourteen times in this repo a capability has been BUILT, been CORRECT, and
# been WIRED TO NOTHING: EventCertificate fields written empty; HIST/PROV
# mandatory-but-empty; a SIGN flag with no chunk; merkle.rs; intent enforced
# then dropped; chunk CRCs never validated on import; header AI-hints promised
# in prose only; establishAcpSession never called on mount; a rename_document
# route with zero callers; twist_angle never exposed; create_helical_sweep
# unreachable; ThreadSpecification deserialized by nowhere; the mould verb's
# recipe that cannot reach the fillet seam; create_pattern recording nothing.
# Review caught none of them. Reviewing harder is not the fix -- a gate is.
#
# RATCHET RULE (NON-NEGOTIABLE, same rule as geometry-engine/KNOWN_REDS.md)
# Entries in this file may only be REMOVED (when the capability gains a real
# production consumer). They may NEVER be added without a diagnosis in the
# trailing comment naming what the symbol is for and why it is still unwired.
# The gate enforces both directions: a new disconnection not listed here exits
# 1 (NEW_DISCONNECTED); a listed dead-symbol entry that has since gained a
# consumer exits 2 (RATCHET_VIOLATION -- remove the line).
#
# Gate script: roshera-backend/scripts/disconnection-gate.ps1
#   powershell -File roshera-backend/scripts/disconnection-gate.ps1
#   powershell -File roshera-backend/scripts/disconnection-gate.ps1 -Seed   # re-seed BASELINE STOCK
#
# ENTRY FORMAT (one per line; everything from the first "  #" onward is
# metadata and is NOT part of the comparison key, so a drifting file:line
# never fires a spurious NEW/RATCHET pair):
#
#   <crate>::<Symbol>  # class=<class> file=<path>:<line> date=<yyyy-mm-dd> [diag: ...]
#
# CLASSES
#   dead-symbol   JUDGED by the gate. A `pub` item in a scoped crate that no
#                 production file other than its own declaring file mentions
#                 anywhere in the workspace.
#   wiring-shape  NEVER JUDGED -- informational only. Two independently
#                 produced pieces of state (claim/artifact, route/consumer,
#                 field/registry-entry, type/handler) that each compile, each
#                 pass a symbol-reachability check, and only disagree at
#                 runtime. These symbols ARE referenced, so any symbol scan
#                 calls them connected. Do not "fix" the gate to judge them --
#                 it would fire a permanent, meaningless RATCHET_VIOLATION.
#                 Their gate is a production-call-site assertion test (see
#                 roshera-backend/CLAUDE.md, "Disconnection gate").
#   out-of-scope  NEVER JUDGED. A confirmed dead symbol in a crate outside the
#                 gate's default -Crates scope. Widening the scope must
#                 re-classify these to dead-symbol deliberately.
#
# SCOPE (why these eight crates)
#   Judged: shared-types, ros-format, timeline-engine, export-engine,
#           assembly-engine, session-manager, ai-integration, verdict-harness.
#   geometry-engine is EXCLUDED: 3725 pub items over 267 files, and the design
#   doc measured the false-positive trimming that scope needs (macro-registered
#   ops, trait-object dispatch through ai_operations_registry, legitimate
#   cross-crate API) and explicitly deferred it. api-server is EXCLUDED because
#   it is a BIN crate -- rustc's own dead_code lint already fires there (~234
#   warnings per CLAUDE.md "Build status", verified 2026-04-30, NOT re-measured
#   here -- no cargo was run). The blind spot this gate covers is the LIB one:
#   every `pub` item reachable from a lib crate's root is a live root to rustc,
#   so dead_code is structurally silent on it however unused.
#
# HONEST COVERAGE: of the fourteen instances, ONE (merkle.rs's
# compute_merkle_root / BatchMerkleProof) is inside this gate's automated
# reach. Eight are wiring-shape and machine-uncheckable by any symbol scan.
# The rest sit in geometry-engine or the frontend. This file does not claim
# more than that -- it claims the stock does not GROW.

# =============================================================================
# SECTION A -- CLASSIFIED: the fourteen built-correct-disconnected instances
# =============================================================================
# These carry diagnoses. Line numbers verified 2026-08-10 against HEAD
# (feat/sketch-dcm-45).

timeline-engine::EventCertificate  # class=wiring-shape file=roshera-backend/timeline-engine/src/event_certificate.rs:103 date=2026-08-10 diag: instance 1 -- type IS constructed in production (recorder_bridge.rs:384); the gap was WHICH FIELDS get populated, an intra-call data-flow gap. Gate = round-trip test asserting non-default fields for the op type that should populate them.
export-engine::hist_prov_chunk_payload  # class=wiring-shape file=roshera-backend/export-engine/src/formats/ros.rs:353 date=2026-08-10 diag: instance 2 -- HIST/PROV are MANDATORY chunks that were written, and written EMPTY. The chunk exists, so every reachability check passes. Gate = writer round-trip asserting the chunk is non-empty, not merely present. RosWriteSummary (ros.rs:353) now exists to make the claim inspectable.
export-engine::sign_flag_vs_sign_chunk  # class=wiring-shape file=roshera-backend/export-engine/src/formats/ros.rs:928 date=2026-08-10 diag: instance 3 -- `options.sign` (a bool) and the chunk table are two independently-set values; a textual grep sees both used. Now cross-checked by check_signature_claim_matches_table (ros.rs:928) with tests both directions. Gate = invariant-consistency test, claim <=> artifact.
# INSTANCE 4 -- merkle.rs (compute_merkle_root, BatchMerkleProof::verify_all).
# NOT an entry line here: it is the one instance of the fourteen the automated
# scan actually catches, so its JUDGED entries live in SECTION B under
# `ros-format::compute_merkle_root` and `ros-format::BatchMerkleProof`. Listing
# it twice would make the gate both judge it and print it as informational.
# Diagnosis: merkle.rs is reachable via ros-format/src/lib.rs:41
# `pub mod merkle`, so rustc's dead_code never fires; export-engine's
# formats/ros.rs:57 imports only HashAlgorithm + MerkleTree from that module.
# INSTANCE 5 -- intent enforced then dropped (IntentContext, INTENT_OVERRIDE,
# timeline-engine/src/recorder_bridge.rs:115 and :135).
# NOT an entry line here: the scan ALSO flags it, so its JUDGED entry lives in
# SECTION B as `timeline-engine::IntentContext`. Listing it twice would make the
# gate both judge it and print it as informational, and the file would say two
# different classes about one symbol.
# Diagnosis: the wiring-shape story is that intent is READ
# (`if operation.facets.intent().is_none()`) but an upstream code path stopped
# SETTING it -- a data-flow gap no reachability check sees. The dead-symbol
# story is separate and narrower: the type NAME `IntentContext` occurs in no
# production file but recorder_bridge.rs, because every consumer goes through
# the `facets.intent()` accessor. Both are true; only the second is machine-
# checkable. The real gate is a regression test pinning "intent set at the MCP
# gate survives to the stored event".
#
# CLOSED 2026-08-15. Both entry lines are REMOVED from Section B (the ratchet
# fired exit 2 on them, which is how this was noticed). The gate this diagnosis
# asked for now exists, and it was written because the defect was still LIVE:
# `boolean_route_carries_declared_facets_across_the_spawn_blocking_boundary`
# (api-server/src/router_integration_tests.rs) drives a real boolean union
# through the full router with the intent, agent and document headers set, and
# asserts the facets reach the recorded event. It went RED first -- author came
# back `System` instead of the declared agent, with no intent facet at all --
# because `bounded_model_op` runs the kernel call in `spawn_blocking` and tokio
# task-locals are not inherited across that boundary. So this instance's
# "upstream code path stopped SETTING it" was still true, five years of prose
# later, on the one route where part identity changes. Fixed at the choke point
# in api-server/src/bounded_exec.rs (`snapshot_request_scope` /
# `with_request_scope`), which re-enters only the overrides actually present --
# an absent one stays absent rather than being materialised as a default.
ros-format::verify_crc  # class=wiring-shape file=roshera-backend/ros-format/src/chunk.rs:272 date=2026-08-10 diag: instance 6 -- verify_crc exists and is called (chunk.rs:413 and in tests); the historical gap was the READER not calling it on the declared/on-disk value during import. A call-site omission in ONE path is invisible to symbol reachability. Gate = import-path integration test that tampers chunk bytes and asserts refusal.
ros-format::header_ai_hints  # class=wiring-shape file=roshera-backend/ros-format/src/header.rs date=2026-08-10 diag: instance 7 -- NO SYMBOL EXISTS. Re-verified 2026-08-10: zero grep hits for ai_hints/AiHints/AI_HINTS in ros-format/src. If this was ever a doc-comment promise with no backing field, no code gate catches prose. Resolution is a test that pins the claim, or deletion of the claim.
roshera-app::establishAcpSession  # class=out-of-scope file=roshera-app/src/lib/acp-blackboard.ts:571 date=2026-08-10 diag: instance 8 -- at introduction it was exported with ZERO importers. Now called at App.tsx:111 and imported by ProviderSettingsDialog.tsx:32. This is the TypeScript dead-export case; ts-prune is the right tool and is blocked on disk today (design doc Â§4). Out of scope until then.
api-server::rename_document  # class=out-of-scope file=roshera-backend/api-server/src/documents.rs:503 date=2026-08-10 diag: instance 9 -- `axum::routing::patch(documents::rename_document)` is a real, compiling call site, so no textual tool can distinguish "wired into the router" from "reachable by an actual client". Caught instead by this repo's ontology drift gate (agent_registry.rs), which now carries `document_rename` as tool 104.
geometry-engine::twist_angle  # class=out-of-scope file=roshera-backend/geometry-engine/src/operations/extrude.rs:508 date=2026-08-10 diag: instance 10 -- a kernel struct field that is READ internally (extrude.rs:567,646) but absent from the AI-facing surface; ai_operations_registry.rs:921 hardcodes 0.0. Compiles clean either way, so not a reachability gap. Gate = registry-drift: kernel op structs vs ai_operations_registry.
geometry-engine::create_helical_sweep  # class=out-of-scope file=roshera-backend/geometry-engine/src/operations/revolve.rs:2352 date=2026-08-10 diag: instance 11 -- private fn, called internally at revolve.rs:111, never exposed through registry/REST/MCP. api-server/src/main.rs:4829 documents the refusal. Gate = kernel-op-list vs exposed-surface-list diff, same shape as instance 9.
shared-types::ThreadSpecification  # class=wiring-shape file=roshera-backend/shared-types/src/geometry_commands.rs:1225 date=2026-08-10 diag: instance 12 -- a Deserialize-derived type nothing constructs from a live request path. serde derives are their own "use", and it is re-exported at lib.rs:49 and used as a field at geometry_commands.rs:587, so every symbol scan calls it connected. Gate = a REST/WS test that sends a wire payload and asserts round-trip through an ACTUAL handler.
geometry-engine::mould_recipe_fillet_seam  # class=out-of-scope file=roshera-backend/geometry-engine/src/operations/mould.rs:31 date=2026-08-10 diag: instance 13 (NEWEST) -- the mould verb, "the agent's core edit loop". Verified 2026-08-10: Recipe, Step and set_dimension have ZERO references outside mould.rs workspace-wide, and Step covers only Box + ExtrudeSquare, so the recipe cannot reach the fillet seam at all. Genuine dead-symbol, but in the unscoped crate; keys Recipe/Step are too generic to judge safely.
geometry-engine::create_pattern  # class=out-of-scope file=roshera-backend/geometry-engine/src/operations/pattern.rs:171 date=2026-08-10 diag: instance 14 (NEWEST) -- create_pattern records NOTHING. Verified 2026-08-10: pattern.rs contains zero occurrences of record_operation / OperationRecorder, violating the documented invariant that "every kernel entry point that mutates topology emits a RecordedOperation on success". It IS called (ai_operations_registry.rs:1051), so it is wiring-shape in substance; filed out-of-scope because the crate is unscoped. Gate = recorder-coverage assertion over the operations module.

# =============================================================================
# SECTION B -- BASELINE STOCK: machine-generated, judged, ratcheted
# =============================================================================
# Seeded 2026-08-10 from `disconnection-gate.ps1 -Seed` at HEAD
# (feat/sketch-dcm-45). Every line is a `pub` item in a scoped crate that no
# production file other than its own declaring file mentions anywhere in the
# workspace.
#
# This is stock, not a to-do list with a deadline. Burn it down
# OPPORTUNISTICALLY: whenever a branch touches a file with an entry here, that
# entry either gets wired (delete the line) or gets re-justified. Do not open a
# sprint to clear it -- fixing the stock before shipping the gate is how review
# failed fourteen times.
#
# A line here means one of three things, all of them the same smell:
#   * genuinely dead code -> delete it;
#   * a capability built and never wired -> wire it, and add the call-site test;
#   * over-broad visibility -> demote to pub(crate)/private and the line goes away.
#
# RE-SEEDING, AND THE ONE WAY TO GET IT WRONG
# `-Seed` prints SECTION B LINES ONLY, to stdout. Paste them BELOW this marker,
# replacing the previous block.
#
#   NEVER `disconnection-gate.ps1 -Seed > KNOWN_DISCONNECTED.md`.
#
# That redirect wipes the header, the ratchet rule, the scope rationale and all
# fourteen diagnoses -- the only part of this file that is not regenerable in
# thirty seconds -- and the gate would then pass clean forever with zero
# classified stock and nobody would notice.
#
# ALWAYS regenerate with `-Seed` from this script, never from a second
# implementation: another tokenizer will disagree and the first run will not be
# clean.

ai-integration::AdvancedAudioProcessor  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor_advanced.rs:6 date=2026-08-10
ai-integration::all_available  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/native_factory.rs:163 date=2026-08-10
ai-integration::allowed_provider_ids  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/allowlist.rs:412 date=2026-08-10
ai-integration::aluminum_6061  # class=dead-symbol file=roshera-backend/ai-integration/src/knowledge/roshera_knowledge.rs:151 date=2026-08-10
ai-integration::AudioProcessor  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor.rs:5 date=2026-08-10
ai-integration::check_provider_availability  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/native_factory.rs:147 date=2026-08-10
ai-integration::CollaborationContext  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:92 date=2026-08-10
ai-integration::CollaborativeAction  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:115 date=2026-08-10
ai-integration::ContextAnalyzer  # class=dead-symbol file=roshera-backend/ai-integration/src/commands/parser.rs:99 date=2026-08-10
ai-integration::ContextualSuggestion  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:169 date=2026-08-10
ai-integration::EndpointCapabilities  # class=dead-symbol file=roshera-backend/ai-integration/src/universal_endpoint.rs:48 date=2026-08-10
ai-integration::ExecutorError  # class=dead-symbol file=roshera-backend/ai-integration/src/executor.rs:515 date=2026-08-10
ai-integration::get_command_examples  # class=dead-symbol file=roshera-backend/ai-integration/src/knowledge/roshera_knowledge.rs:75 date=2026-08-10
ai-integration::get_noise_level  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor_advanced.rs:471 date=2026-08-10
ai-integration::get_speech_probability  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor_advanced.rs:476 date=2026-08-10
ai-integration::has_tts_provider  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/mod.rs:366 date=2026-08-10
ai-integration::is_voice_active  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor.rs:241 date=2026-08-10
ai-integration::min_thickness_for_load  # class=dead-symbol file=roshera-backend/ai-integration/src/knowledge/roshera_knowledge.rs:182 date=2026-08-10
ai-integration::missing_providers  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/native_factory.rs:168 date=2026-08-10
ai-integration::NaturalLanguageParser  # class=dead-symbol file=roshera-backend/ai-integration/src/commands/parser.rs:26 date=2026-08-10
ai-integration::parse_intent  # class=dead-symbol file=roshera-backend/ai-integration/src/knowledge/roshera_knowledge.rs:88 date=2026-08-10
ai-integration::parse_to_intent  # class=dead-symbol file=roshera-backend/ai-integration/src/parser.rs:148 date=2026-08-10
ai-integration::ProviderAvailability  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/native_factory.rs:156 date=2026-08-10
ai-integration::ProviderSelectionRefusal  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/allowlist.rs:389 date=2026-08-10
ai-integration::ScenePattern  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:143 date=2026-08-10
ai-integration::set_tool_tier  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/claude.rs:175 date=2026-08-10
ai-integration::SimpleAudioProcessor  # class=dead-symbol file=roshera-backend/ai-integration/src/audio_processor_simple.rs:6 date=2026-08-10
ai-integration::SimpleParser  # class=dead-symbol file=roshera-backend/ai-integration/src/parser.rs:125 date=2026-08-10
ai-integration::SkillLevel  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:83 date=2026-08-10
ai-integration::steel_304  # class=dead-symbol file=roshera-backend/ai-integration/src/knowledge/roshera_knowledge.rs:161 date=2026-08-10
ai-integration::SuggestionCategory  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:181 date=2026-08-10
ai-integration::TranslationError  # class=dead-symbol file=roshera-backend/ai-integration/src/translator.rs:114 date=2026-08-10
ai-integration::UniversalEndpointError  # class=dead-symbol file=roshera-backend/ai-integration/src/providers/universal_endpoint.rs:23 date=2026-08-10
ai-integration::update_config  # class=dead-symbol file=roshera-backend/ai-integration/src/pipeline/smart_router.rs:308 date=2026-08-10
ai-integration::UserPreferences  # class=dead-symbol file=roshera-backend/ai-integration/src/context_builder.rs:68 date=2026-08-10
ai-integration::with_model  # class=dead-symbol file=roshera-backend/ai-integration/src/executor.rs:64 date=2026-08-10
assembly-engine::assemblable_phase1  # class=dead-symbol file=roshera-backend/assembly-engine/src/report.rs:23 date=2026-08-10
assembly-engine::mate_violations  # class=dead-symbol file=roshera-backend/assembly-engine/src/solve_input.rs:66 date=2026-08-10
assembly-engine::MateEnforcement  # class=dead-symbol file=roshera-backend/assembly-engine/src/mate_residual.rs:70 date=2026-08-10
assembly-engine::MateEnforcementReport  # class=dead-symbol file=roshera-backend/assembly-engine/src/mate_residual.rs:78 date=2026-08-10
assembly-engine::phase1_report  # class=dead-symbol file=roshera-backend/assembly-engine/src/report.rs:35 date=2026-08-10
assembly-engine::static_contradictory_pairs  # class=dead-symbol file=roshera-backend/assembly-engine/src/constrainedness.rs:319 date=2026-08-10
assembly-engine::structural_rank  # class=dead-symbol file=roshera-backend/assembly-engine/src/decompose.rs:131 date=2026-08-10
assembly-engine::sweep_driven  # class=dead-symbol file=roshera-backend/assembly-engine/src/sweep.rs:667 date=2026-08-10
export-engine::add_failed  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/diagnostics.rs:203 date=2026-08-10
export-engine::add_unsupported  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/diagnostics.rs:198 date=2026-08-10
export-engine::ADVANCED_FACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:894 date=2026-08-10
export-engine::AdvancedFaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:892 date=2026-08-10
export-engine::application_context  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:2002 date=2026-08-10
export-engine::as_enum  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/params.rs:179 date=2026-08-10
export-engine::as_string  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/params.rs:139 date=2026-08-10
export-engine::as_typed  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/params.rs:303 date=2026-08-10
export-engine::AXIS2_PLACEMENT_3D_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:282 date=2026-08-10
export-engine::Axis2Placement3DHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:280 date=2026-08-10
export-engine::B_SPLINE_CURVE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/bspline.rs:61 date=2026-08-10
export-engine::B_SPLINE_SURFACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/bspline.rs:246 date=2026-08-10
export-engine::begin_data  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:162 date=2026-08-10
export-engine::BREP_WITH_VOIDS_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/solid.rs:31 date=2026-08-10
export-engine::BRepMetadata  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros_snapshot.rs:223 date=2026-08-10
export-engine::BrepWithVoidsHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/solid.rs:29 date=2026-08-10
export-engine::BSplineCurveHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/bspline.rs:59 date=2026-08-10
export-engine::BSplineSurfaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/bspline.rs:244 date=2026-08-10
export-engine::CARTESIAN_POINT_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:76 date=2026-08-10
export-engine::CartesianPointHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:74 date=2026-08-10
export-engine::CIRCLE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:490 date=2026-08-10
export-engine::CircleHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:488 date=2026-08-10
export-engine::CLOSED_SHELL_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:1119 date=2026-08-10
export-engine::ClosedShellHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:1117 date=2026-08-10
export-engine::command_type_for_kind  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros_provenance.rs:184 date=2026-08-10
export-engine::command_type_for_operation  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros_provenance.rs:152 date=2026-08-10
export-engine::CONICAL_SURFACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:333 date=2026-08-10
export-engine::ConicalSurfaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:331 date=2026-08-10
export-engine::CYLINDRICAL_SURFACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:657 date=2026-08-10
export-engine::CylindricalSurfaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:655 date=2026-08-10
export-engine::DIRECTION_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:141 date=2026-08-10
export-engine::DirectionHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:139 date=2026-08-10
export-engine::EDGE_CURVE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:157 date=2026-08-10
export-engine::EDGE_LOOP_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:647 date=2026-08-10
export-engine::EdgeCurveHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:155 date=2026-08-10
export-engine::EdgeLoopHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:645 date=2026-08-10
export-engine::end_data  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:167 date=2026-08-10
export-engine::estimate_export_time  # class=dead-symbol file=roshera-backend/export-engine/src/lib.rs:40 date=2026-08-10
export-engine::ExportValidator  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:28 date=2026-08-10
export-engine::FACE_BOUND_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:762 date=2026-08-10
export-engine::FACE_OUTER_BOUND_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:787 date=2026-08-10
export-engine::FaceBoundHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:760 date=2026-08-10
export-engine::FaceOuterBoundHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:785 date=2026-08-10
export-engine::FacePcurve  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/pcurve.rs:110 date=2026-08-10
export-engine::fork_point_seq  # class=dead-symbol file=roshera-backend/export-engine/src/formats/timeline_chunk.rs:50 date=2026-08-10
export-engine::import_step_content  # class=dead-symbol file=roshera-backend/export-engine/src/engine.rs:202 date=2026-08-10
export-engine::INTENT_UNPARSEABLE_TAG  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros_provenance.rs:50 date=2026-08-10
export-engine::LINE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:396 date=2026-08-10
export-engine::LineHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:394 date=2026-08-10
export-engine::MANIFOLD_SOLID_BREP_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:1216 date=2026-08-10
export-engine::ManifoldSolidBrepHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:1214 date=2026-08-10
export-engine::MAPPED_ITEM_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/assembly.rs:60 date=2026-08-10
export-engine::MappedItemHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/assembly.rs:58 date=2026-08-10
export-engine::MeshOptimizer  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:75 date=2026-08-10
export-engine::MeshStatistics  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:18 date=2026-08-10
export-engine::OPEN_SHELL_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/shells.rs:29 date=2026-08-10
export-engine::OpenShellHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier3/shells.rs:27 date=2026-08-10
export-engine::optimize_for_export  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:94 date=2026-08-10
export-engine::ORIENTED_EDGE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:551 date=2026-08-10
export-engine::OrientedEdgeHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/topology.rs:549 date=2026-08-10
export-engine::ParamError  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/params.rs:27 date=2026-08-10
export-engine::PLANE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:578 date=2026-08-10
export-engine::PlaneHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:576 date=2026-08-10
export-engine::preprocess  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/parser.rs:34 date=2026-08-10
export-engine::protocol_definition  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:2014 date=2026-08-10
export-engine::ResolveOutcome  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/resolver.rs:45 date=2026-08-10
export-engine::RosChunkSummary  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:1125 date=2026-08-10
export-engine::RosKeyRecoverability  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:1152 date=2026-08-10
export-engine::RosReplayFailure  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:271 date=2026-08-10
export-engine::RosReplayStatus  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:290 date=2026-08-10
export-engine::RosSignatureVerdict  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:191 date=2026-08-10
export-engine::RosWriteSignature  # class=dead-symbol file=roshera-backend/export-engine/src/formats/ros.rs:258 date=2026-08-10
export-engine::schema_name  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1984 date=2026-08-10
export-engine::SHAPE_REPRESENTATION_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/root.rs:68 date=2026-08-10
export-engine::ShapeRepresentationHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/root.rs:66 date=2026-08-10
export-engine::SPHERICAL_SURFACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:63 date=2026-08-10
export-engine::SphericalSurfaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:61 date=2026-08-10
export-engine::StepApplicationProtocol  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1959 date=2026-08-10
export-engine::StepExportOptions  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1941 date=2026-08-10
export-engine::StepHeader  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:27 date=2026-08-10
export-engine::StepId  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:66 date=2026-08-10
export-engine::StepWriter  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:75 date=2026-08-10
export-engine::SURFACE_OF_LINEAR_EXTRUSION_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/swept.rs:212 date=2026-08-10
export-engine::SURFACE_OF_REVOLUTION_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/swept.rs:84 date=2026-08-10
export-engine::SurfaceOfLinearExtrusionHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/swept.rs:210 date=2026-08-10
export-engine::SurfaceOfRevolutionHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/swept.rs:82 date=2026-08-10
export-engine::TOROIDAL_SURFACE_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:175 date=2026-08-10
export-engine::ToroidalSurfaceHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier2/analytic.rs:173 date=2026-08-10
export-engine::UNIT_CONTEXT_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/units.rs:500 date=2026-08-10
export-engine::UNIT_DECLARATION_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/units.rs:246 date=2026-08-10
export-engine::UnitContextHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/units.rs:499 date=2026-08-10
export-engine::UnitDeclarationHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/units.rs:243 date=2026-08-10
export-engine::validate_for_export  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:37 date=2026-08-10
export-engine::VECTOR_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:199 date=2026-08-10
export-engine::VectorHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:197 date=2026-08-10
export-engine::VERTEX_POINT_HANDLER  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:768 date=2026-08-10
export-engine::VertexPointHandler  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/handlers/tier1/geometry.rs:766 date=2026-08-10
export-engine::with_protocol  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:96 date=2026-08-10
export-engine::with_target_triangles  # class=dead-symbol file=roshera-backend/export-engine/src/validation.rs:88 date=2026-08-10
export-engine::write_assembly_constraint  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1655 date=2026-08-10
export-engine::write_assembly_structure  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1682 date=2026-08-10
export-engine::write_axis1_placement  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:304 date=2026-08-10
export-engine::write_axis2_placement_3d  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:231 date=2026-08-10
export-engine::write_b_spline_curve  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:363 date=2026-08-10
export-engine::write_cartesian_point  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:177 date=2026-08-10
export-engine::write_circle  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:342 date=2026-08-10
export-engine::write_curve  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:458 date=2026-08-10
export-engine::write_direction  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:204 date=2026-08-10
export-engine::write_end  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:172 date=2026-08-10
export-engine::write_line  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:321 date=2026-08-10
export-engine::write_shell  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:1261 date=2026-08-10
export-engine::write_surface  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:601 date=2026-08-10
export-engine::write_vector  # class=dead-symbol file=roshera-backend/export-engine/src/formats/step/writer.rs:216 date=2026-08-10
ros-format::AccessContext  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:166 date=2026-08-10
ros-format::AccessControlEntry  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:47 date=2026-08-10
ros-format::AccessControlManager  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:185 date=2026-08-10
ros-format::AccessLevel  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:14 date=2026-08-10
ros-format::add_chunk_key  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:302 date=2026-08-10
ros-format::ai_provenance  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:144 date=2026-08-10
ros-format::AIProvenanceHeader  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:113 date=2026-08-10
ros-format::as_u8  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:62 date=2026-08-10
ros-format::AuditContext  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:161 date=2026-08-10
ros-format::AuditEntry  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:108 date=2026-08-10
ros-format::AuditEvent  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:14 date=2026-08-10
ros-format::AuditExport  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:410 date=2026-08-10
ros-format::AuditFilter  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:170 date=2026-08-10
ros-format::AuditSeverity  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:99 date=2026-08-10
ros-format::AuditStatistics  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:426 date=2026-08-10
ros-format::BatchMerkleProof  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:411 date=2026-08-10
ros-format::calculate_hashes  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:264 date=2026-08-10
ros-format::can_decrypt_chunk  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:196 date=2026-08-10
ros-format::can_perform  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:40 date=2026-08-10
ros-format::ChainBreakWitness  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:186 date=2026-08-10
ros-format::ChainVerdict  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:197 date=2026-08-10
ros-format::check_access  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:233 date=2026-08-10
ros-format::CommandFilter  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:319 date=2026-08-10
ros-format::CommandSummary  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:603 date=2026-08-10
ros-format::compliance_mode  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:99 date=2026-08-10
ros-format::ComplianceExport  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:616 date=2026-08-10
ros-format::compute_merkle_root  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:441 date=2026-08-10
ros-format::CURRENT_MAJOR_VERSION  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:43 date=2026-08-10
ros-format::CURRENT_PATCH_VERSION  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:45 date=2026-08-10
ros-format::current_time_secs  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:57 date=2026-08-10
ros-format::end_session  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:401 date=2026-08-10
ros-format::escrow_key_set  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:562 date=2026-08-10
ros-format::export_acl  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:303 date=2026-08-10
ros-format::export_for_compliance  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:578 date=2026-08-10
ros-format::FeatureFlags  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:130 date=2026-08-10
ros-format::FileHeaderBuilder  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:565 date=2026-08-10
ros-format::FileSignatureMetadata  # class=dead-symbol file=roshera-backend/ros-format/src/signature.rs:28 date=2026-08-10
ros-format::find_all_by_type  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:459 date=2026-08-10
ros-format::from_hex  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:194 date=2026-08-10
ros-format::from_u32  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:24 date=2026-08-10
ros-format::generate_chunk_iv  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:304 date=2026-08-10
ros-format::generate_proof  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:176 date=2026-08-10
ros-format::get_command  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:495 date=2026-08-10
ros-format::get_effective_permissions  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:277 date=2026-08-10
ros-format::grant_access  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:204 date=2026-08-10
ros-format::hash_hex  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:105 date=2026-08-10
ros-format::hash_size  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:31 date=2026-08-10
ros-format::header_crc_input  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:71 date=2026-08-10
ros-format::hmac_sha256  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:39 date=2026-08-10
ros-format::INTEGRITY_SCHEME_V2_MIN_MINOR  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:49 date=2026-08-10
ros-format::into_tracker  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:675 date=2026-08-10
ros-format::io_context  # class=dead-symbol file=roshera-backend/ros-format/src/error.rs:825 date=2026-08-10
ros-format::is_compressed  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:361 date=2026-08-10
ros-format::is_encrypted  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:397 date=2026-08-10
ros-format::is_intact  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:204 date=2026-08-10
ros-format::is_required  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:92 date=2026-08-10
ros-format::is_supported  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:474 date=2026-08-10
ros-format::key_size_bytes  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:100 date=2026-08-10
ros-format::KeyAlgorithm  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:78 date=2026-08-10
ros-format::KeyEntry  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:157 date=2026-08-10
ros-format::KeyEscrowService  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:552 date=2026-08-10
ros-format::KeysHeader  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:137 date=2026-08-10
ros-format::KeyType  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:112 date=2026-08-10
ros-format::matches_filter  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:271 date=2026-08-10
ros-format::MAX_CHUNK_SIZE  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:23 date=2026-08-10
ros-format::maximum_privacy  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:87 date=2026-08-10
ros-format::MerkleHash  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:18 date=2026-08-10
ros-format::MerkleHash512  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:21 date=2026-08-10
ros-format::MerkleNode  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:41 date=2026-08-10
ros-format::MerkleProof  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:336 date=2026-08-10
ros-format::ms_to_system_time  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:65 date=2026-08-10
ros-format::nonce_size  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:55 date=2026-08-10
ros-format::ProofNode  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:329 date=2026-08-10
ros-format::random_32  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:126 date=2026-08-10
ros-format::random_u64  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:133 date=2026-08-10
ros-format::read_all  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:528 date=2026-08-10
ros-format::read_block  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:456 date=2026-08-10
ros-format::recover_key_set  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:598 date=2026-08-10
ros-format::revoke_access  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:222 date=2026-08-10
ros-format::root_hash_hex  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:171 date=2026-08-10
ros-format::ROSHERA_KDF_MEMORY_KIB  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:371 date=2026-08-10
ros-format::ROSHERA_KDF_TIME_COST  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:380 date=2026-08-10
ros-format::ROSHERA_KDF_TIME_COST_MAX  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:387 date=2026-08-10
ros-format::ROSHERA_MAGIC  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:14 date=2026-08-10
ros-format::SecureRng  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:100 date=2026-08-10
ros-format::SecureString  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:75 date=2026-08-10
ros-format::SecurityAuditLog  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:210 date=2026-08-10
ros-format::SessionInfo  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:340 date=2026-08-10
ros-format::sha512  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:29 date=2026-08-10
ros-format::should_encrypt  # class=dead-symbol file=roshera-backend/ros-format/src/chunk.rs:97 date=2026-08-10
ros-format::should_track_parameters  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:49 date=2026-08-10
ros-format::should_track_responses  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:45 date=2026-08-10
ros-format::SignatureRecord  # class=dead-symbol file=roshera-backend/ros-format/src/signature.rs:48 date=2026-08-10
ros-format::STANDARD_CHUNK_FOURCCS  # class=dead-symbol file=roshera-backend/ros-format/src/keys.rs:396 date=2026-08-10
ros-format::start_session  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:380 date=2026-08-10
ros-format::StreamingDecryptor  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:435 date=2026-08-10
ros-format::StreamingEncryptor  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:325 date=2026-08-10
ros-format::tag_size  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:63 date=2026-08-10
ros-format::validate_chain  # class=dead-symbol file=roshera-backend/ros-format/src/aipr.rs:500 date=2026-08-10
ros-format::verify_all  # class=dead-symbol file=roshera-backend/ros-format/src/merkle.rs:423 date=2026-08-10
ros-format::verify_auth_tag  # class=dead-symbol file=roshera-backend/ros-format/src/encryption.rs:546 date=2026-08-10
ros-format::verify_chain  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:323 date=2026-08-10
ros-format::verify_signature  # class=dead-symbol file=roshera-backend/ros-format/src/signature.rs:129 date=2026-08-10
ros-format::with_ai_provenance  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:160 date=2026-08-10
ros-format::with_constraint  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:68 date=2026-08-10
ros-format::with_context  # class=dead-symbol file=roshera-backend/ros-format/src/audit.rs:136 date=2026-08-10
ros-format::with_default_level  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:198 date=2026-08-10
ros-format::with_expiration  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:73 date=2026-08-10
ros-format::with_feature_flags  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:616 date=2026-08-10
ros-format::with_file_size  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:605 date=2026-08-10
ros-format::with_index_info  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:610 date=2026-08-10
ros-format::with_mfa  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:158 date=2026-08-10
ros-format::with_role  # class=dead-symbol file=roshera-backend/ros-format/src/access.rs:153 date=2026-08-10
ros-format::with_timestamped  # class=dead-symbol file=roshera-backend/ros-format/src/header.rs:164 date=2026-08-10
ros-format::xor_bytes  # class=dead-symbol file=roshera-backend/ros-format/src/util.rs:181 date=2026-08-10
session-manager::add_user  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:455 date=2026-08-10
session-manager::apply_local  # class=dead-symbol file=roshera-backend/session-manager/src/conflict_resolution.rs:346 date=2026-08-10
session-manager::attach_api_key_store  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:790 date=2026-08-10
session-manager::auth_manager_arc  # class=dead-symbol file=roshera-backend/session-manager/src/manager.rs:75 date=2026-08-10
session-manager::CacheEntry  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:20 date=2026-08-10
session-manager::CacheManagerStats  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:528 date=2026-08-10
session-manager::can_access_object  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:563 date=2026-08-10
session-manager::check_permission  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:319 date=2026-08-10
session-manager::check_rate_limit  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:1243 date=2026-08-10
session-manager::CollaborationError  # class=dead-symbol file=roshera-backend/session-manager/src/collaboration.rs:173 date=2026-08-10
session-manager::CollaborationEventType  # class=dead-symbol file=roshera-backend/session-manager/src/collaboration.rs:75 date=2026-08-10
session-manager::CollaborationTracker  # class=dead-symbol file=roshera-backend/session-manager/src/collaboration.rs:49 date=2026-08-10
session-manager::create_api_key  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:734 date=2026-08-10
session-manager::create_session_channel  # class=dead-symbol file=roshera-backend/session-manager/src/broadcast.rs:188 date=2026-08-10
session-manager::create_session_permissions  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:287 date=2026-08-10
session-manager::DatabaseConfig  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:83 date=2026-08-10
session-manager::DatabaseType  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:74 date=2026-08-10
session-manager::DeltaStatistics  # class=dead-symbol file=roshera-backend/session-manager/src/delta_manager.rs:24 date=2026-08-10
session-manager::DeltaType  # class=dead-symbol file=roshera-backend/session-manager/src/delta.rs:17 date=2026-08-10
session-manager::deny_permission  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:420 date=2026-08-10
session-manager::enable_2fa  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:1047 date=2026-08-10
session-manager::geometry_cache_key  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:577 date=2026-08-10
session-manager::GeometryMetadata  # class=dead-symbol file=roshera-backend/session-manager/src/command_processor.rs:29 date=2026-08-10
session-manager::get_security_events  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:1156 date=2026-08-10
session-manager::get_session_users  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:584 date=2026-08-10
session-manager::grant_permission  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:385 date=2026-08-10
session-manager::hit_ratio  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:82 date=2026-08-10
session-manager::is_locked_out  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:1034 date=2026-08-10
session-manager::object_created  # class=dead-symbol file=roshera-backend/session-manager/src/broadcast.rs:108 date=2026-08-10
session-manager::ObjectDelta  # class=dead-symbol file=roshera-backend/session-manager/src/delta.rs:30 date=2026-08-10
session-manager::ObjectMetadata  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:261 date=2026-08-10
session-manager::ObjectPermissions  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:219 date=2026-08-10
session-manager::ot_engine  # class=dead-symbol file=roshera-backend/session-manager/src/manager.rs:561 date=2026-08-10
session-manager::overall_hit_ratio  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:551 date=2026-08-10
session-manager::PasswordRequirements  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:271 date=2026-08-10
session-manager::permission_manager  # class=dead-symbol file=roshera-backend/session-manager/src/manager.rs:80 date=2026-08-10
session-manager::PersistenceManager  # class=dead-symbol file=roshera-backend/session-manager/src/persistence.rs:10 date=2026-08-10
session-manager::PostgresDatabase  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:367 date=2026-08-10
session-manager::RateLimit  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:115 date=2026-08-10
session-manager::record_login_attempt  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:988 date=2026-08-10
session-manager::register_rule  # class=dead-symbol file=roshera-backend/session-manager/src/conflict_resolution.rs:263 date=2026-08-10
session-manager::remove_user  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:494 date=2026-08-10
session-manager::restore_api_key  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:843 date=2026-08-10
session-manager::SecurityEvent  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:162 date=2026-08-10
session-manager::SessionMetadata  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:248 date=2026-08-10
session-manager::SessionPermissions  # class=dead-symbol file=roshera-backend/session-manager/src/permissions.rs:244 date=2026-08-10
session-manager::SessionStateSnapshot  # class=dead-symbol file=roshera-backend/session-manager/src/broadcast.rs:152 date=2026-08-10
session-manager::SessionTimelineStats  # class=dead-symbol file=roshera-backend/session-manager/src/timeline_integration.rs:407 date=2026-08-10
session-manager::SqliteDatabase  # class=dead-symbol file=roshera-backend/session-manager/src/database.rs:1855 date=2026-08-10
session-manager::start_maintenance  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:317 date=2026-08-10
session-manager::state_vector  # class=dead-symbol file=roshera-backend/session-manager/src/conflict_resolution.rs:417 date=2026-08-10
session-manager::TimelineDelta  # class=dead-symbol file=roshera-backend/session-manager/src/delta.rs:66 date=2026-08-10
session-manager::TimelineIntegration  # class=dead-symbol file=roshera-backend/session-manager/src/timeline_integration.rs:22 date=2026-08-10
session-manager::total_memory_bytes  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:543 date=2026-08-10
session-manager::TransformedOperation  # class=dead-symbol file=roshera-backend/session-manager/src/conflict_resolution.rs:137 date=2026-08-10
session-manager::TransformRule  # class=dead-symbol file=roshera-backend/session-manager/src/conflict_resolution.rs:123 date=2026-08-10
session-manager::TtlLruCache  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:93 date=2026-08-10
session-manager::TwoFactorAuth  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:147 date=2026-08-10
session-manager::user_joined  # class=dead-symbol file=roshera-backend/session-manager/src/broadcast.rs:95 date=2026-08-10
session-manager::user_left  # class=dead-symbol file=roshera-backend/session-manager/src/broadcast.rs:100 date=2026-08-10
session-manager::UserActivity  # class=dead-symbol file=roshera-backend/session-manager/src/collaboration.rs:58 date=2026-08-10
session-manager::UserChanges  # class=dead-symbol file=roshera-backend/session-manager/src/delta.rs:77 date=2026-08-10
session-manager::verify_2fa  # class=dead-symbol file=roshera-backend/session-manager/src/auth.rs:1086 date=2026-08-10
session-manager::WarmupData  # class=dead-symbol file=roshera-backend/session-manager/src/cache.rs:517 date=2026-08-10
shared-types::abs_plastic  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:197 date=2026-08-10
shared-types::add_entity_to_sketch  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:400 date=2026-08-10
shared-types::add_material  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:292 date=2026-08-10
shared-types::add_sketch_plane  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:362 date=2026-08-10
shared-types::analytical_properties  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:369 date=2026-08-10
shared-types::AnalyticalProperties  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:146 date=2026-08-10
shared-types::approx_zero  # class=dead-symbol file=roshera-backend/shared-types/src/lib.rs:152 date=2026-08-10
shared-types::BatchRequest  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:220 date=2026-08-10
shared-types::BatchResponse  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:231 date=2026-08-10
shared-types::BatchResult  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:244 date=2026-08-10
shared-types::by_category  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:302 date=2026-08-10
shared-types::CachedMesh  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:181 date=2026-08-10
shared-types::CameraInfo  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:47 date=2026-08-10
shared-types::ClippingPlane  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:339 date=2026-08-10
shared-types::CommandInfo  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:185 date=2026-08-10
shared-types::convert_from_mm  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:489 date=2026-08-10
shared-types::current_history  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:349 date=2026-08-10
shared-types::CursorTarget  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:87 date=2026-08-10
shared-types::CurveParameters  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:577 date=2026-08-10
shared-types::DisplaySettings  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:307 date=2026-08-10
shared-types::EnvironmentContext  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:256 date=2026-08-10
shared-types::ErrorResponse  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:153 date=2026-08-10
shared-types::expand_to_include  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:471 date=2026-08-10
shared-types::GeometryRepresentation  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:200 date=2026-08-10
shared-types::GeometryStats  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:196 date=2026-08-10
shared-types::get_analytical_properties  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:271 date=2026-08-10
shared-types::get_cached_mesh  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:374 date=2026-08-10
shared-types::get_display_mesh  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:406 date=2026-08-10
shared-types::get_mesh_for_display  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:217 date=2026-08-10
shared-types::get_object_mut  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:312 date=2026-08-10
shared-types::get_sketch_plane  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:389 date=2026-08-10
shared-types::get_sketch_plane_mut  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:394 date=2026-08-10
shared-types::GridInfo  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:281 date=2026-08-10
shared-types::has_valid_mesh_cache  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:237 date=2026-08-10
shared-types::HealthMetrics  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:181 date=2026-08-10
shared-types::HealthResponse  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:166 date=2026-08-10
shared-types::invalidate_mesh_cache  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:398 date=2026-08-10
shared-types::is_analytical  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:359 date=2026-08-10
shared-types::is_retryable  # class=dead-symbol file=roshera-backend/shared-types/src/error.rs:327 date=2026-08-10
shared-types::LightInfo  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:320 date=2026-08-10
shared-types::MaterialCategory  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:32 date=2026-08-10
shared-types::MaterialInfo  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:177 date=2026-08-10
shared-types::MaterialLibrary  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:113 date=2026-08-10
shared-types::MechanicalProperties  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:98 date=2026-08-10
shared-types::modifies_geometry  # class=dead-symbol file=roshera-backend/shared-types/src/commands.rs:316 date=2026-08-10
shared-types::MouseContext  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:275 date=2026-08-10
shared-types::MousePosition  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:257 date=2026-08-10
shared-types::new_analytical_object  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:334 date=2026-08-10
shared-types::PhysicalProperties  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:51 date=2026-08-10
shared-types::PixelPosition  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:266 date=2026-08-10
shared-types::PrecisionSettings  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:322 date=2026-08-10
shared-types::remove_sketch_plane  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:370 date=2026-08-10
shared-types::RenderStats  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:349 date=2026-08-10
shared-types::RosheraResult  # class=dead-symbol file=roshera-backend/shared-types/src/lib.rs:157 date=2026-08-10
shared-types::SelectionInfo  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:212 date=2026-08-10
shared-types::SessionContext  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:34 date=2026-08-10
shared-types::set_active_sketch_plane  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:383 date=2026-08-10
shared-types::SnapSettings  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:294 date=2026-08-10
shared-types::SubElementRef  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:294 date=2026-08-10
shared-types::SubElementType  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:309 date=2026-08-10
shared-types::SystemCapabilities  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:225 date=2026-08-10
shared-types::SystemStatus  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:333 date=2026-08-10
shared-types::ThermalProperties  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:87 date=2026-08-10
shared-types::to_mm  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:478 date=2026-08-10
shared-types::update_activity  # class=dead-symbol file=roshera-backend/shared-types/src/session.rs:425 date=2026-08-10
shared-types::update_cached_mesh  # class=dead-symbol file=roshera-backend/shared-types/src/geometry.rs:259 date=2026-08-10
shared-types::UserDisconnection  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:97 date=2026-08-10
shared-types::UserStatus  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:84 date=2026-08-10
shared-types::ViewportInfo  # class=dead-symbol file=roshera-backend/shared-types/src/vision.rs:225 date=2026-08-10
shared-types::VisualProperties  # class=dead-symbol file=roshera-backend/shared-types/src/materials.rs:64 date=2026-08-10
shared-types::with_request_id  # class=dead-symbol file=roshera-backend/shared-types/src/api.rs:269 date=2026-08-10
shared-types::with_time  # class=dead-symbol file=roshera-backend/shared-types/src/commands.rs:365 date=2026-08-10
shared-types::WorkflowContext  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:108 date=2026-08-10
shared-types::WorkflowHistoryEntry  # class=dead-symbol file=roshera-backend/shared-types/src/system_context.rs:150 date=2026-08-10
timeline-engine::AccessLogEntry  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:299 date=2026-08-10
timeline-engine::AdaptiveBranchingStrategy  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/strategy.rs:288 date=2026-08-10
timeline-engine::add_rule  # class=dead-symbol file=roshera-backend/timeline-engine/src/execution/validation.rs:37 date=2026-08-10
timeline-engine::AIResolutionService  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/conflict.rs:19 date=2026-08-10
timeline-engine::BooleanType  # class=dead-symbol file=roshera-backend/timeline-engine/src/types.rs:861 date=2026-08-10
timeline-engine::BranchConfig  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/strategy.rs:57 date=2026-08-10
timeline-engine::BranchingContext  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/strategy.rs:33 date=2026-08-10
timeline-engine::BranchStatistics  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:450 date=2026-08-10
timeline-engine::brep_to_serialized  # class=dead-symbol file=roshera-backend/timeline-engine/src/brep_serialization.rs:133 date=2026-08-10
timeline-engine::CachedDependencies  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:11 date=2026-08-10
timeline-engine::calculate_transitive_deps  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:168 date=2026-08-10
timeline-engine::can_reorder  # class=dead-symbol file=roshera-backend/timeline-engine/src/dependency_graph.rs:226 date=2026-08-10
timeline-engine::CheckpointConfig  # class=dead-symbol file=roshera-backend/timeline-engine/src/types.rs:1226 date=2026-08-10
timeline-engine::complete_branch  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:295 date=2026-08-10
timeline-engine::Constrainedness  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:72 date=2026-08-10
timeline-engine::create_matrix_from_quaternion  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/transform.rs:343 date=2026-08-10
timeline-engine::create_scale_matrix  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/transform.rs:333 date=2026-08-10
timeline-engine::create_test_box  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/common.rs:373 date=2026-08-10
timeline-engine::create_translation_matrix  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/transform.rs:287 date=2026-08-10
timeline-engine::DeletedEntity  # class=dead-symbol file=roshera-backend/timeline-engine/src/types.rs:707 date=2026-08-10
timeline-engine::dependency_cache  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:138 date=2026-08-10
timeline-engine::dependency_key  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:245 date=2026-08-10
timeline-engine::DependencyEdge  # class=dead-symbol file=roshera-backend/timeline-engine/src/dependency_graph.rs:30 date=2026-08-10
timeline-engine::entity_id_to_geometry_id  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/common.rs:60 date=2026-08-10
timeline-engine::EntityMapping  # class=dead-symbol file=roshera-backend/timeline-engine/src/entity_mapping.rs:16 date=2026-08-10
timeline-engine::EnvelopeError  # class=dead-symbol file=roshera-backend/timeline-engine/src/kernel_ref.rs:140 date=2026-08-10
timeline-engine::evict_if_needed  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:192 date=2026-08-10
timeline-engine::execution_error  # class=dead-symbol file=roshera-backend/timeline-engine/src/error.rs:134 date=2026-08-10
timeline-engine::ExecutionStats  # class=dead-symbol file=roshera-backend/timeline-engine/src/execution/mod.rs:227 date=2026-08-10
timeline-engine::find_best_branch  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:321 date=2026-08-10
timeline-engine::find_critical_paths  # class=dead-symbol file=roshera-backend/timeline-engine/src/dependency_graph.rs:329 date=2026-08-10
timeline-engine::for_assembly  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:241 date=2026-08-10
timeline-engine::for_sketch  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:226 date=2026-08-10
timeline-engine::from_geometry_id  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:318 date=2026-08-10
timeline-engine::from_shared_type  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:305 date=2026-08-10
timeline-engine::from_solid_certificate  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:162 date=2026-08-10
timeline-engine::geometry_id_to_entity_id  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/common.rs:65 date=2026-08-10
timeline-engine::get_active_branches  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:229 date=2026-08-10
timeline-engine::get_ai_branches  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:245 date=2026-08-10
timeline-engine::get_all_entities  # class=dead-symbol file=roshera-backend/timeline-engine/src/execution/context.rs:271 date=2026-08-10
timeline-engine::get_branch_events_map  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:1737 date=2026-08-10
timeline-engine::get_branch_head  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:828 date=2026-08-10
timeline-engine::get_child_branches  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:237 date=2026-08-10
timeline-engine::get_current_state  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:467 date=2026-08-10
timeline-engine::get_dependencies  # class=dead-symbol file=roshera-backend/timeline-engine/src/dependency_graph.rs:128 date=2026-08-10
timeline-engine::get_entity_deps  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:52 date=2026-08-10
timeline-engine::get_event_helper  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:169 date=2026-08-10
timeline-engine::get_event_internal  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:1745 date=2026-08-10
timeline-engine::get_parallel_groups  # class=dead-symbol file=roshera-backend/timeline-engine/src/dependency_graph.rs:245 date=2026-08-10
timeline-engine::IntoTimelineError  # class=dead-symbol file=roshera-backend/timeline-engine/src/error.rs:111 date=2026-08-10
timeline-engine::invalidate_entities  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/operation_cache.rs:139 date=2026-08-10
timeline-engine::is_branch_active  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:2010 date=2026-08-10
timeline-engine::is_memory_limit_exceeded  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:187 date=2026-08-10
timeline-engine::KernelRefError  # class=dead-symbol file=roshera-backend/timeline-engine/src/kernel_ref.rs:104 date=2026-08-10
timeline-engine::list_branches_alt  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:419 date=2026-08-10
timeline-engine::mark_event_active  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:293 date=2026-08-10
timeline-engine::mark_event_undone  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:285 date=2026-08-10
timeline-engine::matrix4_to_array  # class=dead-symbol file=roshera-backend/timeline-engine/src/operations/common.rs:139 date=2026-08-10
timeline-engine::ModificationType  # class=dead-symbol file=roshera-backend/timeline-engine/src/types.rs:720 date=2026-08-10
timeline-engine::new_with_sink  # class=dead-symbol file=roshera-backend/timeline-engine/src/recorder_bridge.rs:322 date=2026-08-10
timeline-engine::next_sequence_number  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:190 date=2026-08-10
timeline-engine::operation_cache  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:128 date=2026-08-10
timeline-engine::operation_key  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:226 date=2026-08-10
timeline-engine::override_floor  # class=dead-symbol file=roshera-backend/timeline-engine/src/incremental.rs:253 date=2026-08-10
timeline-engine::parse_ref  # class=dead-symbol file=roshera-backend/timeline-engine/src/kernel_ref.rs:314 date=2026-08-10
timeline-engine::ParsedRef  # class=dead-symbol file=roshera-backend/timeline-engine/src/kernel_ref.rs:92 date=2026-08-10
timeline-engine::provenance_path  # class=dead-symbol file=roshera-backend/timeline-engine/src/lineage.rs:409 date=2026-08-10
timeline-engine::prune_old_branches  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:399 date=2026-08-10
timeline-engine::put_entity_deps  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:65 date=2026-08-10
timeline-engine::RECORDER_CHANNEL_CAPACITY  # class=dead-symbol file=roshera-backend/timeline-engine/src/recorder_bridge.rs:71 date=2026-08-10
timeline-engine::register_edge  # class=dead-symbol file=roshera-backend/timeline-engine/src/entity_mapping.rs:55 date=2026-08-10
timeline-engine::register_face  # class=dead-symbol file=roshera-backend/timeline-engine/src/entity_mapping.rs:46 date=2026-08-10
timeline-engine::register_vertex  # class=dead-symbol file=roshera-backend/timeline-engine/src/entity_mapping.rs:64 date=2026-08-10
timeline-engine::registered_types  # class=dead-symbol file=roshera-backend/timeline-engine/src/execution/registry.rs:54 date=2026-08-10
timeline-engine::remove_dependency  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:139 date=2026-08-10
timeline-engine::ResolutionContext  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/conflict.rs:467 date=2026-08-10
timeline-engine::ResolutionReport  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/conflict.rs:496 date=2026-08-10
timeline-engine::serialized_to_brep  # class=dead-symbol file=roshera-backend/timeline-engine/src/brep_serialization.rs:266 date=2026-08-10
timeline-engine::set_event_entities  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/dependency_cache.rs:163 date=2026-08-10
timeline-engine::skipped_solid  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:214 date=2026-08-10
timeline-engine::SnapshotMetadata  # class=dead-symbol file=roshera-backend/timeline-engine/src/storage/snapshot.rs:53 date=2026-08-10
timeline-engine::SolidCertChecks  # class=dead-symbol file=roshera-backend/timeline-engine/src/event_certificate.rs:51 date=2026-08-10
timeline-engine::state_at  # class=dead-symbol file=roshera-backend/timeline-engine/src/lineage.rs:392 date=2026-08-10
timeline-engine::StorageEngine  # class=dead-symbol file=roshera-backend/timeline-engine/src/storage/mod.rs:20 date=2026-08-10
timeline-engine::StorageStats  # class=dead-symbol file=roshera-backend/timeline-engine/src/storage/mod.rs:250 date=2026-08-10
timeline-engine::tessellation_key  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:240 date=2026-08-10
timeline-engine::TimelineStats  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline.rs:2463 date=2026-08-10
timeline-engine::total_memory_usage  # class=dead-symbol file=roshera-backend/timeline-engine/src/cache/mod.rs:180 date=2026-08-10
timeline-engine::UndoRedoState  # class=dead-symbol file=roshera-backend/timeline-engine/src/timeline_impl.rs:18 date=2026-08-10
timeline-engine::update_metrics  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:253 date=2026-08-10
timeline-engine::update_quality_score  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/mod.rs:276 date=2026-08-10
timeline-engine::update_success_rate  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/strategy.rs:310 date=2026-08-10
timeline-engine::validation_error  # class=dead-symbol file=roshera-backend/timeline-engine/src/error.rs:129 date=2026-08-10
timeline-engine::value_for  # class=dead-symbol file=roshera-backend/timeline-engine/src/mould.rs:281 date=2026-08-10
timeline-engine::with_ai_service  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/conflict.rs:40 date=2026-08-10
timeline-engine::with_capacity_and_sink  # class=dead-symbol file=roshera-backend/timeline-engine/src/recorder_bridge.rs:353 date=2026-08-10
timeline-engine::with_params  # class=dead-symbol file=roshera-backend/timeline-engine/src/branch/strategy.rs:119 date=2026-08-10
verdict-harness::AuthorityResolver  # class=dead-symbol file=roshera-backend/verdict-harness/src/resolver.rs:218 date=2026-08-10
verdict-harness::DeterministicResolver  # class=dead-symbol file=roshera-backend/verdict-harness/src/resolver.rs:93 date=2026-08-10
verdict-harness::pending_len  # class=dead-symbol file=roshera-backend/verdict-harness/src/room.rs:222 date=2026-08-10
verdict-harness::QuorumResolver  # class=dead-symbol file=roshera-backend/verdict-harness/src/resolver.rs:107 date=2026-08-10
verdict-harness::VerdictRecord  # class=dead-symbol file=roshera-backend/verdict-harness/src/room.rs:44 date=2026-08-10
verdict-harness::VetoResolver  # class=dead-symbol file=roshera-backend/verdict-harness/src/resolver.rs:174 date=2026-08-10
