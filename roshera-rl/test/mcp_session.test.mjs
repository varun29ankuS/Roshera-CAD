/**
 * THE WIRE-CONTRACT PROOF.
 *
 * `@modelcontextprotocol/sdk` is not installed here, so no test in this
 * package has ever seen a real tool result — which is exactly how three
 * criticals hid: `mcp_session` and `reward` were written against an IMAGINED
 * contract, every test injected a fake session, and the suite certified the
 * mock.
 *
 * So every result object below is COPIED FROM THE SOURCE that produces it,
 * cited line by line, and pushed through the REAL `readToolResult`. A test
 * built from an invented shape is worse than no test.
 *
 *   roshera-mcp/src/core.ts:380-385         ok(data)
 *   roshera-mcp/src/core.ts:471-523         fail(e)  (+ structuredContent)
 *   roshera-mcp/src/core.ts:187-193         the ApiError message format
 *   roshera-mcp/src/gates.ts:121-131        gateRefusal(payload)
 *   roshera-mcp/src/gates.ts:142-163        typedRefusalOf  (mirrored here)
 *   roshera-mcp/src/tools/timeline.ts:26-35 refusalOrFail(e)
 *   roshera-mcp/src/tools/perception.ts:199-248  verify_part's own body
 *   geometry-engine/src/readable/claim.rs:64-78  ClaimVerdict
 *   api-server/src/auth_middleware.rs:20-33, 870-874  the 429 body
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  readToolResult, credentialEnv, defaultMcpEntry, verifyClaims, fetchRecipe,
} from "../lib/mcp_session.mjs";
import { rewardFromResult, mergeFinal } from "../lib/reward.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const checks = [];
const check = (name, fn) => checks.push([name, fn]);

/** core.ts:380-385 — `ok(data)`: one text block, no isError, no structured. */
const ok = (data) => ({ content: [{ type: "text", text: JSON.stringify(data, null, 2) }] });

/** core.ts:485-497 — `fail(e)`: prose text + isError, structuredContent only
 *  when the body carried an `error_code` (core.ts:413-430, 498-521). */
const fail = (msg, hint, structured) => ({
  content: [{ type: "text", text: hint ? `ERROR: ${msg}\nHINT: ${hint}` : `ERROR: ${msg}` }],
  isError: true,
  ...(structured ? { structuredContent: structured } : {}),
});

/** gates.ts:121-131 — a gate refusal: JSON `{refused:true,…}` AND isError. */
const gateRefusal = (payload) => ({
  content: [{ type: "text", text: JSON.stringify({ refused: true, ...payload }, null, 2) }],
  isError: true,
});

// ── the four real result kinds ──────────────────────────────────────────

check("ok() success: data is the parsed body, nothing is an error", () => {
  // core.ts:914-931 okp() in `cert` mode + create.ts:253-260's own fields.
  const res = ok({
    object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10",
    part_id: 1,
    placement: { center_world: [0, 0, 0], dimensions_world: [50, 50, 60] },
    perception: { sound: true, brep_valid: true, watertight: true, manifold: null, volume: 117809.7, face_count: 3 },
  });
  const env = readToolResult(res);
  assert.equal(env.is_error, false);
  assert.equal(env.refusal, null);
  assert.equal(env.rate_limited, false);
  assert.equal(env.parse_error, null);
  assert.equal(env.data.object_uuid, "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10");
  const r = rewardFromResult(env);
  assert.equal(r.components.sound, true, "the cert-mode perception object is where `sound` lives");
  assert.equal(r.components.refused, null);
});

check("fail()'s PROSE is never a silent success", () => {
  // core.ts:189-193 builds the message; errorHint (core.ts:563-564) supplies
  // this hint for a "not found" message.
  const env = readToolResult(fail(
    "POST /api/geometry/boolean → 404: no live solid has part_id 7",
    "that id no longer names a live solid: every mutating op (boolean, shell, blend) MINTS a NEW id and CONSUMES its inputs…",
  ));
  assert.equal(env.is_error, true, "isError must survive the session — dropping it was CRITICAL 1");
  assert.equal(env.data, null);
  assert.ok(env.parse_error.includes("not JSON"), "and the absence of a body is STATED");
  const r = rewardFromResult(env);
  assert.ok(!("sound" in r.components), "a failed call measured no soundness");
  assert.equal(typeof r.components.call_failed, "string", "the failure is a named component, not silence");
  assert.equal(mergeFinal([r]).components.call_failures, 1);
});

check("a gate refusal is counted as a refusal, with its gate", () => {
  // gates.ts's verification-scope gate payload (gates.ts:87-106).
  const env = readToolResult(gateRefusal({
    gate: "verification_scope",
    reason: "solid-mutating ops ran under this checkpoint with no verify_part / verify_claim since the last of them",
    built: ["create_cylinder"],
    escape: "pass skip_verification: true on the closing checkpoint",
  }));
  assert.deepEqual(env.refusal, { gate: "verification_scope" });
  const r = rewardFromResult(env);
  assert.equal(r.components.refused, "verification_scope");
  assert.equal(mergeFinal([r]).components.refusals, 1);
  assert.ok(r.gaps.some((g) => g.name === "sound"), "a refused call built nothing to certify");
});

check("refusalOrFail's OBJECT refusal is a refusal too", () => {
  // timeline.ts:26-35 — `ok({refused: JSON.parse(e.body)})`. `refused` is an
  // OBJECT and the result carries NO isError. The old `refused === true` test
  // missed this entirely.
  const env = readToolResult(ok({
    refused: {
      success: false,
      error_code: "document_not_found",
      error: "no document 'doc-7' is registered",
      retryable: false,
    },
  }));
  assert.notEqual(env.refusal, null, "an object-valued `refused` is a refusal (gates.ts:153)");
  assert.equal(env.refusal.gate, null, "this refusal names no gate — and that is not a reason to drop it");
  const r = rewardFromResult(env);
  assert.equal(r.components.refused, "unnamed_gate");
  assert.equal(mergeFinal([r]).components.refusals, 1);
});

check("an isError result carrying the REFUSED marker is a refusal", () => {
  // gates.ts:161 — the third detector branch.
  const env = readToolResult(fail(
    "POST /api/geometry/drill → 422: REFUSED: hole spacing below the wall-thickness minimum",
  ));
  assert.notEqual(env.refusal, null);
  assert.equal(rewardFromResult(env).components.refused, "unnamed_gate");
});

check("a ClaimVerdict with refused:false is NOT read as a refusal", () => {
  // claim.rs:64-78 — `refused` is a top-level BOOLEAN on every verdict, so a
  // detector that tested truthiness loosely would score every successful
  // claim check as a refusal.
  const env = readToolResult(ok({
    verified: true, refused: false, computed: 117809.72451, expected: 117809.72451,
    abs_error: 1.4e-9, tolerance_used: 117.80972451, resolved: [["v", 117809.72451]], unresolved: [],
  }));
  assert.equal(env.refusal, null);
});

// ── rate limiting ───────────────────────────────────────────────────────

check("a 429 is visible in what actually crosses the wire", () => {
  // auth_middleware.rs:870-874 mints AuthError{error,code,status}; its
  // IntoResponse (auth_middleware.rs:26-34) serialises the whole struct as the
  // body; core.ts:189-193 folds it into the ApiError message. The body has
  // `code`, NOT `error_code`, so parseCatalogError (core.ts:413-430) returns
  // null and NO structuredContent is attached — the text is the only signal.
  const env = readToolResult(fail(
    'POST /api/geometry/cylinder → 429: {"error":"Rate limit exceeded","code":"RATE_LIMIT_EXCEEDED","status":429}',
  ));
  assert.equal(env.rate_limited, true);
  assert.equal(env.structured, null, "the 429 body is not catalog-shaped, so no typed field arrives");
  assert.equal(rewardFromResult(env).components.rate_limited, true);
});

check("an ordinary error is not mistaken for a rate limit", () => {
  const env = readToolResult(fail("POST /api/geometry/cylinder → 422: radius must be positive"));
  assert.equal(env.rate_limited, false);
});

check("a catalog error keeps its typed fields", () => {
  // core.ts:498-521 — structuredContent mirrors the refusal-card wire shape.
  const env = readToolResult(fail(
    "POST /api/geometry/fillet → 422: blend failed",
    "requested radius 8mm exceeds the local curvature limit at edge 3 — retry with radius ≤ 4.2mm (r_max)…",
    { reason: "POST /api/geometry/fillet → 422: blend failed", error_code: "blend_failed", retryable: false },
  ));
  assert.equal(env.structured.error_code, "blend_failed");
  assert.equal(env.structured.retryable, false);
});

// ── the compact-verdict trap ────────────────────────────────────────────

check("a PROSE perception is a stated gap, never parsed into a verdict", () => {
  // core.ts:923-926 — the DEFAULT ambient mode renders perception as ONE
  // LINE from compactVerdict. `perception.sound` does not exist in that mode,
  // which is why mcp_session pins ROSHERA_AMBIENT_PERCEPTION=cert.
  const env = readToolResult(ok({
    object_uuid: "3f2b…", part_id: 1,
    perception: "SOUND ✓ brep·watertight (unverified: manifold,no-self-intersect — verify_part to certify) | vol=117809.7mm³ | 3 faces",
  }));
  const r = rewardFromResult(env);
  assert.ok(!("sound" in r.components), "prose is not a boolean and must not be guessed into one");
  const gap = r.gaps.find((g) => g.name === "sound");
  assert.ok(gap.reason.includes("PROSE"), "and the reason names what was actually received");
});

check("verify_part's TOP-LEVEL sound is read", () => {
  // perception.ts:199-248 — verify_part hand-builds its body: `sound` sits at
  // the top level and there is no `perception` wrapper, plus an image block
  // that must not confuse the text extraction.
  const env = readToolResult({
    content: [
      { type: "text", text: JSON.stringify({
        part_id: 1, sound: true, brep_valid: true, brep_watertight: true,
        manifold: true, self_intersection_free: true, tessellation_clean: true,
        mesh_quality_clean: true, construction_consistent: true,
        eyes_consistent: "consistent", verdict: "OK — valid closed solid",
        display_mesh: { open_edges: 0, nonmanifold_edges: 0, note: "display tessellation quality only — does NOT determine validity" },
        dims: null, reconcile: { status: "pending" },
      }, null, 2) },
      { type: "image", data: "iVBORw0KGgo=", mimeType: "image/png" },
    ],
  });
  assert.equal(rewardFromResult(env).components.sound, true);
});

// ── the mirror stays a mirror ───────────────────────────────────────────

check("the refusal detector still matches gates.ts", () => {
  // The ontology-drift-gate shape this repo already uses: two independently
  // maintained surfaces asserted equal, so a change to the canonical detector
  // breaks loudly here instead of silently narrowing this copy.
  const gates = readFileSync(join(HERE, "..", "..", "roshera-mcp", "src", "gates.ts"), "utf8");
  const CANONICAL = 'r === true || (r !== null && typeof r === "object")';
  assert.ok(
    gates.includes(CANONICAL),
    `gates.ts no longer contains the canonical refusal test ${CANONICAL} — ` +
    `lib/mcp_session.mjs mirrors it and must be updated in the same change`,
  );
  const mirror = readFileSync(join(HERE, "..", "lib", "mcp_session.mjs"), "utf8");
  assert.ok(mirror.includes(CANONICAL), "the mirror must carry the canonical test verbatim");
});

// ── spawn-time seams ────────────────────────────────────────────────────

check("the mcp entry default resolves against this module, not the CWD", () => {
  const expected = resolve(HERE, "..", "..", "roshera-mcp", "dist", "index.js");
  assert.equal(defaultMcpEntry(), expected);
  // The reported failure: `npm run batch --prefix roshera-rl` from the repo
  // root leaves CWD at the repo root, where the old CWD-relative default
  // pointed one directory ABOVE the repo — every spawn failed, every episode
  // SETUP_FAILED. Resolving from any CWD must not move the answer.
  const repoRoot = resolve(HERE, "..", "..");
  assert.notEqual(
    defaultMcpEntry(), resolve(repoRoot, "../roshera-mcp/dist/index.js"),
    "run from the repo root, a CWD-relative default lands outside the repo entirely",
  );
  assert.equal(defaultMcpEntry(), resolve(repoRoot, "roshera-mcp/dist/index.js"));
});

check("only an ApiKey credential crosses into the child", () => {
  // core.ts:30-33 — the child reads ROSHERA_API_KEY and sends `ApiKey <key>`.
  assert.deepEqual(credentialEnv({ Authorization: "ApiKey abc123" }), { ROSHERA_API_KEY: "abc123" });
  assert.deepEqual(credentialEnv({}), {});
  assert.deepEqual(credentialEnv(undefined), {});
  assert.throws(
    () => credentialEnv({ Authorization: "Bearer eyJhbGciOi" }),
    /unsupported scheme/i,
    "a Bearer token forwarded whole would 401 every child call while the harness's own fetches worked",
  );
});

// ── terminal scoring ────────────────────────────────────────────────────
//
// `verifyClaims` / `fetchRecipe` are parameterised by `call`, so the logic
// that decides verified / false / absent is driven here against REAL verdicts
// instead of being replaced by a fake session.

const UUID = "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10";
const volumeClaim = {
  name: "volume", expr: "v",
  bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
  expected: 117809.724509617, tolerance: 117.8,
};
/**
 * Records what was sent, and replies with the envelope the REAL session would
 * have produced for `raw` — the raw result goes through `readToolResult`, so
 * these tests cannot accidentally hand the scoring code a friendlier shape
 * than the wire does.
 */
const recorder = (raw) => {
  const sent = [];
  const reply = readToolResult(raw);
  const call = async (tool, args) => { sent.push({ tool, args }); return reply; };
  return { call, sent };
};

check("a claim is sent in verify_claim's real language, and its verdict read", async () => {
  // claim.rs:148-158 — the measured, in-tolerance verdict.
  const { call, sent } = recorder(ok({
    verified: true, refused: false, computed: 117809.72451, expected: 117809.724509617,
    abs_error: 4e-7, tolerance_used: 117.8, resolved: [["v", 117809.72451]], unresolved: [],
  }));
  const out = await verifyClaims(call, [volumeClaim], [UUID]);
  assert.deepEqual(sent[0], {
    tool: "verify_claim",
    args: {
      expr: "v",
      bindings: [{ var: "v", measure: { kind: "volume", part: UUID } }],
      expected: 117809.724509617,
      tolerance: 117.8,
    },
  }, "the wire args must be {expr, bindings, expected, tolerance} — inspect.ts:63-99");
  assert.equal(out[0].verified, true);
  assert.equal(out[0].computed, 117809.72451, "the field is `computed`; there is no `measured`");
  assert.equal(out[0].abs_error, 4e-7);
});

check("a measured-and-wrong claim is FALSE, not absent", async () => {
  const { call } = recorder(ok({
    verified: false, refused: false, computed: 90000, expected: 117809.724509617,
    abs_error: 27809.7, tolerance_used: 117.8, resolved: [["v", 90000]], unresolved: [],
  }));
  const out = await verifyClaims(call, [volumeClaim], [UUID]);
  assert.equal(out[0].verified, false);
  assert.ok(!("absent" in out[0]), "the kernel measured it — that is a real negative verdict");
});

check("a kernel-REFUSED claim is absent with its unresolved bindings named", async () => {
  // claim.rs:116-127 / agent.rs:222-235 — a binding that did not resolve.
  const { call } = recorder(ok({
    verified: false, refused: true, computed: null, expected: 117809.724509617,
    abs_error: null, tolerance_used: 117.8, resolved: [], unresolved: ["v"],
  }));
  const out = await verifyClaims(call, [volumeClaim], [UUID]);
  assert.equal(out[0].verified, null,
    "verified:false would misreport a check that never happened as a check that failed");
  assert.ok(out[0].absent.includes("v"), "and the unresolved binding is named");
});

check("a claim naming a solid the episode never built is absent, and never sent", async () => {
  const { call, sent } = recorder(ok({}));
  const out = await verifyClaims(call, [volumeClaim], []);   // nothing observed
  assert.equal(sent.length, 0, "there is nothing to ask about");
  assert.equal(out[0].verified, null);
  assert.ok(out[0].absent.includes("object_uuid"));
});

check("a failed verify_claim call is absent with the error quoted", async () => {
  const { call } = recorder(fail("POST /api/agent/verify-claim → 500: internal error"));
  const out = await verifyClaims(call, [volumeClaim], [UUID]);
  assert.equal(out[0].verified, null);
  assert.ok(out[0].absent.includes("500"));
});

check("the recipe carries what recipe_get really returns, addressed by document", async () => {
  // timeline.ts:267-283 over handlers/timeline.rs:3986-4008.
  const { call, sent } = recorder(ok({
    source: {
      kind: "durable_document", reference: "doc-7", branch: "main", document: "doc-7",
      note: "projected from the document's PERSISTED event log; the document was NOT opened and the live model was not touched.",
    },
    step_count: 2, sequence_range: [0, 1], sequence_contiguous: true, undecodable_events: 0,
    checkpoints: [{ name: "cylinder", description: "r25 h60", covers: [0, 1], covers_is_empty: false }],
    certificate_summary: { steps_total: 2, steps_with_recorded_certificate: 2, sound: 2, unsound: 0, indeterminate: 0, last_certified_sequence: 1, note: "…" },
    reparameterize: "…",
    steps: [
      { sequence: 0, op_kind: "create_cylinder", params: { radius: 25, height: 60 }, inputs: [], outputs: [1], intent: null, checkpoint: "cylinder", reissue: { route: "POST /api/geometry/cylinder", body: { radius: 25, height: 60 } } },
      { sequence: 1, op_kind: "verify", params: {}, inputs: [1], outputs: [], intent: null, checkpoint: "cylinder", reissue: null, reissue_absent_reason: "a verification is not a build step" },
    ],
  }));
  const ref = await fetchRecipe(call, "doc-7");
  assert.deepEqual(sent[0], { tool: "recipe_get", args: { reference: "doc-7" } },
    "the default 'main' resolves against the process-wide ACTIVE document " +
    "(handlers/timeline.rs:4400-4433) — i.e. someone else's episode");
  assert.equal(ref.step_count, 2);
  assert.deepEqual(ref.sequence_range, [0, 1]);
  assert.equal(ref.steps.length, 2, "the steps are embedded: the document is deleted at reap");
  assert.ok(!("ref" in ref), "there is no `ref` field on a recipe and there never was");
  assert.ok(!("absent" in ref));
});

check("a recipe_get refusal is an absence with a reason, not a null", async () => {
  // timeline.ts:284-288 — an unknown reference is a TYPED 404 surfaced as
  // ok({refused: <body>}).
  const { call } = recorder(ok({
    refused: { success: false, error_code: "document_not_found", error: "no document 'doc-7'", retryable: false },
  }));
  const ref = await fetchRecipe(call, "doc-7");
  assert.ok(ref.absent.includes("refused"));
  assert.equal(ref.step_count, undefined);
});

check("an EMPTY durable log says so rather than reading as an empty build", async () => {
  const { call } = recorder(ok({
    source: { kind: "durable_document", reference: "doc-7", branch: "main", document: "doc-7" },
    step_count: 0, sequence_range: null, sequence_contiguous: true, undecodable_events: 0,
    checkpoints: [], certificate_summary: null, reparameterize: "…", steps: [],
  }));
  const ref = await fetchRecipe(call, "doc-7");
  assert.equal(ref.step_count, 0);
  assert.ok(ref.absent.includes("not evidence"),
    "zero steps is not proof the episode built nothing");
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\nmcp_session: ${checks.length} checks passed\n`);
