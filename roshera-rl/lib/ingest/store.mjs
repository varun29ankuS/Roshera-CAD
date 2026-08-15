/**
 * Store — the IMPURE half of ingestion: takes `rowsFromTrajectory`'s pure
 * output (Task 5, `lib/ingest/rows.mjs`) and lands it in Postgres.
 *
 * IDEMPOTENCY, the load-bearing property here, is split across two table
 * families on purpose (see schema.mjs's own docstring for the full
 * rationale):
 *
 *   - ONE ROW PER EPISODE (`rl_episode`, `rl_recipe`, `rl_certificate`):
 *     `INSERT ... ON CONFLICT (episode_id) DO UPDATE` — literally the shape
 *     the brief asks for ("Upsert keyed on (run_id, episode_id) with the
 *     source digest"). `episode_id` alone is the conflict target because it
 *     already embeds `run_id` (rows.mjs: `digestOf({run_id, task_id, seed,
 *     started_at})`), so a composite key would be redundant. `rl_recipe`/
 *     `rl_certificate` additionally DELETE (never re-INSERT after) when the
 *     row should stop existing at all — a recipe absent this ingest, or a
 *     certificate_summary that vanished from an otherwise-present recipe —
 *     because an upsert alone cannot make a row disappear.
 *
 *   - MANY ROWS PER EPISODE (`rl_step`, `rl_refusal`, `rl_claim_result`,
 *     `rl_recipe_step`, `rl_solid`, `rl_lineage_edge`): `DELETE FROM <table>
 *     WHERE episode_id = $1` immediately before the bulk re-insert, inside
 *     the SAME transaction as the episode upsert. Re-ingesting the same
 *     file always takes this delete-then-insert path (there is no
 *     "unchanged, skip" short-circuit) — an offline loader re-reading its
 *     own fixture is not a hot path worth optimizing away, and skipping it
 *     would hide exactly the bug this design mutation-proves against: THE
 *     DELETE IS THE ENTIRE IDEMPOTENCY GUARD for this table family. Comment
 *     it out and a second `ingestFile` of the same path doubles every one
 *     of those tables' row counts for that episode — no Postgres error,
 *     because there is deliberately no uniqueness constraint on the natural
 *     key for this family (only a surrogate `BIGSERIAL` id) forcing that
 *     guard to live in application code where it can be mutation-tested.
 *
 * Every write for one file happens inside ONE transaction ("one transaction
 * per episode so a partial ingest leaves whole episodes" — brief, Step 3):
 * a crash mid-ingest leaves the previous episode's rows exactly as they
 * were, never half-written.
 *
 * `rl_quarantine` is idempotent the same way as the one-row-per-episode
 * family: `ON CONFLICT (path) DO UPDATE`. And the reverse transition is
 * handled too: `ingestFile`'s success path DELETEs any existing
 * `rl_quarantine` row for that path in the SAME transaction as the episode
 * write, so a path that used to be malformed and now ingests cleanly never
 * leaves a stale quarantine entry for `verify()` to misreport as drift.
 */

import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { rowsFromTrajectory } from "./rows.mjs";
import { digestOf } from "../provenance.mjs";

/**
 * Same algorithm as rows.mjs's private `sha256Hex` (not exported there, and
 * rows.mjs is deliberately left unmodified — this store is out of that
 * module's scope). Duplicated rather than imported: three lines, no shared
 * state, and rows.mjs's own `episode.source_digest` field is already this
 * exact value for any file that parses, so the two never disagree.
 */
function sha256Hex(text) {
  return "sha256:" + createHash("sha256").update(text).digest("hex");
}

/** `undefined`/`null` become SQL NULL; everything else is stored as JSON text, which Postgres's jsonb input function parses on the way in. Node's `pg` driver does NOT do this for you — a bare JS array/object passed as a parameter is serialized as a Postgres ARRAY LITERAL, not JSON, and corrupts a jsonb column silently. */
function jsonb(v) {
  return v === undefined || v === null ? null : JSON.stringify(v);
}

function nn(v) {
  return v === undefined ? null : v;
}

async function upsertRun(client, run) {
  await client.query(
    // `schema_version`, `split` and `attributable` are covered by `run_id`
    // itself (rows.mjs), which is why `DO NOTHING` is safe for them: a second
    // episode of the same run writes identical values.
    // `kernel_claimed`/`mcp_version`/`tool_allowlist` used to sit here too and
    // were NOT covered — first-writer-wins froze them for every sibling. See
    // `runRowFrom`. `provenance` is the one remaining partial: it embeds
    // `provenance.task`, which `run_id` excludes by design, so this JSONB
    // keeps the FIRST sibling's task block for the run — stated in
    // schema.mjs's own comment rather than glossed as full coverage.
    `INSERT INTO rl_run
       (run_id, schema_version, split, provenance, attributable)
     VALUES ($1,$2,$3,$4,$5)
     ON CONFLICT (run_id) DO NOTHING`,
    [
      run.run_id, nn(run.schema_version), nn(run.split), jsonb(run.provenance), run.attributable,
    ],
  );
}

/**
 * Deduplicated analytical rollups of identity fields already carried
 * (redundantly, as JSONB) inside `rl_run.provenance`. Populated only when
 * the corresponding identity is a REAL descriptor rather than a stated
 * absence (`{absent: "..."}`) — an absent kernel/policy/task has nothing to
 * dimension, and inventing a row for it would be exactly the kind of
 * default-to-a-guess this project's identity rule forbids.
 */
async function upsertDimensions(client, provenance) {
  const kernel = provenance?.kernel;
  if (kernel && typeof kernel === "object" && typeof kernel.sha === "string") {
    await client.query(
      `INSERT INTO rl_kernel_build (sha, dirty, reported_by)
       VALUES ($1,$2,$3)
       ON CONFLICT (sha) DO NOTHING`,
      // `dirty` is NULLABLE for a reason (schema.mjs): a trajectory whose
      // kernel stated a sha and no dirty reading has no cleanliness verdict to
      // record, and `kernel.dirty === true` used to write `false` there — a
      // claim of a clean tree nobody made. The stated reason itself is not
      // lost: `rl_run.provenance` carries the whole kernel block, including
      // its `dirty_absent` sentence.
      [kernel.sha, typeof kernel.dirty === "boolean" ? kernel.dirty : null, nn(kernel.reported_by)],
    );
  }

  const policy = provenance?.policy;
  if (policy && typeof policy === "object" && typeof policy.absent !== "string") {
    const policyDigest = digestOf(policy);
    await client.query(
      `INSERT INTO rl_policy (policy_digest, kind, describe)
       VALUES ($1,$2,$3)
       ON CONFLICT (policy_digest) DO NOTHING`,
      [policyDigest, nn(policy.kind), jsonb(policy)],
    );
  }

  const task = provenance?.task;
  if (task && typeof task === "object" && typeof task.digest === "string") {
    if (typeof task.family === "string" && task.family !== "") {
      await client.query(
        `INSERT INTO rl_task_family (name) VALUES ($1) ON CONFLICT (name) DO NOTHING`,
        [task.family],
      );
    }
    await client.query(
      `INSERT INTO rl_task (task_digest, task_id, family)
       VALUES ($1,$2,$3)
       ON CONFLICT (task_digest) DO NOTHING`,
      [task.digest, nn(task.id), nn(task.family)],
    );
  }
}

async function upsertEpisode(client, episode) {
  await client.query(
    `INSERT INTO rl_episode
       (episode_id, run_id, path, task_id, seed, started_at, outcome, attributable,
        reward_final, tokens, wall_ms, error, model_scope, source_digest, ingested_at)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14, now())
     ON CONFLICT (episode_id) DO UPDATE SET
       run_id = EXCLUDED.run_id,
       path = EXCLUDED.path,
       task_id = EXCLUDED.task_id,
       seed = EXCLUDED.seed,
       started_at = EXCLUDED.started_at,
       outcome = EXCLUDED.outcome,
       attributable = EXCLUDED.attributable,
       reward_final = EXCLUDED.reward_final,
       tokens = EXCLUDED.tokens,
       wall_ms = EXCLUDED.wall_ms,
       error = EXCLUDED.error,
       model_scope = EXCLUDED.model_scope,
       source_digest = EXCLUDED.source_digest,
       ingested_at = now()`,
    [
      episode.episode_id, episode.run_id, nn(episode.path), episode.task_id, nn(episode.seed),
      nn(episode.started_at), episode.outcome, episode.attributable, jsonb(episode.reward_final),
      nn(episode.tokens), nn(episode.wall_ms), nn(episode.error), jsonb(episode.model_scope),
      episode.source_digest,
    ],
  );
}

async function replaceSteps(client, episodeId, steps) {
  await client.query(`DELETE FROM rl_step WHERE episode_id = $1`, [episodeId]);
  for (const s of steps) {
    await client.query(
      `INSERT INTO rl_step (episode_id, step_index, tool, args, result_digest, reward, duration_ms, gate_preflight)
       VALUES ($1,$2,$3,$4,$5,$6,$7,$8)`,
      [episodeId, s.index, nn(s.tool), jsonb(s.args), nn(s.result_digest), jsonb(s.reward), nn(s.duration_ms), nn(s.gate_preflight)],
    );
  }
}

async function replaceRefusals(client, episodeId, refusals) {
  await client.query(`DELETE FROM rl_refusal WHERE episode_id = $1`, [episodeId]);
  for (const r of refusals) {
    await client.query(
      `INSERT INTO rl_refusal (episode_id, step_index, gate, reason)
       VALUES ($1,$2,$3,$4)`,
      [episodeId, r.step_index, nn(r.gate), nn(r.reason)],
    );
  }
}

/** Item 1b (audit S4) — same shape and same idempotency guard as `replaceRefusals` above. */
async function replaceGatePreflightGaps(client, episodeId, gaps) {
  await client.query(`DELETE FROM rl_gate_preflight_gap WHERE episode_id = $1`, [episodeId]);
  for (const g of gaps) {
    await client.query(
      `INSERT INTO rl_gate_preflight_gap (episode_id, step_index, ref, stage, reason)
       VALUES ($1,$2,$3,$4,$5)`,
      [episodeId, g.step_index, nn(g.ref), nn(g.stage), nn(g.reason)],
    );
  }
}

async function replaceClaims(client, episodeId, claims) {
  await client.query(`DELETE FROM rl_claim_result WHERE episode_id = $1`, [episodeId]);
  for (const c of claims) {
    await client.query(
      `INSERT INTO rl_claim_result (episode_id, name, verified, expected, computed, abs_error, tolerance_used, absent)
       VALUES ($1,$2,$3,$4,$5,$6,$7,$8)`,
      [episodeId, c.name, nn(c.verified), nn(c.expected), nn(c.computed), nn(c.abs_error), nn(c.tolerance_used), nn(c.absent)],
    );
  }
}

/**
 * `rl_recipe` and `rl_certificate` belong to the ONE-ROW-PER-EPISODE family
 * (see this file's own top docstring and schema.mjs's): the write itself is
 * `INSERT ... ON CONFLICT (episode_id) DO UPDATE`, exactly like
 * `upsertEpisode` — not the many-rows delete-then-insert pattern. A DELETE
 * is still issued, but only to cover the one thing an upsert cannot do:
 * make a row STOP existing. `recipe` can be `null` on a re-ingest of an
 * episode_id that was previously COMPLETED (recipe present) if the same
 * identity now resolves to a non-COMPLETED outcome, and a present recipe's
 * `certificate_summary` can independently disappear between two ingests —
 * both are edge cases in practice (episode_id is content-addressed), but a
 * stale row surviving a state that no longer produces one is exactly the
 * kind of silent lie this store exists to refuse.
 */
async function upsertRecipe(client, episodeId, recipe) {
  if (!recipe) {
    await client.query(`DELETE FROM rl_recipe WHERE episode_id = $1`, [episodeId]);
    await client.query(`DELETE FROM rl_certificate WHERE episode_id = $1`, [episodeId]);
    return;
  }

  await client.query(
    `INSERT INTO rl_recipe
       (episode_id, retrieved_by, reference, source, step_count, sequence_range,
        sequence_contiguous, undecodable_events, checkpoints, certificate_summary, note, absent)
     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
     ON CONFLICT (episode_id) DO UPDATE SET
       retrieved_by = EXCLUDED.retrieved_by,
       reference = EXCLUDED.reference,
       source = EXCLUDED.source,
       step_count = EXCLUDED.step_count,
       sequence_range = EXCLUDED.sequence_range,
       sequence_contiguous = EXCLUDED.sequence_contiguous,
       undecodable_events = EXCLUDED.undecodable_events,
       checkpoints = EXCLUDED.checkpoints,
       certificate_summary = EXCLUDED.certificate_summary,
       note = EXCLUDED.note,
       absent = EXCLUDED.absent`,
    [
      episodeId, nn(recipe.retrieved_by), nn(recipe.reference), jsonb(recipe.source),
      nn(recipe.step_count), jsonb(recipe.sequence_range), nn(recipe.sequence_contiguous),
      nn(recipe.undecodable_events), jsonb(recipe.checkpoints), jsonb(recipe.certificate_summary),
      nn(recipe.note), nn(recipe.absent),
    ],
  );

  const cert = recipe.certificate_summary;
  if (cert && typeof cert === "object") {
    await client.query(
      `INSERT INTO rl_certificate
         (episode_id, steps_total, steps_with_recorded_certificate, sound, unsound, indeterminate, last_certified_sequence, note)
       VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
       ON CONFLICT (episode_id) DO UPDATE SET
         steps_total = EXCLUDED.steps_total,
         steps_with_recorded_certificate = EXCLUDED.steps_with_recorded_certificate,
         sound = EXCLUDED.sound,
         unsound = EXCLUDED.unsound,
         indeterminate = EXCLUDED.indeterminate,
         last_certified_sequence = EXCLUDED.last_certified_sequence,
         note = EXCLUDED.note`,
      [
        episodeId, nn(cert.steps_total), nn(cert.steps_with_recorded_certificate), nn(cert.sound),
        nn(cert.unsound), nn(cert.indeterminate), nn(cert.last_certified_sequence), nn(cert.note),
      ],
    );
  } else {
    // This recipe no longer carries a certificate_summary (or never did) —
    // a stale one from a prior ingest of this same episode_id must not
    // survive, or a reader would see a certificate that no longer applies.
    await client.query(`DELETE FROM rl_certificate WHERE episode_id = $1`, [episodeId]);
  }
}

async function replaceRecipeSteps(client, episodeId, recipeSteps) {
  await client.query(`DELETE FROM rl_recipe_step WHERE episode_id = $1`, [episodeId]);
  for (const st of recipeSteps) {
    await client.query(
      `INSERT INTO rl_recipe_step
         (episode_id, sequence, op_kind, params, inputs, outputs, intent,
          checkpoint, checkpoint_absent_reason, reissue, reissue_absent_reason)
       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)`,
      [
        episodeId, st.sequence, nn(st.op_kind), jsonb(st.params), jsonb(st.inputs), jsonb(st.outputs),
        nn(st.intent), jsonb(st.checkpoint), nn(st.checkpoint_absent_reason), jsonb(st.reissue),
        nn(st.reissue_absent_reason),
      ],
    );
  }
}

async function replaceSolids(client, episodeId, solids) {
  await client.query(`DELETE FROM rl_solid WHERE episode_id = $1`, [episodeId]);
  for (const s of solids) {
    await client.query(
      `INSERT INTO rl_solid (episode_id, token, produced_by_sequence, op_kind)
       VALUES ($1,$2,$3,$4)`,
      [episodeId, s.token, nn(s.produced_by_sequence), nn(s.op_kind)],
    );
  }
}

async function replaceLineageEdges(client, episodeId, edges) {
  await client.query(`DELETE FROM rl_lineage_edge WHERE episode_id = $1`, [episodeId]);
  for (const e of edges) {
    await client.query(
      `INSERT INTO rl_lineage_edge (episode_id, from_token, to_token, via_sequence, op_kind)
       VALUES ($1,$2,$3,$4,$5)`,
      [episodeId, e.from, e.to, nn(e.via_sequence), nn(e.op_kind)],
    );
  }
}

async function upsertQuarantine(client, path, reason, sourceDigest) {
  await client.query(
    `INSERT INTO rl_quarantine (path, reason, source_digest, ingested_at)
     VALUES ($1,$2,$3, now())
     ON CONFLICT (path) DO UPDATE SET
       reason = EXCLUDED.reason,
       source_digest = EXCLUDED.source_digest,
       ingested_at = now()`,
    [path, reason, sourceDigest],
  );
}

/**
 * Ingest one JSONL file. JSONL is the source of truth (the run loop never
 * depends on this): this function only ever reads a file already on disk
 * and never mutates it. Wrapped in one transaction — a crash mid-write
 * rolls back to the previous state of this one episode (or quarantine
 * entry), never a half-written row set.
 */
export async function ingestFile(client, path) {
  const text = readFileSync(path, "utf8");
  const sourceDigest = sha256Hex(text);
  const rows = rowsFromTrajectory(text, { path });

  await client.query("BEGIN");
  try {
    if (rows.quarantine) {
      const reason = rows.quarantine[0].reason;
      await upsertQuarantine(client, path, reason, sourceDigest);
      await client.query("COMMIT");
      return { path, status: "quarantined", reason };
    }

    // A path that was ONCE quarantined and is now ingesting successfully
    // must not leave its stale rl_quarantine row behind: verify() reads
    // rl_quarantine's own recorded path/source_digest independently of
    // rl_episode, and a leftover row there would report this now-good file
    // as drifted against a digest from the OLD, malformed content — a false
    // alarm that trains an operator to ignore verify() entirely. Cleared in
    // the SAME transaction as the episode write so the two tables can never
    // disagree about this path's current state.
    await client.query(`DELETE FROM rl_quarantine WHERE path = $1`, [path]);

    await upsertRun(client, rows.run);
    await upsertDimensions(client, rows.run.provenance);
    await upsertEpisode(client, rows.episode);
    await replaceSteps(client, rows.episode.episode_id, rows.steps);
    await replaceGatePreflightGaps(client, rows.episode.episode_id, rows.gatePreflightGaps);
    await replaceRefusals(client, rows.episode.episode_id, rows.refusals);
    await replaceClaims(client, rows.episode.episode_id, rows.claims);
    await upsertRecipe(client, rows.episode.episode_id, rows.recipe);
    await replaceRecipeSteps(client, rows.episode.episode_id, rows.recipeSteps);
    await replaceSolids(client, rows.episode.episode_id, rows.solids);
    await replaceLineageEdges(client, rows.episode.episode_id, rows.lineageEdges);
    await client.query("COMMIT");
    return { path, status: "ingested", episode_id: rows.episode.episode_id, run_id: rows.run.run_id };
  } catch (e) {
    await client.query("ROLLBACK");
    throw e;
  }
}

/** Ingest every `.jsonl` file directly inside `dir`, in name order, one at a time (foreground, no concurrency — this is an offline loader, not a service). */
export async function ingestDir(client, dir) {
  const files = readdirSync(dir).filter((f) => f.endsWith(".jsonl")).sort();
  const results = [];
  for (const f of files) {
    results.push(await ingestFile(client, join(dir, f)));
  }
  return results;
}

/**
 * Re-read every file this store has a recorded path for (episodes and
 * quarantine entries alike) and compare its current bytes against the
 * digest recorded at ingest time. JSONL is the source of truth: if a file
 * changed or vanished on disk since it was ingested, the DATABASE is now
 * the stale copy, and `verify` exists to say so rather than let a query
 * silently trust rows nothing revalidated.
 */
export async function verify(client) {
  const episodes = await client.query(
    `SELECT path, source_digest FROM rl_episode WHERE path IS NOT NULL`,
  );
  const quarantined = await client.query(
    `SELECT path, source_digest FROM rl_quarantine WHERE path IS NOT NULL`,
  );
  const rows = [...episodes.rows, ...quarantined.rows];

  let checked = 0;
  const drifted = [];
  for (const row of rows) {
    checked += 1;
    let text;
    try {
      text = readFileSync(row.path, "utf8");
    } catch (e) {
      drifted.push({ path: row.path, reason: `the file could not be read at its recorded path: ${e?.message ?? e}` });
      continue;
    }
    const digestNow = sha256Hex(text);
    if (digestNow !== row.source_digest) {
      drifted.push({
        path: row.path,
        reason: `the file's bytes changed since it was ingested (recorded ${row.source_digest}, now ${digestNow})`,
      });
    }
  }
  return { checked, drifted };
}
