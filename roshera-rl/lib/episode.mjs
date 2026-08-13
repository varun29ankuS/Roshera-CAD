/**
 * One episode: create a document, spawn an MCP process PINNED to it, drive
 * the policy to done, score, reap.
 *
 * The episode is a real MCP session, not a simulation of one — same
 * ToolTable, same gates, same typed refusals. Isolation comes from the
 * process boundary: every piece of episode state in gates.ts is a
 * module-level binding, so two episodes cannot contaminate each other
 * because they do not share an address space. Process death is the reset.
 *
 * `spawn` is injectable so the lifecycle can be tested without a backend. An
 * injected session must return `mcp_session.readToolResult` ENVELOPES from
 * `call`, exactly as the real one does — there is one result shape in this
 * package, and a fake that speaks a friendlier one would certify itself.
 */
import { openTrajectory } from "./trajectory.mjs";
import { rewardFromResult, mergeFinal } from "./reward.mjs";
import { assertActionAllowed } from "./policy.mjs";
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
  SPAWN: "spawning the MCP session",
});

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
  kernelSha, mcpVersion = "0.1.0", spawn = spawnMcpSession,
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
      toolAllowlist: [...task.toolAllowlist], split: task.split,
    });
  } catch (e) {
    return {
      outcome: "SETUP_FAILED", rewardFinal: mergeFinal([]), documentId: null,
      trajectoryPath, wallMs: Date.now() - started,
      error: `the trajectory could not be opened: ${String(e?.message ?? e)}`,
      reap: { reaped: null, reason: "no document was created" },
    };
  }

  /**
   * Claims/recipe for an episode that never reached terminal scoring.
   *
   * `detail` is the concrete failure — which stage, and the error text it
   * carried — appended to the outcome's standing reason. Without it a reader
   * of the trajectory learns only the category, which is what made two
   * different SETUP_FAILED episodes read identically.
   */
  const unscored = (outcome, detail) => {
    const reason = detail
      ? `${NO_TERMINAL_SCORING[outcome]} — ${detail}`
      : NO_TERMINAL_SCORING[outcome];
    return {
      claims: task.claims.map((c) => ({
        name: c.name, verified: null, computed: null, absent: reason,
      })),
      recipeRef: { absent: reason },
    };
  };

  // ── setup ────────────────────────────────────────────────────────────
  let documentId = null;
  let session = null;
  // Which stage is in flight, so the catch below can NAME it. Setup is two
  // independent pieces of I/O against two different failure surfaces (an HTTP
  // API and a child process), and collapsing them into one reason discards the
  // only fact a diagnosis starts from.
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
    setupStage = SETUP_STAGE.SPAWN;
    session = await spawn({ documentId, baseUrl, authHeader, mcpEntry });
  } catch (e) {
    // `documentId` is set the moment creation succeeds, before `spawn` is
    // even called — if spawn is what failed, that document is real and
    // orphaned unless reaped here too.
    const reap = await reapDocument(baseUrl, authHeader, documentId);
    const outcome = e?.rateLimited === true ? "RATE_LIMITED" : "SETUP_FAILED";
    // The stage AND the underlying message. The error used to be dropped here
    // and rebuilt as a disjunction, so a 401 from the API and a child process
    // that could not start produced byte-identical records.
    const error = `${setupStage} failed: ${String(e?.message ?? e)}`;
    const { claims, recipeRef } = unscored(outcome, error);
    traj.close({
      outcome,
      rewardFinal: mergeFinal([]),
      claims, recipeRef, tokens: 0, wallMs: Date.now() - started, error,
    });
    return {
      outcome, rewardFinal: mergeFinal([]), documentId,
      trajectoryPath, wallMs: Date.now() - started, error,
      reap,
    };
  }

  // ── drive ────────────────────────────────────────────────────────────
  const rewards = [];
  let outcome = "BUDGET_EXHAUSTED";
  let observation = null;
  let claims = [];
  let recipeRef = null;
  // Carried out on the returned object the way SETUP_FAILED already does —
  // a crash with no recorded reason is a dead end for whoever reads the
  // batch tally afterward.
  let episodeError = null;

  for (let i = 0; i < task.stepBudget; i += 1) {
    let action;
    try {
      // A FROZEN SNAPSHOT of the reward history: `policy.act` is arbitrary
      // third-party code, and handing it the live array it could push to (or
      // whose entries it could edit) would let a policy rewrite the record of
      // its own episode — in the same call that freezes the task and the
      // script precisely to stop that.
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
      rewards.push(reward);
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
    rewards.push(reward);
    observation = result;
    traj.step({
      i, action, resultDigest: digest(result?.data ?? result?.text ?? null), reward,
      refusal: result?.refusal
        ? {
            gate: result.refusal.gate,
            reason: result.data?.reason ?? result.data?.refused?.error ?? result.text ?? null,
          }
        : null,
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
  } else {
    // `episodeError` is null for BUDGET_EXHAUSTED / INVALID_ACTION (nothing
    // failed — a limit was reached), and a real message for CRASHED /
    // RATE_LIMITED, which then rides along into every absence.
    ({ claims, recipeRef } = unscored(outcome, episodeError));
  }
  try { await session.close(); } catch { /* already dead */ }
  const reap = await reapDocument(baseUrl, authHeader, documentId);

  const rewardFinal = mergeFinal(rewards);
  const wallMs = Date.now() - started;
  traj.close({
    outcome, rewardFinal, claims, recipeRef,
    tokens: policy.tokensUsed(), wallMs, error: episodeError,
  });
  return {
    outcome, rewardFinal, documentId, trajectoryPath, wallMs,
    error: episodeError, reap,
  };
}
