/**
 * Rows — the PURE half of ingestion.
 *
 * `rowsFromTrajectory(text, {path})` is a function from JSONL text to row
 * objects: no database, no filesystem, no clock. That is what lets a bad
 * ingest be replayed exactly from the same bytes, and what lets this module
 * be tested exhaustively against fixtures instead of a live Postgres.
 *
 * Two rules govern every branch below, both load-bearing and both stated
 * in the project's own words:
 *
 *   - ABSENCE IS STATED WITH A REASON, NEVER DEFAULTED. A trajectory whose
 *     `provenance` block is missing (every saved trajectory that predates
 *     `buildProvenance` looks exactly like this — no key at all, not even
 *     `{absent: ...}`) still produces a full row set, flagged
 *     `attributable: false`. Dropping those rows would make the corpus look
 *     cleaner than it is, which is the one thing this module must never do.
 *
 *   - A MALFORMED FILE NEVER THROWS AND IS NEVER SILENTLY SKIPPED. It
 *     produces a `quarantine` entry naming the reason. A file that could not
 *     be read is a FACT about the corpus, not an absence from it — so the
 *     entire body below runs under one outer try/catch, and every place a
 *     required record could be missing is checked explicitly rather than
 *     left to throw on first access.
 *
 * ─── run vs. episode ──────────────────────────────────────────────────────
 *
 * One JSONL file is one EPISODE. Several episodes can share one RUN — same
 * kernel build, same MCP entry, same policy, same harness commit, same
 * split — because `runBatch` (runner.mjs) resolves kernel identity and MCP
 * digest ONCE per batch and rebuilds `provenance` per episode only because
 * `policy.describe()` and the task can legitimately differ episode to
 * episode. Nothing on disk carries an explicit batch id, so `run_id` here is
 * a STATED APPROXIMATION: a digest over the fields that identify the
 * PRODUCER of the episode (kernel, mcp, policy, harness, split) — never
 * `provenance.task` (that is per-episode by construction) and never
 * `tool_allowlist` (a task field, not a batch field). Two sibling episodes
 * of one real batch collapse onto the same `run_id`; two episodes that
 * merely reused the same kernel sha on different days do too, and that is
 * the honest limit of what the file format can support without a batch id
 * — not silently claimed to be exact.
 *
 * ─── lineage edges (v1) ───────────────────────────────────────────────────
 *
 * An edge is derived ONLY from one recipe step's own `inputs` × `outputs`
 * cross product — never from the wider timeline DAG the document may also
 * carry. That scope was decided, not defaulted (see the plan's ledger:
 * "lineage_edge v1 carries only edges from the episode's own recipe steps,
 * not the full timeline DAG"). A step with no inputs (a fresh primitive) or
 * no outputs (`set_name`) contributes no edge.
 */

import { createHash } from "node:crypto";
import { digestOf, identityDetermined } from "../provenance.mjs";

function sha256Hex(text) {
  return "sha256:" + createHash("sha256").update(text).digest("hex");
}

function isPlainObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}

function isAbsent(v) {
  return isPlainObject(v) && typeof v.absent === "string";
}

/** A malformed-file result: every collection empty, one quarantine entry. */
function quarantined(path, reason) {
  return {
    run: null, episode: null, steps: [], refusals: [], claims: [],
    recipe: null, recipeSteps: [], solids: [], lineageEdges: [],
    quarantine: [{ path: path ?? null, reason }],
  };
}

/**
 * Parse the JSONL body. Throws (caught by the caller) naming exactly which
 * line failed and why, rather than letting `JSON.parse`'s own message —
 * which does not carry a line number — be the only trace.
 */
function parseLines(text) {
  const raw = String(text).split("\n");
  const lines = [];
  for (let idx = 0; idx < raw.length; idx += 1) {
    const line = raw[idx].trim();
    if (line === "") continue;
    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch (e) {
      throw new Error(`line ${idx + 1} is not valid JSON: ${e?.message ?? e}`);
    }
    lines.push(parsed);
  }
  return lines;
}

/**
 * The single field a consumer filters on — DERIVED FROM THE BLOCK, never
 * taken from the file's own boolean.
 *
 * This used to be `provenance?.attributable === true`, which trusted an
 * upstream invariant this function cannot check. Measured: a trajectory whose
 * kernel is a stated absence still produced `attributable = true` in both
 * `rl_run` and `rl_episode`, contradicting the JSONB in the same row.
 *
 * `buildProvenance` never emits that combination, so today's WRITER is safe —
 * but the writer is not the population this module reads. `ingestDir` ingests
 * every `.jsonl` in a directory: hand-edited files, files copied in from
 * another machine, files written by a future or regressed writer. That is
 * precisely the population `rl_quarantine` exists for. And `verify()` can
 * never catch this class of defect, because it compares bytes-since-ingest —
 * a file that was internally inconsistent when it was ingested verifies clean
 * forever. Twelve lines below, `recipeRowFrom` already states the rule this
 * now follows: never trust an upstream invariant it cannot check.
 *
 * THE SAME PREDICATE `buildProvenance` USES, imported rather than reimplemented
 * (`identityDetermined`, provenance.mjs) — two copies of the writer's verdict
 * and the reader's verdict would drift, and the reader's is the one a corpus
 * gets filtered on.
 *
 * The file's own claim is not erased when it disagrees: the whole block,
 * including its `attributable`, is stored in `rl_run.provenance`, so a reader
 * can see both the claim and the recomputed verdict. Only the derived column
 * is authoritative.
 *
 * All four shapes still collapse to one honest verdict without the caller
 * branching: a full attributable block, a full block that failed its own
 * check, a stated absence object (`{absent: "..."}` — no dimensions at all),
 * and a legacy file with no `provenance` key whatsoever.
 */
function attributableOf(provenance) {
  return identityDetermined(provenance?.kernel) &&
    identityDetermined(provenance?.mcp) &&
    identityDetermined(provenance?.policy) &&
    identityDetermined(provenance?.harness);
}

function runRowFrom({ header, provenance, attributable }) {
  // EVERY FIELD IS STATED, `null` INCLUDED. `digestOf` refuses an `undefined`
  // value rather than letting `JSON.stringify` drop the key (provenance.mjs) —
  // a header missing `schema_version` or `split` is an ordinary file, not a
  // malformed one, so its absence is written as an explicit `null` here rather
  // than left to vanish from the identity. `?? null` is a no-op for every
  // header that carries the key, so no existing digest moves.
  const identity = {
    schema_version: header.schema_version ?? null,
    kernel: provenance?.kernel ?? null,
    mcp: provenance?.mcp ?? null,
    policy: provenance?.policy ?? null,
    harness: provenance?.harness ?? null,
    split: header.split ?? null,
  };
  // A RUN ROW CARRIES ONLY WHAT THE RUN IDENTITY COVERS — with one stated
  // exception, `provenance`, noted at the end of this comment.
  //
  // It used to also carry `kernel_claimed` (`header.kernel_sha`), `mcp_version`
  // and `tool_allowlist` — none of which is part of `run_id`, twelve lines
  // above, deliberately and correctly. Because `upsertRun` is `ON CONFLICT
  // (run_id) DO NOTHING`, the first episode ingested under a run FROZE those
  // three columns for every sibling that ever shared it: edit a task's
  // allowlist, run again, ingest into the same directory, and the column still
  // reported the OLD list with no absence and no reason. That is a stale value
  // under a confident column name — the exact defect this package exists to
  // refuse — and no query anywhere read the columns.
  //
  // Nothing is lost. All three live per episode in the trajectory file, which
  // is the source of truth, and `mcp_version` additionally already sits inside
  // `provenance.mcp.version` in the JSONB below. Dropping is also the
  // REVERSIBLE direction: adding a per-episode column later is a non-
  // destructive `ALTER ... ADD COLUMN`, whereas removing one after a corpus
  // depends on it is the expensive migration.
  //
  // THE EXCEPTION, FOUND WHILE MAKING THAT CHANGE AND LEFT DELIBERATELY:
  // `provenance` below is stored whole, and it carries `task`, which the
  // identity above deliberately excludes. So `rl_run.provenance -> 'task'`
  // holds whichever sibling was ingested first — the same first-writer-wins
  // shape, one layer down in the JSONB. Not fixed here because nothing is
  // lost (every episode's own task identity is recorded per-episode, in
  // `rl_task` and in its trajectory file) and because changing what the run
  // document stores is a design decision, not a defect fix. See schema.mjs.
  return {
    run_id: digestOf(identity),
    schema_version: header.schema_version ?? null,
    split: header.split ?? null,
    provenance,
    attributable,
  };
}

function episodeRowFrom({ path, header, terminal, runId, attributable, sourceDigest }) {
  return {
    // `started_at ?? null` for the same reason as `runRowFrom`'s identity: a
    // header without it is stated as null rather than dropped from the digest.
    episode_id: digestOf({ run_id: runId, task_id: header.task_id, seed: header.seed, started_at: header.started_at ?? null }),
    run_id: runId,
    path: path ?? null,
    task_id: header.task_id,
    seed: header.seed,
    started_at: header.started_at ?? null,
    outcome: terminal.outcome,
    attributable,
    reward_final: terminal.reward_final ?? null,
    tokens: terminal.tokens ?? null,
    wall_ms: terminal.wall_ms ?? null,
    // ALWAYS PRESENT: `null` reads as "the episode reported no error", never
    // as "this record forgot to say" — terminal.error carries the same
    // always-present convention trajectory.mjs's close() documents.
    error: terminal.error ?? null,
    model_scope: terminal.model_scope ?? { absent: "the terminal record carried no model_scope" },
    source_digest: sourceDigest,
  };
}

function stepRows(episodeId, stepLines) {
  return stepLines.map((s) => ({
    episode_id: episodeId,
    index: s.i,
    tool: s.action?.tool ?? null,
    args: s.action?.args ?? null,
    result_digest: s.result_digest ?? null,
    reward: s.reward ?? null,
    duration_ms: s.ms ?? null,
  }));
}

function refusalRows(episodeId, stepLines) {
  return stepLines
    .filter((s) => s.refusal != null)
    .map((s) => ({
      episode_id: episodeId,
      step_index: s.i,
      gate: s.refusal.gate ?? null,
      reason: s.refusal.reason ?? null,
    }));
}

function claimRows(episodeId, claims) {
  return (claims ?? []).map((c) => ({
    episode_id: episodeId,
    name: c.name,
    verified: c.verified ?? null,
    expected: c.expected ?? null,
    computed: c.computed ?? null,
    abs_error: c.abs_error ?? null,
    tolerance_used: c.tolerance_used ?? null,
    // never defaulted to a made-up sentence: only carried when the claim
    // itself stated one (the `verified: null` branch trajectory.mjs names).
    absent: c.absent ?? null,
  }));
}

/**
 * `terminal.recipe_ref` takes one of three real shapes:
 *   - absent entirely (null/undefined) — defensive; the schema says it is
 *     never a bare null, but this function never trusts an upstream
 *     invariant it cannot check;
 *   - a bare `{absent: "<reason>"}` — every non-COMPLETED outcome, per
 *     trajectory.mjs's own docstring (terminal scoring never ran, so there
 *     is no descriptor to embed at all);
 *   - a full descriptor (`retrieved_by`, `reference`, `source`, `step_count`,
 *     ..., `steps`) which MAY ALSO carry its own top-level `absent` — the
 *     real, observed case (first-live-trajectory.jsonl, live-isolated-8/*):
 *     a COMPLETED episode whose durable log reported zero steps still
 *     returns the whole descriptor, with `absent` explaining why `steps` is
 *     empty rather than replacing the descriptor with a bare absence.
 */
function recipeRowFrom(episodeId, recipeRef) {
  if (recipeRef == null) return null;
  if (isAbsent(recipeRef) && !("reference" in recipeRef)) {
    return { episode_id: episodeId, absent: recipeRef.absent };
  }
  return {
    episode_id: episodeId,
    retrieved_by: recipeRef.retrieved_by ?? null,
    reference: recipeRef.reference ?? null,
    source: recipeRef.source ?? null,
    step_count: recipeRef.step_count ?? null,
    sequence_range: recipeRef.sequence_range ?? null,
    sequence_contiguous: recipeRef.sequence_contiguous ?? null,
    undecodable_events: recipeRef.undecodable_events ?? null,
    checkpoints: recipeRef.checkpoints ?? [],
    certificate_summary: recipeRef.certificate_summary ?? null,
    note: recipeRef.note ?? null,
    absent: recipeRef.absent ?? null,
  };
}

function recipeStepRows(episodeId, recipeRef) {
  const steps = recipeRef?.steps;
  if (!Array.isArray(steps)) return [];
  return steps.map((st) => ({
    episode_id: episodeId,
    sequence: st.sequence,
    op_kind: st.op_kind ?? null,
    params: st.params ?? null,
    inputs: st.inputs ?? [],
    outputs: st.outputs ?? [],
    intent: st.intent ?? null,
    checkpoint: st.checkpoint ?? null,
    checkpoint_absent_reason: st.checkpoint_absent_reason ?? null,
    // THE REISSUE MAPPING: an object when a re-issue route exists, or `null`
    // with `reissue_absent_reason` naming why not (e.g. `set_name` has none)
    // — never silently dropped either way.
    reissue: st.reissue ?? null,
    reissue_absent_reason: st.reissue_absent_reason ?? null,
  }));
}

function solidRows(episodeId, recipeRef) {
  const steps = recipeRef?.steps;
  if (!Array.isArray(steps)) return [];
  const rows = [];
  for (const st of steps) {
    for (const token of st.outputs ?? []) {
      if (typeof token !== "string") continue;
      rows.push({
        episode_id: episodeId, token,
        produced_by_sequence: st.sequence, op_kind: st.op_kind ?? null,
      });
    }
  }
  return rows;
}

/** Lineage edges v1 — see the module docstring for the scope decision. */
function lineageEdgeRows(episodeId, recipeRef) {
  const steps = recipeRef?.steps;
  if (!Array.isArray(steps)) return [];
  const edges = [];
  for (const st of steps) {
    const inputs = st.inputs ?? [];
    const outputs = st.outputs ?? [];
    if (inputs.length === 0 || outputs.length === 0) continue;
    for (const from of inputs) {
      for (const to of outputs) {
        edges.push({
          episode_id: episodeId, from, to,
          via_sequence: st.sequence, op_kind: st.op_kind ?? null,
        });
      }
    }
  }
  return edges;
}

export function rowsFromTrajectory(text, { path } = {}) {
  try {
    const lines = parseLines(text);
    const header = lines.find((l) => l?.kind === "header");
    if (!header) {
      return quarantined(path, "no header record found: the first record must have kind \"header\"");
    }
    if (typeof header.task_id !== "string" || header.task_id === "") {
      return quarantined(path, "the header record carries no task_id");
    }
    if (typeof header.seed !== "number") {
      return quarantined(path, "the header record carries no numeric seed");
    }
    const terminal = lines.find((l) => l?.kind === "terminal");
    if (!terminal) {
      return quarantined(path, "no terminal record found: the file has no record with kind \"terminal\"");
    }
    if (typeof terminal.outcome !== "string" || terminal.outcome === "") {
      return quarantined(path, "the terminal record carries no outcome");
    }
    const stepLines = lines.filter((l) => l?.kind === "step");

    // See the module docstring: absent entirely (legacy files) and a stated
    // `{absent: reason}` both collapse to the same honest verdict below.
    const provenance = "provenance" in header
      ? header.provenance
      : { absent: "the header carried no provenance block (this file predates provenance being written, or the writer skipped it)" };
    const attributable = attributableOf(provenance);

    const run = runRowFrom({ header, provenance, attributable });
    const sourceDigest = sha256Hex(String(text));
    const episode = episodeRowFrom({
      path, header, terminal, runId: run.run_id, attributable, sourceDigest,
    });

    const recipeRef = terminal.recipe_ref;
    return {
      run,
      episode,
      steps: stepRows(episode.episode_id, stepLines),
      refusals: refusalRows(episode.episode_id, stepLines),
      claims: claimRows(episode.episode_id, terminal.claims),
      recipe: recipeRowFrom(episode.episode_id, recipeRef),
      recipeSteps: recipeStepRows(episode.episode_id, recipeRef),
      solids: solidRows(episode.episode_id, recipeRef),
      lineageEdges: lineageEdgeRows(episode.episode_id, recipeRef),
    };
  } catch (e) {
    return quarantined(path, `the file could not be parsed as a trajectory: ${e?.message ?? e}`);
  }
}
