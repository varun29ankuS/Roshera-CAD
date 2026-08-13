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
 * `spawn` is injectable so the lifecycle can be tested without a backend.
 */
import { openTrajectory } from "./trajectory.mjs";
import { rewardFromResult, mergeFinal } from "./reward.mjs";
import { assertActionAllowed } from "./policy.mjs";
import { spawnMcpSession } from "./mcp_session.mjs";

const digest = (v) => {
  const s = JSON.stringify(v ?? null);
  let h = 0n;
  for (const ch of s) h = (h * 1099511628211n ^ BigInt(ch.codePointAt(0))) & 0xffffffffffffffffn;
  return `fnv1a64:${h.toString(16)}`;
};

/**
 * Delete the episode's document, best-effort. Two call sites share this so
 * they cannot drift apart: the normal end-of-episode reap, and the
 * SETUP_FAILED path where `spawn` failed AFTER document creation already
 * succeeded — that document is just as real and must not be orphaned in
 * PartManager's DashMap. A failed DELETE here must never itself become a
 * thrown error; `runner.mjs`'s reaper is the backstop for whatever this
 * best-effort attempt misses.
 */
async function reapDocument(baseUrl, authHeader, documentId) {
  if (!documentId) return;
  try {
    await fetch(`${baseUrl}/api/documents/${documentId}`, {
      method: "DELETE", headers: { ...authHeader },
    });
  } catch { /* the reaper in runner.mjs is the backstop */ }
}

export async function runEpisode({
  task, policy, seed, baseUrl, authHeader, mcpEntry, trajectoryPath,
  kernelSha, mcpVersion = "0.1.0", spawn = spawnMcpSession,
}) {
  const started = Date.now();
  const traj = openTrajectory({
    path: trajectoryPath, taskId: task.id, seed, kernelSha, mcpVersion,
    toolAllowlist: [...task.toolAllowlist], split: task.split,
  });

  // ── setup ────────────────────────────────────────────────────────────
  let documentId = null;
  let session = null;
  try {
    const res = await fetch(`${baseUrl}/api/documents`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...authHeader },
      body: JSON.stringify({ name: `rl:${task.id}:${seed}` }),
    });
    if (!res.ok) throw new Error(`document creation returned ${res.status}`);
    documentId = (await res.json())?.id ?? null;
    if (!documentId) throw new Error("document creation returned no id");
    session = await spawn({ documentId, baseUrl, authHeader, mcpEntry });
  } catch (e) {
    // `documentId` is set the moment creation succeeds, before `spawn` is
    // even called — if spawn is what failed, that document is real and
    // orphaned unless reaped here too.
    await reapDocument(baseUrl, authHeader, documentId);
    traj.close({
      outcome: "SETUP_FAILED",
      rewardFinal: mergeFinal([]),
      claims: [], recipeRef: null, tokens: 0, wallMs: Date.now() - started,
    });
    return {
      outcome: "SETUP_FAILED", rewardFinal: mergeFinal([]), documentId,
      trajectoryPath, wallMs: Date.now() - started, error: String(e?.message ?? e),
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
      action = await policy.act({ task, observation, history: rewards });
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
      // counts it, the same as any kernel-issued refusal.
      outcome = "INVALID_ACTION";
      const reward = { components: { refused: "harness_allowlist" }, gaps: [] };
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
      outcome = e?.status === 429 ? "RATE_LIMITED" : "CRASHED";
      traj.step({
        i, action, resultDigest: null,
        reward: { components: {}, gaps: [{ name: "sound", reason: `call failed: ${e?.message ?? e}` }] },
        refusal: null, ms: Date.now() - t0,
      });
      break;
    }
    const reward = rewardFromResult(result);
    rewards.push(reward);
    observation = result;
    traj.step({
      i, action, resultDigest: digest(result), reward,
      refusal: result?.refused ? { gate: result.gate, reason: result.reason } : null,
      ms: Date.now() - t0,
    });
  }

  // ── score and reap ───────────────────────────────────────────────────
  if (outcome === "COMPLETED") {
    try {
      claims = await session.claims(task.claims);
      recipeRef = await session.recipeRef();
    } catch {
      // Terminal scoring is best effort; an unscored episode reports its
      // claims as absent rather than as failed.
      claims = task.claims.map((c) => ({
        name: c.name, verified: null,
        absent: "terminal verification did not complete",
      }));
    }
  }
  try { await session.close(); } catch { /* already dead */ }
  await reapDocument(baseUrl, authHeader, documentId);

  const rewardFinal = mergeFinal(rewards);
  const wallMs = Date.now() - started;
  traj.close({
    outcome, rewardFinal, claims, recipeRef,
    tokens: policy.tokensUsed(), wallMs,
  });
  return { outcome, rewardFinal, documentId, trajectoryPath, wallMs, error: episodeError };
}
