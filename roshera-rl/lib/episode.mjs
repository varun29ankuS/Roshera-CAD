/**
 * One episode: create a document AND a part, spawn an MCP process PINNED to
 * both, drive the policy to done, score, reap.
 *
 * The episode is a real MCP session, not a simulation of one — same
 * ToolTable, same gates, same typed refusals. SESSION isolation comes from the
 * process boundary: every piece of episode state in gates.ts is a
 * module-level binding, so two episodes cannot contaminate each other
 * because they do not share an address space. Process death is the reset.
 *
 * MODEL isolation does not come free with it, and used to be missing outright:
 * the backend keeps one global `AppState.model` and only routes away from it
 * for a caller that sends `X-Roshera-Part-Id` (api-server/src/part_mgr.rs:264,
 * 286-312). Measured on 8 concurrent live episodes (2026-08-13): part ids
 * 73…93 inside supposedly fresh documents, every session reading every other
 * session's solids. So the episode creates a PART (`POST /api/parts`,
 * part_mgr.rs:340-358) beside its document, pins the child to it, and reaps
 * it — and records what its own model actually held at the end.
 *
 * `spawn` is injectable so the lifecycle can be tested without a backend. An
 * injected session must return `mcp_session.readToolResult` ENVELOPES from
 * `call`, exactly as the real one does — there is one result shape in this
 * package, and a fake that speaks a friendlier one would certify itself.
 */
import { openTrajectory } from "./trajectory.mjs";
import { rewardFromResult, mergeFinal } from "./reward.mjs";
import { assertActionAllowed, deepFreeze } from "./policy.mjs";
import { spawnMcpSession } from "./mcp_session.mjs";

const FNV_OFFSET_BASIS = 14695981039346656037n;
const FNV_PRIME = 1099511628211n;
const MASK64 = 0xffffffffffffffffn;

/**
 * FNV-1a, 64-bit, over the UTF-8 bytes of the canonical JSON form: XOR the
 * byte INTO the hash, then multiply — from the offset basis, not from zero.
 * The label and the algorithm have to agree; a digest labelled `fnv1a64:`
 * that is not FNV-1a is a small lie in the one field a consumer would use to
 * compare two runs.
 */
const digest = (v) => {
  const bytes = new TextEncoder().encode(JSON.stringify(v ?? null));
  let h = FNV_OFFSET_BASIS;
  for (const b of bytes) h = ((h ^ BigInt(b)) * FNV_PRIME) & MASK64;
  return `fnv1a64:${h.toString(16)}`;
};

/**
 * Item 7 (audit S3.1) — gate 6 (`roshera-mcp/src/gates.ts`'s
 * verification-scope gate) fires only when a NEW `timeline_checkpoint`
 * closes the open one; an episode that opens one checkpoint, mutates, and
 * simply STOPS is never asked to verify anything, which is the normal shape
 * of an RL episode's own final steps. The design ruling: close this HERE,
 * not in MCP — the episode is the only place a session has a defined end,
 * so it is the only place the check can be complete.
 *
 * `MUTATES_SOLIDS` and `VERIFIES` are copied from gates.ts (`:269-290`,
 * `:347`), not imported: gates.ts lives in a sibling package with its own
 * build, and the design ruling asks for a PURE function over data this
 * package already holds, not a new cross-package dependency. A drift
 * between the two lists is a real, accepted limit — the same one the repo
 * already lives with for gate 2a's cross-package parity check
 * (`handlers/timeline.rs:5478-5484`).
 */
const MUTATES_SOLIDS = new Set([
  "create_box", "create_cylinder", "create_sphere", "create_cone",
  "boolean", "boolean_many", "revolve", "nurbs_loft", "shell",
  "fillet_edges", "chamfer_edges", "drill_pattern", "transform",
  "sketch_extrude", "psketch_extrude", "psketch_revolve", "import_step",
  "timeline_mould", "delete_part", "clear_parts",
]);
const VERIFIES = new Set(["verify_part", "verify_claim"]);

/**
 * M2 (2026-08-15 final review), corrected by H1 (2026-08-15 whole-branch
 * review) — `cad_program` and `invoke` are COMPOSITE dispatch: each runs its
 * own inner calls through gates.ts's wrapper individually
 * (`registry.ts:119-132`), so ten mutations inside one `cad_program` batch
 * each arm and clear gate 6 exactly as ten direct calls would. None of that
 * shows up as separate entries in THIS package's step log — `stepLog`
 * records only the OUTER tool name (`action.tool` below) — so a successful
 * composite dispatch is the concrete, measured answer to "can
 * `MUTATES_SOLIDS` and `VERIFIES` be byte-identical to gates.ts's copies
 * while the two packages' BEHAVIOUR still differs": yes, exactly here.
 *
 * **How this set was derived, so the next person can re-derive it rather
 * than guess:** grep `roshera-mcp/src` for `entry.handler(` — the call that
 * actually dispatches an inner tool through the registry. There are exactly
 * two hits today: `cad_program.ts:454` and `metatools.ts:571` (`invoke`'s
 * handler). Any tool whose OWN handler is not one of those two call sites is
 * not composite, however tempting its name — `workbench` was in this set
 * until H1: its handler is `async ({ mode }) => ok(wb.enter(mode))`
 * (`workbench.ts:293`), which dispatches nothing, changes only which tools
 * are *exposed*, and was never one of the two `entry.handler(` sites. One
 * successful `workbench` call was short-circuiting `unverifiedMutatingWork`
 * for the WHOLE episode from that point on while stating a reason below
 * ("inner mutations and inner verify_part/verify_claim calls … not logged
 * as separate entries here") that was false for `workbench` specifically —
 * there are no inner calls to not-log. Latent only because the shipped task
 * allowlist (`task.mjs:248`) never named it; `task.mjs:10-11` anticipates a
 * future task that does.
 *
 * The set-equality pin in episode.test.mjs proves the two membership lists
 * agree; it cannot and does not prove anything about what a composite call
 * did once dispatched.
 */
const COMPOSITE_DISPATCH = new Set(["cad_program", "invoke"]);

/**
 * F1 (2026-08-15 whole-branch review, finding H3) — the `gates.ts` copy of
 * this exact function ("Whether a successful `verify_claim` dispatch
 * actually measured something", gates.ts, next to `VERIFIES`) explains the
 * two reasons `ok: true` alone is not "the caller looked": the backend
 * `verify_claim_handler` is structurally always HTTP 200, including its own
 * `refused: true` shape, and a claim built entirely from `constant` bindings
 * references no part at all. Both copies must agree — see the parity test
 * in `episode.test.mjs`.
 *
 * `args` is the request `action.args` this dispatch was made with; `data`
 * is the parsed response body (`result?.data` from `readToolResult`,
 * mcp_session.mjs) — both already in hand at the one call site below, no
 * second fetch, mirroring gates.ts's own no-second-fetch discipline.
 */
export function verifyClaimActuallyMeasured(args, data) {
  const bindings = Array.isArray(args?.bindings) ? args.bindings : [];
  const measuresGeometry = bindings.some((b) => b?.measure?.kind !== "constant");
  if (!measuresGeometry) return false;
  return !(data !== null && typeof data === "object" && data.refused === true);
}

/**
 * A pure function from the episode's own step log to "what mutating work, if
 * any, ended the episode unverified" — mirroring gates.ts's
 * `intentUnverified` bookkeeping (`gates.ts`'s `recordDispatchOutcome`), one
 * call at a time, in step order — with ONE undocumented-until-now
 * dependency (L5, 2026-08-16 residuals): gates.ts only ever tallies a
 * `MUTATES_SOLIDS` call when `openIntent !== null` (`recordDispatchOutcome`'s
 * own guard); this function has no `openIntent` concept at all and tallies
 * every successful `MUTATES_SOLIDS` step unconditionally. The two agree
 * TODAY only because gates.ts's intent gate refuses every intent-less
 * mutation before it can succeed (`MUTATES_SOLIDS.has(tool) && openIntent
 * === null` → refusal, `preDispatchGate`) — such a call reaches this
 * package's step log as `ok: false` and is skipped at the top of the loop
 * below, before it would ever reach the unconditional tally branch. That
 * refusal is the coupling this function's accuracy rests on; if the intent
 * gate ever stopped refusing an intent-less mutation (or a future caller
 * fed this function a step log that never went through gates.ts at all),
 * this function would silently start tallying calls gates.ts itself never
 * counted. Not pinned mechanically — the two files disagree in SHAPE
 * (`openIntent` is not data this function's step-log-only signature could
 * see), not merely in a copied constant the `MUTATES_SOLIDS`/`VERIFIES`
 * set-equality pins below can catch.
 *
 *   - a call that was refused or errored (`ok: false`) built nothing, so it
 *     is skipped entirely — matching gates.ts's own `result?.isError !== true`
 *     guard around this whole branch;
 *   - `timeline_checkpoint` or `clear_timeline` (successful) CLEAR the
 *     tally: a checkpoint that closed successfully already passed gate 6
 *     itself (verified, or explicitly `skip_verification`'d ON THE RECORD),
 *     so whatever preceded it is settled and must not haunt the episode's
 *     own final verdict;
 *   - `clear_parts` (successful) CLEARS the tally too (M1, 2026-08-15 final
 *     review) — mirroring gates.ts's own `recordDispatchOutcome` exactly:
 *     `clear_parts` deletes EVERY part, so nothing recorded before it
 *     (including `clear_parts` itself) can be named to `verify_part` any
 *     more. Tallying it as ordinary `MUTATES_SOLIDS` work — the naive
 *     reading of gates.ts's own `MUTATES_SOLIDS` membership, which still
 *     includes `clear_parts` — would flag the episode over work whose only
 *     remedy is impossible, the exact corpus consequence M1 closed one layer
 *     up. `delete_part` gets no such exception (removes ONE part; other
 *     unverified work under the intent may still be real) and falls through
 *     to the ordinary `MUTATES_SOLIDS` branch below, same as gates.ts;
 *   - `verify_part` (successful) always clears it — the caller LOOKED;
 *   - `verify_claim` (successful) clears it ONLY when
 *     `verifyClaimActuallyMeasured` says the call measured something (F1) —
 *     a step entry that does not carry `claimMeasured: true` is treated as
 *     NOT having looked, the conservative default;
 *   - any other successful `MUTATES_SOLIDS` call adds to it;
 *   - a successful `COMPOSITE_DISPATCH` call (M2) makes the WHOLE
 *     reconstruction unreliable from that point on and short-circuits to a
 *     STATED ABSENCE rather than a count: its inner dispatches (each one
 *     itself possibly a mutation, a verify, or a checkpoint close) ran
 *     through gates.ts individually but left no trace under their own name
 *     in this step log, so `{count: 0}` here would be exactly the fabricated
 *     "measured, and clean" reading this project refuses — the honest
 *     answer is "cannot be reconstructed", not "nothing was left unverified".
 *
 * `tools` is the DISTINCT verbs (a Set, matching gates.ts's own choice —
 * "boolean, fillet_edges across 40 calls" is legible, forty repetitions of
 * "boolean" is not); `count` tallies every call, distinct or not, same as
 * gates.ts's own `intentUnverified.count`.
 *
 * Defensively coded against malformed input (`null`/`undefined`/non-array,
 * entries missing `tool`) so it degrades to the clean answer rather than
 * throwing — belt-and-braces alongside the try/catch at its one call site
 * below, per the hard constraint that this check must never cost an episode
 * its trajectory.
 */
export function unverifiedMutatingWork(stepLog) {
  const tools = new Set();
  let count = 0;
  const entries = Array.isArray(stepLog) ? stepLog : [];
  for (const s of entries) {
    if (s == null || typeof s !== "object") continue;
    if (s.ok !== true) continue; // refused/errored — built nothing to verify
    const tool = s.tool;
    if (typeof tool !== "string") continue;
    if (COMPOSITE_DISPATCH.has(tool)) {
      return {
        absent:
          `the step log contains a successful '${tool}' dispatch — a ` +
          "composite call whose inner mutations and inner verify_part/" +
          "verify_claim calls run through gates.ts individually but are " +
          "not logged as separate entries here, so unverified-mutation " +
          "tracking cannot be reconstructed from this step log",
      };
    }
    if (tool === "timeline_checkpoint" || tool === "clear_timeline" || tool === "clear_parts") {
      tools.clear();
      count = 0;
    } else if (VERIFIES.has(tool)) {
      if (tool === "verify_part" || s.claimMeasured === true) {
        tools.clear();
        count = 0;
      }
      // else: a verify_claim step logged without claimMeasured:true — it
      // measured nothing (refused, or all-constant bindings) — leave the
      // tally exactly as it was.
    } else if (MUTATES_SOLIDS.has(tool)) {
      tools.add(tool);
      count += 1;
    }
  }
  return { count, tools: [...tools].sort() };
}

/**
 * Item 7's one call site, wrapped so a throw inside the derivation above
 * (or any future edit to it) becomes a STATED ABSENCE rather than a failed
 * episode — the hard constraint the brief names outranking the feature
 * itself: "the episode path must gain no new failure mode."
 */
function deriveUnverifiedMutations(stepLog) {
  try {
    return unverifiedMutatingWork(stepLog);
  } catch (e) {
    return { absent: `the unverified-mutations check itself failed: ${String(e?.message ?? e)}` };
  }
}

/** Why terminal scoring did not run, per outcome. Never a bare `[]`/`null`. */
const NO_TERMINAL_SCORING = {
  BUDGET_EXHAUSTED: "the step or token budget ran out before the policy declared done, so terminal verification never ran",
  INVALID_ACTION: "the episode ended on an action outside its own allowlist, so terminal verification never ran",
  CRASHED: "the episode crashed, so terminal verification never ran",
  RATE_LIMITED: "the shared rate class refused this episode's calls, so terminal verification never ran",
  // WHICH stage, and WHY, arrive as the `detail` `unscored` appends — see
  // SETUP_STAGE below. This base string deliberately no longer says "document
  // creation or spawn": a stated reason that names both possibilities and
  // commits to neither, while discarding the real error, is not a stated
  // reason. It cost a hand-run probe to tell a 401 apart from a spawn failure.
  SETUP_FAILED: "setup failed before the session existed, so there was no session in which to verify anything",
};

/**
 * The setup stages, in the order they run. The label is what the trajectory
 * record and every per-claim absence name, so the two failures the live run hit
 * — a 401 from `POST /api/documents`, and a spawn that died on a missing
 * dependency — are distinguishable from the record alone.
 */
const SETUP_STAGE = Object.freeze({
  DOCUMENT: "document creation",
  PART: "part creation",
  SPAWN: "spawning the MCP session",
});

/**
 * The MCP server version stamped into every trajectory header. One binding, so
 * the batch runner's own SETUP_FAILED record (a policy factory that threw
 * before an episode could start — runner.mjs) cannot stamp a different version
 * than the episodes beside it in the same batch.
 */
export const MCP_VERSION = "0.1.0";

/**
 * Claims/recipe for an episode that never reached terminal scoring.
 *
 * `detail` is the concrete failure — which stage, and the error text it
 * carried — appended to the outcome's standing reason. Without it a reader of
 * the trajectory learns only the category, which is what made two different
 * SETUP_FAILED episodes read identically.
 *
 * Exported because `runner.mjs` writes one such record itself, for the episode
 * that could not begin at all; two hand-written copies of this shape would be
 * free to drift, and the absence reason is the entire content of that record.
 */
export function unscoredFor(task, outcome, detail) {
  const reason = detail
    ? `${NO_TERMINAL_SCORING[outcome]} — ${detail}`
    : NO_TERMINAL_SCORING[outcome];
  return {
    claims: task.claims.map((c) => ({
      name: c.name, verified: null, computed: null, absent: reason,
    })),
    recipeRef: { absent: reason },
    // The isolation reading is unscored for exactly the same reasons and
    // travels with them, so an episode that never ran cannot report a model
    // scope of zero solids — which would read as "its model was empty", a
    // different and false claim.
    modelScope: { absent: reason },
  };
}

/**
 * Delete the episode's document, best-effort, REPORTING what happened. Two
 * call sites share this so they cannot drift apart: the normal end-of-episode
 * reap, and the SETUP_FAILED path where `spawn` failed AFTER document
 * creation already succeeded — that document is just as real and must not be
 * orphaned in PartManager's DashMap.
 *
 * A failed DELETE here must never itself become a thrown error, but it must
 * not vanish either: the outcome is returned, the episode result carries it,
 * and `runner.mjs`'s reaper retries every document this call could not drop
 * (and reports the ones it still could not, rather than asserting they were
 * cleaned up).
 */
/**
 * Delete the episode's PART, best-effort, REPORTING what happened — the same
 * contract as `reapDocument` below and for the same reason. A part that
 * survives its episode holds a whole `BRepModel` in `PartManager`'s DashMap
 * (api-server/src/part_mgr.rs:97, 186-189) for the life of the server.
 *
 * Deleting the DOCUMENT does not take the part with it: `documents::activate`
 * is the only thing that clears the part registry (`state.parts.clear()`,
 * documents.rs:664) and an episode never activates anything. So the part is
 * reaped explicitly, next to the document.
 */
export async function reapPart(baseUrl, authHeader, partId) {
  if (!partId) return { reaped: null, reason: "no part was created" };
  try {
    const res = await fetch(`${baseUrl}/api/parts/${partId}`, {
      method: "DELETE", headers: { ...authHeader },
    });
    if (res.ok) return { reaped: true, reason: null };
    // part_mgr.rs:416-427 — an unknown id is a typed PartNotFound (404),
    // which is as real a report as a network failure and just as unhidden.
    return { reaped: false, reason: `DELETE returned ${res.status}` };
  } catch (e) {
    return { reaped: false, reason: `DELETE failed: ${String(e?.message ?? e)}` };
  }
}

export async function reapDocument(baseUrl, authHeader, documentId) {
  if (!documentId) return { reaped: null, reason: "no document was created" };
  try {
    const res = await fetch(`${baseUrl}/api/documents/${documentId}`, {
      method: "DELETE", headers: { ...authHeader },
    });
    if (res.ok) return { reaped: true, reason: null };
    // A typed refusal is as real as a network failure here: the backend
    // refuses to delete the ACTIVE document, the last document, or the
    // durability session (api-server/src/documents.rs:555-567).
    return { reaped: false, reason: `DELETE returned ${res.status}` };
  } catch (e) {
    return { reaped: false, reason: `DELETE failed: ${String(e?.message ?? e)}` };
  }
}

export async function runEpisode({
  task, policy, seed, baseUrl, authHeader, mcpEntry, trajectoryPath,
  kernelSha, mcpVersion = MCP_VERSION, spawn = spawnMcpSession, provenance,
}) {
  const started = Date.now();
  // An episode never throws: every failure mode is a named outcome, and that
  // has to include the FIRST line of I/O. `openTrajectory` writes the header
  // synchronously, so an unwritable outDir used to throw out of this function
  // before any try/catch existed — the one path that broke the promise.
  let traj;
  try {
    traj = openTrajectory({
      path: trajectoryPath, taskId: task.id, seed, kernelSha, mcpVersion,
      toolAllowlist: [...task.toolAllowlist], split: task.split, provenance,
    });
  } catch (e) {
    return {
      outcome: "SETUP_FAILED", rewardFinal: mergeFinal([]), documentId: null,
      partId: null, trajectoryPath, wallMs: Date.now() - started,
      error: `the trajectory could not be opened: ${String(e?.message ?? e)}`,
      reap: { reaped: null, reason: "no document was created" },
      partReap: { reaped: null, reason: "no part was created" },
      modelScope: { absent: "no session existed, so no model was read" },
    };
  }

  /** This episode's task, bound to the shared `unscoredFor` above. */
  const unscored = (outcome, detail) => unscoredFor(task, outcome, detail);

  // Item 7 — every dispatched call's tool name and whether it succeeded, in
  // step order. Genuinely empty until the drive loop below reaches its
  // first `session.call`, which is what makes `deriveUnverifiedMutations`
  // trivially and honestly `{count: 0, tools: []}` for a SETUP_FAILED
  // episode below: zero steps really did run.
  const stepLog = [];

  // ── setup ────────────────────────────────────────────────────────────
  let documentId = null;
  let partId = null;
  let session = null;
  // Which stage is in flight, so the catch below can NAME it. Setup is three
  // independent pieces of I/O against different failure surfaces (two HTTP
  // routes and a child process), and collapsing them into one reason discards
  // the only fact a diagnosis starts from.
  let setupStage = SETUP_STAGE.DOCUMENT;
  try {
    const res = await fetch(`${baseUrl}/api/documents`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeader },
      body: JSON.stringify({ name: `rl:${task.id}:${seed}` }),
    });
    if (!res.ok) {
      // The shared EvalHarness rate class is what N concurrent episodes
      // saturate FIRST, and document creation is the first request each one
      // makes — so a 429 here is the rate ceiling, not a setup defect, and
      // reporting it as SETUP_FAILED would hide the very measurement this
      // outcome exists to surface (auth_middleware.rs:870-874).
      const rate = res.status === 429;
      const e = new Error(`document creation returned ${res.status}`);
      e.rateLimited = rate;
      throw e;
    }
    documentId = (await res.json())?.id ?? null;
    if (!documentId) throw new Error("document creation returned no id");
    setupStage = SETUP_STAGE.PART;
    // THE EPISODE'S OWN BRepModel. `POST /api/parts` (part_mgr.rs:340-358) is
    // the only thing in the system that allocates one; until this call existed
    // nothing anywhere asked for isolation, so every episode's kernel ops
    // resolved to the one global model.
    const partRes = await fetch(`${baseUrl}/api/parts`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeader },
      body: JSON.stringify({ name: `rl:${task.id}:${seed}` }),
    });
    if (!partRes.ok) {
      // Same reasoning as document creation: a 429 here is the shared rate
      // ceiling N concurrent episodes saturate, not a setup defect
      // (auth_middleware.rs:870-874).
      const e = new Error(`part creation returned ${partRes.status}`);
      e.rateLimited = partRes.status === 429;
      throw e;
    }
    partId = (await partRes.json())?.id ?? null;
    if (!partId) throw new Error("part creation returned no id");
    setupStage = SETUP_STAGE.SPAWN;
    session = await spawn({ documentId, partId, baseUrl, authHeader, mcpEntry });
  } catch (e) {
    // `documentId` and `partId` are each set the moment their own creation
    // succeeds, before the next stage runs — whichever stage failed, what was
    // already allocated is real and orphaned unless reaped here too.
    const reap = await reapDocument(baseUrl, authHeader, documentId);
    const partReap = await reapPart(baseUrl, authHeader, partId);
    const outcome = e?.rateLimited === true ? "RATE_LIMITED" : "SETUP_FAILED";
    // The stage AND the underlying message. The error used to be dropped here
    // and rebuilt as a disjunction, so a 401 from the API and a child process
    // that could not start produced byte-identical records.
    const error = `${setupStage} failed: ${String(e?.message ?? e)}`;
    const { claims, recipeRef, modelScope } = unscored(outcome, error);
    traj.close({
      outcome,
      rewardFinal: mergeFinal([]),
      claims, recipeRef, modelScope, tokens: 0, wallMs: Date.now() - started, error,
      unverifiedMutations: deriveUnverifiedMutations(stepLog),
    });
    return {
      outcome, rewardFinal: mergeFinal([]), documentId, partId,
      trajectoryPath, wallMs: Date.now() - started, error,
      reap, partReap, modelScope,
    };
  }

  // ── drive ────────────────────────────────────────────────────────────
  const rewards = [];
  let outcome = "BUDGET_EXHAUSTED";
  let observation = null;
  let claims = [];
  let recipeRef = null;
  let modelScope = null;
  // Carried out on the returned object the way SETUP_FAILED already does —
  // a crash with no recorded reason is a dead end for whoever reads the
  // batch tally afterward.
  let episodeError = null;

  for (let i = 0; i < task.stepBudget; i += 1) {
    let action;
    try {
      // A FROZEN SNAPSHOT of the reward history, FROZEN AT THE ENTRIES.
      // `policy.act` is arbitrary third-party code, and handing it the live
      // array it could push to — or whose entries it could edit — would let a
      // policy rewrite the record of its own episode, in the same call that
      // freezes the task and the script precisely to stop that.
      //
      // EXACTLY WHAT IS PROTECTED, since a shallow guarantee stated as a deep
      // one is the defect this replaced: `slice()` gives the policy its own
      // array (pushes and splices cannot reach `rewards`), and every entry was
      // `deepFreeze`d at the moment it was recorded below — so its
      // `components` object, its `gaps` array and each gap object inside it
      // are all frozen too, at every level of plain object/array nesting. See
      // `deepFreeze` in policy.mjs for the one thing it does NOT cover (the
      // internal slots of Map/Set/Date), which a reward vector never contains:
      // components are strings, booleans and numbers, gaps are `{name,
      // reason}` strings. Freezing happens ONCE per reward at record time, not
      // per step over the whole history, so the cost stays O(1) per step as
      // the episode grows.
      action = await policy.act({
        task, observation, history: Object.freeze(rewards.slice()),
      });
    } catch (e) {
      // The session.call path below already writes a step carrying its
      // failure reason; a policy crash gets the same treatment so it is not
      // recorded nowhere — not in a step, not in the returned object.
      outcome = "CRASHED";
      episodeError = String(e?.message ?? e);
      traj.step({
        i, action: null, resultDigest: null,
        reward: { components: {}, gaps: [{ name: "sound", reason: `policy.act failed: ${episodeError}` }] },
        refusal: null, ms: 0,
      });
      break;
    }
    if (action?.done === true) { outcome = "COMPLETED"; break; }
    try {
      assertActionAllowed(task, action);
    } catch (e) {
      // The stamped action space is the permitted one. An out-of-allowlist
      // action is recorded as a refusal by the HARNESS, named INVALID_ACTION
      // explicitly rather than left to fall through to the loop's
      // BUDGET_EXHAUSTED default — an episode that ran zero real steps must
      // not read the same as one that genuinely burned its whole budget. The
      // synthetic refusal reward is pushed into `rewards` so `mergeFinal`
      // counts it, the same as any kernel-issued refusal — and it carries the
      // SAME stated gaps a kernel refusal carries, because nothing was built
      // and nothing was measured here either.
      outcome = "INVALID_ACTION";
      const reason =
        "the call was refused by the harness before it reached the kernel, so " +
        "soundness was never measured — there is no verdict to report";
      const reward = {
        components: { refused: "harness_allowlist" },
        gaps: [
          { name: "sound", reason },
          { name: "fidelity_signed", reason },
        ],
      };
      // Frozen at record time — the harness's own refusal is as much a part of
      // the record as a kernel-issued one, and just as un-editable.
      rewards.push(deepFreeze(reward));
      traj.step({
        i, action, resultDigest: null, reward,
        refusal: { gate: "harness_allowlist", reason: String(e.message) }, ms: 0,
      });
      break;
    }
    if (policy.tokensUsed() > task.tokenBudget) { outcome = "BUDGET_EXHAUSTED"; break; }

    const t0 = Date.now();
    let result;
    try {
      result = await session.call(action.tool, action.args);
    } catch (e) {
      // `client.callTool` returns isError RESULTS rather than throwing, so
      // this branch is a genuinely dead transport (the child died, the pipe
      // broke). `.status` is kept as a secondary signal only: `ApiError` and
      // its status live inside the MCP child and never cross stdio, so a 429
      // is detected from the RESULT (below), not from a thrown error.
      outcome = e?.status === 429 ? "RATE_LIMITED" : "CRASHED";
      episodeError = String(e?.message ?? e);
      traj.step({
        i, action, resultDigest: null,
        reward: { components: {}, gaps: [{ name: "sound", reason: `call failed: ${episodeError}` }] },
        refusal: null, ms: Date.now() - t0,
      });
      break;
    }
    const reward = rewardFromResult(result);
    // FROZEN THE MOMENT IT IS RECORDED, at every level — this is what makes
    // the frozen history snapshot above a real guarantee rather than a frozen
    // array of editable objects. `mergeFinal` reads these same objects at the
    // end of the episode, so an entry a policy could still edit is a terminal
    // tally a policy could still rewrite.
    rewards.push(deepFreeze(reward));
    observation = result;
    // Item 7 — the tool name and whether the call actually did anything
    // (`is_error !== true`, the same test gates.ts's own intentUnverified
    // bookkeeping uses), in step order. A harness-level refusal (above,
    // `assertActionAllowed`) never reaches here because it never reached the
    // kernel — nothing was built, so there is nothing to log either.
    //
    // F1 — `claimMeasured` is computed HERE, where `action.args` (the
    // request) and `result.data` (the parsed response) are both in hand, and
    // carried on the step entry rather than recomputed later: `verify_claim`
    // is the only tool this matters for, so the key is simply absent
    // (`undefined`) for every other tool, which `unverifiedMutatingWork`
    // above treats identically to `false` — the conservative default.
    stepLog.push({
      tool: action.tool,
      ok: result?.is_error !== true,
      ...(action.tool === "verify_claim"
        ? { claimMeasured: verifyClaimActuallyMeasured(action.args, result?.data) }
        : {}),
    });
    traj.step({
      i, action, resultDigest: digest(result?.data ?? result?.text ?? null), reward,
      refusal: result?.refusal
        ? {
            gate: result.refusal.gate,
            reason: result.data?.reason ?? result.data?.refused?.error ?? result.text ?? null,
          }
        : null,
      // gate 3's fail-open, made legible (item 1, audit S4): `result.data` is
      // the op's OWN result, which `registry.ts`'s `attachGatePreflightGaps`
      // merges these two keys into ONLY when a live pre-flight fetch could
      // not complete. `resultDigest` above is a HASH — nothing downstream can
      // read a reason out of it — so this is the one line that actually
      // carries the fact from gates.ts into a step a trajectory can be
      // scored on. Straight pass-through, undefined on the (overwhelmingly
      // common) path where the pre-flight completed, exactly mirroring
      // gates.ts's own choice never to stamp a healthy call `"ok"`.
      gatePreflight: result?.data?.gate_preflight,
      gatePreflightGaps: result?.data?.gate_preflight_gaps,
      ms: Date.now() - t0,
    });
    if (result?.rate_limited === true) {
      // The shared 6000/min EvalHarness class refused us. It is its own
      // outcome so the ceiling shows up in the batch tally instead of being
      // averaged into a lower score — and it is detected from what actually
      // crossed the wire (mcp_session.rateLimitedByWire), not from a thrown
      // status that never leaves the child process.
      outcome = "RATE_LIMITED";
      episodeError = result.text ?? "the shared rate class refused the call";
      break;
    }
  }

  // ── score and reap ───────────────────────────────────────────────────
  if (outcome === "COMPLETED") {
    try {
      claims = await session.claims(task.claims);
      recipeRef = await session.recipeRef();
    } catch (e) {
      // Terminal scoring is best effort; an unscored episode reports its
      // claims as absent rather than as failed.
      const why = `terminal verification did not complete: ${String(e?.message ?? e)}`;
      claims = task.claims.map((c) => ({
        name: c.name, verified: null, computed: null, absent: why,
      }));
      recipeRef = { absent: why };
    }
    // THE ISOLATION READING, in its own try: what this session's model holds
    // is a different question from whether its claims verified, and a
    // list_parts that fails must not void a claim the kernel already measured.
    try {
      modelScope = await session.modelScope();
    } catch (e) {
      modelScope = {
        absent: `the model-scope read did not complete: ${String(e?.message ?? e)}`,
      };
    }
  } else {
    // `episodeError` is null for BUDGET_EXHAUSTED / INVALID_ACTION (nothing
    // failed — a limit was reached), and a real message for CRASHED /
    // RATE_LIMITED, which then rides along into every absence.
    ({ claims, recipeRef, modelScope } = unscored(outcome, episodeError));
  }
  try { await session.close(); } catch { /* already dead */ }
  const reap = await reapDocument(baseUrl, authHeader, documentId);
  const partReap = await reapPart(baseUrl, authHeader, partId);

  const rewardFinal = mergeFinal(rewards);
  const wallMs = Date.now() - started;
  // Item 7 — computed for EVERY outcome, not only COMPLETED: unlike `claims`/
  // `recipeRef` above (which need the terminal claim-scoring call the loop
  // above only makes on COMPLETED), whether the agent verified its own
  // mutating work is a fact about the STEP HISTORY alone, and that history
  // exists no matter how the episode ended.
  traj.close({
    outcome, rewardFinal, claims, recipeRef, modelScope,
    tokens: policy.tokensUsed(), wallMs, error: episodeError,
    unverifiedMutations: deriveUnverifiedMutations(stepLog),
  });
  return {
    outcome, rewardFinal, documentId, partId, trajectoryPath, wallMs,
    error: episodeError, reap, partReap,
    // Returned as well as recorded, so a batch can report model isolation in
    // its own summary instead of leaving it to whoever reads the JSONL later.
    modelScope,
  };
}
