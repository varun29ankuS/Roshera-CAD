/**
 * Run N episodes under a concurrency cap, tally the outcomes, and REAP.
 *
 * The cap is real: process-per-episode means memory, and the api-server's
 * EvalHarness rate class (6000 req/min) is shared across every concurrent
 * episode. Both are measured here rather than assumed — RATE_LIMITED is its
 * own outcome precisely so the ceiling shows up in the tally instead of
 * being averaged into a lower score.
 */
import { join } from "node:path";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runEpisode, reapDocument, reapPart, unscoredFor, MCP_VERSION } from "./episode.mjs";
import { OUTCOMES, openTrajectory } from "./trajectory.mjs";
import { spawnMcpSession, defaultMcpEntry } from "./mcp_session.mjs";
import { mergeFinal } from "./reward.mjs";
import { resolveKernelIdentity, buildProvenance, resolveBatchIdentity, digestOf } from "./provenance.mjs";

/**
 * THIS FILE'S OWN LOCATION → the `roshera-rl` package root, the default
 * `harnessRoot` `buildProvenance` reads the harness's own git identity from.
 *
 * `new URL("..", import.meta.url).pathname` would do here what it does at
 * every other call site on this machine: percent-encode the space in
 * `C:\Users\Varun Sharma\` and hand `git`/`fs` a path that does not exist —
 * `fileURLToPath` decodes it back to a real one. Same reasoning as
 * `defaultMcpEntry` (mcp_session.mjs:194-196), which this module now also
 * relies on for the same class of bug.
 */
const DEFAULT_HARNESS_ROOT = fileURLToPath(new URL("..", import.meta.url));

/**
 * THE REAPER — the batch-level backstop `episode.mjs` names.
 *
 * Each episode already attempts its own DELETEs and REPORTS the outcomes
 * (`result.reap` for its document, `result.partReap` for its part). This
 * retries every document and every part that attempt could not drop —
 * a DELETE lost to a network blip, or refused because the document was
 * momentarily the active one (api-server/src/documents.rs:561-564) — and then
 * states plainly which documents are STILL orphaned in PartManager's DashMap.
 *
 * One retry pass, not a loop: a document the backend refuses twice is not
 * going to yield to a third identical request, and a batch runner that spins
 * on cleanup would hold the rate class the next batch needs. What it will not
 * do is claim a cleanup it did not achieve — the survivors are returned so
 * the caller can print them.
 */
export async function reapOrphans({ baseUrl, authHeader, results }) {
  const orphans = [];
  for (const r of results) {
    // The PART is retried on the same terms as the document: it holds a whole
    // `BRepModel` in `PartManager`'s DashMap (api-server/src/part_mgr.rs:97)
    // and nothing on the document's own delete path drops it.
    if (r?.partId && r.partReap?.reaped !== true) {
      const retry = await reapPart(baseUrl, authHeader, r.partId);
      if (retry.reaped === true) {
        r.partReap = { reaped: true, reason: `reaped by the batch reaper after: ${r.partReap?.reason ?? "no first attempt"}` };
      } else {
        r.partReap = { reaped: false, reason: retry.reason };
        orphans.push({ documentId: null, partId: r.partId, reason: retry.reason });
      }
    }
    if (!r?.documentId) continue;
    if (r.reap?.reaped === true) continue;
    const retry = await reapDocument(baseUrl, authHeader, r.documentId);
    if (retry.reaped === true) {
      r.reap = { reaped: true, reason: `reaped by the batch reaper after: ${r.reap?.reason ?? "no first attempt"}` };
      continue;
    }
    r.reap = { reaped: false, reason: retry.reason };
    orphans.push({ documentId: r.documentId, partId: null, reason: retry.reason });
  }
  return orphans;
}

/**
 * The record for an episode that never reached `runEpisode` — the failure
 * happened in one of the TWO pieces of THIRD-PARTY CODE the worker loop runs
 * before handing off: the policy factory (`policyFor`), or `policy.describe()`
 * called from inside `buildProvenance`. Either way `runEpisode` was never
 * entered and cannot record it, so this does — one shared record shape for
 * both, so the two cannot drift into two different "no episode happened"
 * stories.
 *
 * `SETUP_FAILED`, not `CRASHED`: the taxonomy is closed (trajectory.mjs
 * OUTCOMES — a new category is a design change, not a typo), and of the six,
 * SETUP_FAILED is the one that means "no episode happened". `CRASHED` says the
 * MCP process died, and here no process was ever spawned — no document was
 * created either, which is why `documentId` is null and nothing is reaped.
 *
 * A TRAJECTORY IS STILL WRITTEN. A batch is read afterwards from its
 * trajectories, so a result carrying a `trajectoryPath` that no file backs is
 * an episode nobody can diagnose without re-running the batch — the same
 * defect the `error` field was added to `close()` to fix. The header/terminal
 * pair here is the same shape every other SETUP_FAILED episode writes, built
 * from the same `unscoredFor` so the two cannot drift.
 *
 * `detail` is the CALLER'S already-composed reason — which of the two
 * third-party calls threw, and its own message — so the two call sites below
 * cannot produce two subtly different sentences for the same taxonomy entry.
 *
 * `provenance` is the block built by `provenanceForSetupFailure` below. It is
 * REQUIRED here, not optional: both call sites run after the batch has already
 * resolved kernel/mcp/harness, so falling through to `openTrajectory`'s
 * last-resort default would write a reason ("the caller passed no provenance
 * block…") that is false at both of them — and `rows.mjs` persists
 * `header.provenance` whole into `rl_run.provenance`, so the falsehood would
 * land in the corpus rather than merely in a file.
 */
function setupFailedBeforeEpisode({ item, trajectoryPath, kernelSha, detail, provenance }) {
  const rewardFinal = mergeFinal([]);
  const { claims, recipeRef, modelScope } = unscoredFor(item.task, "SETUP_FAILED", detail);
  try {
    const traj = openTrajectory({
      path: trajectoryPath, taskId: item.task.id, seed: item.seed, kernelSha,
      mcpVersion: MCP_VERSION, toolAllowlist: [...item.task.toolAllowlist],
      split: item.task.split, provenance,
    });
    traj.close({
      outcome: "SETUP_FAILED", rewardFinal, claims, recipeRef, modelScope,
      tokens: 0, wallMs: 0, error: detail,
    });
  } catch (e) {
    // An unwritable outDir must not turn one episode's setup failure into a
    // dead batch — that is the very defect this function exists to remove. The
    // returned result still carries the reason, and says the record is missing
    // rather than leaving a dangling path unexplained.
    return {
      outcome: "SETUP_FAILED", rewardFinal, documentId: null, partId: null,
      trajectoryPath: null, wallMs: 0,
      error: `${detail} (and its trajectory could not be written: ${String(e?.message ?? e)})`,
      reap: { reaped: null, reason: "no document was created" },
      partReap: { reaped: null, reason: "no part was created" },
      modelScope,
    };
  }
  return {
    outcome: "SETUP_FAILED", rewardFinal, documentId: null, partId: null,
    trajectoryPath, wallMs: 0, error: detail,
    reap: { reaped: null, reason: "no document was created" },
    partReap: { reaped: null, reason: "no part was created" },
    modelScope,
  };
}

/**
 * The provenance block for an episode that failed BEFORE `buildProvenance`
 * could return one.
 *
 * Three of the four identity dimensions are already known at both call sites:
 * `kernel`, `mcp` and `harness` are resolved ONCE for the whole batch before
 * the queue exists, and `task` is `defineTask`-validated. Only the POLICY is
 * genuinely unknown — either its factory threw before a policy existed, or its
 * own `describe()` threw. So exactly one dimension becomes a stated absence
 * and the rest are recorded as resolved. Discarding all four (which is what
 * falling through to `openTrajectory`'s default did) threw away facts the
 * batch had already established and made the episode's `run_id` a phantom
 * carrying nothing but a false sentence.
 *
 * `attributable: false` is PROVEN, not defaulted: the policy dimension is an
 * absence, and one absent identity makes the block unattributable — the same
 * rule `buildProvenance` applies.
 *
 * THIS FUNCTION CANNOT THROW. It is the failure path; a failure record that
 * fails leaves an episode nobody can diagnose. `digestOf` refuses a task shape
 * it cannot represent faithfully (provenance.mjs), so that refusal is caught
 * here and becomes a stated absence on the task dimension rather than an
 * exception escaping the worker.
 */
function provenanceForSetupFailure({ kernel, mcp, harness, task, policyAbsent }) {
  let taskIdentity;
  try {
    taskIdentity = { id: task.id, family: task.family, digest: digestOf(task) };
  } catch (e) {
    taskIdentity = {
      id: task.id, family: task.family,
      absent: `the task could not be digested: ${String(e?.message ?? e)}`,
    };
  }
  return {
    kernel,
    mcp,
    harness,
    policy: { absent: policyAbsent },
    task: taskIdentity,
    attributable: false,
  };
}

export async function runBatch({
  tasks, policyFor, seeds, concurrency = 4, baseUrl, authHeader = {},
  outDir, kernelSha, mcpEntry, harnessRoot = DEFAULT_HARNESS_ROOT, spawn = spawnMcpSession,
}) {
  mkdirSync(outDir, { recursive: true });

  // THE REFUSAL HAPPENS HERE — before the queue exists, before a single
  // worker is started, before anything is spawned. `kernelSha` here is the
  // OPERATOR'S CLAIM (e.g. ROSHERA_KERNEL_SHA); it is asked of the server
  // exactly once for the whole batch, and a genuine disagreement is a
  // `KernelIdentityConflict` that PROPAGATES OUT OF THIS FUNCTION uncaught.
  // It must never be caught into a per-episode outcome: that would turn one
  // honest "this batch cannot say which kernel produced it" into eight
  // reported failures, each looking like its own defect.
  const kernel = await resolveKernelIdentity({ baseUrl, authHeader, claimed: kernelSha });

  // Resolved ONCE, not per episode: the MCP entry path is the same child
  // every episode in this batch spawns, so its digest — and the fact that it
  // could or could not be read — belongs in every episode's provenance
  // identically. `defaultMcpEntry()` is the same real filesystem path
  // `spawnMcpSession` itself falls back to (mcp_session.mjs:504-511); reusing
  // it here means the digest `buildProvenance` records is a digest of the
  // file that actually ran, not of some other candidate path.
  const resolvedMcpEntry = mcpEntry ?? defaultMcpEntry();

  // BATCH-INVARIANT, RESOLVED ONCE: neither the mcp digest nor the harness's
  // own git identity depends on which task or policy an episode runs, so
  // recomputing them per episode was pure waste — a file re-read and two
  // `git` subprocesses spawned again for every episode in the batch — and
  // worse than waste: two episodes in the SAME batch could disagree about
  // their own harness if the tree changed mid-run (a `git status` catching a
  // checkout in progress), making one batch internally inconsistent about
  // its own provenance. `mcp`/`harness` below are handed unchanged to every
  // episode's `buildProvenance` call; only `policy`/`task` still vary and are
  // still recomputed per episode.
  const { mcp, harness } = await resolveBatchIdentity({
    mcpEntry: resolvedMcpEntry, harnessRoot,
  });

  const queue = tasks.map((task, i) => ({ task, seed: seeds[i] ?? i, i }));
  const results = [];

  const worker = async () => {
    for (;;) {
      const item = queue.shift();
      if (item === undefined) return;
      const trajectoryPath = join(outDir, `${item.task.id}-${item.seed}-${item.i}.jsonl`);
      // THE POLICY FACTORY IS THIRD-PARTY CODE AND RUNS BEFORE THE EPISODE.
      // It used to be called while building `runEpisode`'s argument object,
      // i.e. outside the `.catch` below: a factory that threw rejected this
      // worker, then `Promise.all`, then the whole batch — taking down every
      // sibling episode, including ones that had already finished and been
      // recorded. Every other failure mode here is a named per-episode
      // outcome, and this one now is too.
      let policy;
      try {
        policy = policyFor(item.task, item.seed);
      } catch (e) {
        const detail = `the policy factory threw before the episode began: ${String(e?.message ?? e)}`;
        results.push(
          setupFailedBeforeEpisode({
            item, trajectoryPath, kernelSha, detail,
            provenance: provenanceForSetupFailure({
              kernel, mcp, harness, task: item.task,
              policyAbsent: `the policy factory threw before a policy existed to describe ` +
                `itself, so no policy identity could be obtained: ${String(e?.message ?? e)}`,
            }),
          }),
        );
        continue;
      }
      // `buildProvenance` calls `policy.describe()` SYNCHRONOUSLY, and
      // `policy` is exactly as third-party as `policyFor` above — this used
      // to sit unguarded here, so a policy whose `describe()` threw rejected
      // this worker, then `Promise.all`, then the WHOLE BATCH, discarding
      // every sibling episode that had already completed. Same fix, same
      // shape: caught here, recorded as this one episode's own SETUP_FAILED,
      // and the worker moves on to the next item instead of taking the batch
      // down with it.
      //
      // Built per episode, not once for the batch: `policy.describe()` can
      // differ episode to episode (a policy factory is free to return a
      // different policy per task/seed), and the task itself always does —
      // so the `attributable` verdict has to be recomputed for what THIS
      // episode actually ran, not copied from its first sibling. `mcp` and
      // `harness`, in contrast, ARE copied from the one batch-wide resolution
      // above — see the comment there for why recomputing those per episode
      // was itself a defect.
      let provenance;
      try {
        provenance = await buildProvenance({ kernel, policy, task: item.task, mcp, harness });
      } catch (e) {
        results.push(
          setupFailedBeforeEpisode({
            item, trajectoryPath, kernelSha,
            detail: `building the episode's provenance record threw before the episode began ` +
              `(policy.describe() is third-party code): ${String(e?.message ?? e)}`,
            provenance: provenanceForSetupFailure({
              kernel, mcp, harness, task: item.task,
              policyAbsent: `assembling the provenance block threw while calling the policy's ` +
                `own describe(), so no policy identity could be obtained: ${String(e?.message ?? e)}`,
            }),
          }),
        );
        continue;
      }
      // An episode never throws: every failure mode is a named outcome. A
      // worker that could die on one episode would silently shrink the batch.
      const r = await runEpisode({
        task: item.task, policy, seed: item.seed,
        baseUrl, authHeader, trajectoryPath, kernelSha, mcpEntry: resolvedMcpEntry,
        spawn, provenance,
      }).catch((e) => ({
        outcome: "CRASHED", rewardFinal: { components: {}, gaps: [] },
        documentId: null, partId: null, trajectoryPath, wallMs: 0,
        error: String(e?.message ?? e),
        reap: { reaped: null, reason: "the episode threw before reporting a document" },
        partReap: { reaped: null, reason: "the episode threw before reporting a part" },
        modelScope: { absent: "the episode threw before any model could be read" },
      }));
      results.push(r);
    }
  };

  await Promise.all(
    Array.from({ length: Math.max(1, Math.min(concurrency, queue.length)) }, worker),
  );

  const orphans = await reapOrphans({ baseUrl, authHeader, results });

  // Every outcome name appears, zeros included: an absent key reads as "not
  // measured", which is the one thing this taxonomy exists to prevent.
  const tally = Object.fromEntries(OUTCOMES.map((o) => [o, 0]));
  for (const r of results) tally[r.outcome] = (tally[r.outcome] ?? 0) + 1;
  return { results, tally, orphans };
}
