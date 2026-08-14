/**
 * Schema — `ensureSchema(client)`.
 *
 * Every table here is prefixed `rl_`; this package (`roshera-rl`) is their
 * ONLY writer, in the whole system. That single-writer property is what
 * makes it acceptable for a Node package to own a slice of Postgres
 * directly — nothing else reads or writes these tables, so there is no
 * cross-service migration story to coordinate. Follows the pattern already
 * in production at `roshera-backend/session-manager/src/database.rs:409`
 * onward (`CREATE TABLE IF NOT EXISTS` inside a startup routine) rather than
 * introducing a migration tool for a corpus this package alone owns.
 *
 * THE LIMIT OF THAT CHOICE, STATED: `CREATE TABLE IF NOT EXISTS` creates, it
 * never reshapes. A database that already ran an older version of this file
 * keeps any column since removed here — `rl_run.kernel_claimed`,
 * `rl_run.mcp_version` and `rl_run.tool_allowlist` are the three, dropped
 * because they were per-episode facts frozen by the first writer under a
 * run-level name. Nothing writes or reads them any more, so they simply hold
 * whatever they last held. This file deliberately does NOT issue a
 * `DROP COLUMN` to remove them: dropping a column destroys data in a database
 * this package shares with a human operator's dev work, and that is an
 * operator's decision to take deliberately, not a side effect of importing a
 * module. A fresh database never has them at all.
 *
 * Two families of table, deliberately shaped differently:
 *
 *   - ONE ROW PER EPISODE (`rl_episode`, `rl_recipe`, `rl_certificate`):
 *     keyed on `episode_id` itself, upserted with `INSERT ... ON CONFLICT
 *     (episode_id) DO UPDATE` — the literal shape the brief asks for
 *     ("Upsert keyed on (run_id, episode_id) with the source digest").
 *     `episode_id` alone is sufficient as the conflict target: it is
 *     `digestOf({run_id, task_id, seed, started_at})` (rows.mjs), so it
 *     already embeds `run_id` — a composite `(run_id, episode_id)` unique
 *     constraint would be redundant with the one that matters.
 *
 *   - MANY ROWS PER EPISODE (`rl_step`, `rl_refusal`, `rl_claim_result`,
 *     `rl_recipe_step`, `rl_solid`, `rl_lineage_edge`): a surrogate
 *     `BIGSERIAL` primary key and NO uniqueness constraint on the natural
 *     key. Idempotency for these is owned entirely by `store.mjs`'s
 *     `DELETE FROM <table> WHERE episode_id = $1` immediately before the
 *     bulk re-insert, inside the same per-episode transaction. That delete
 *     is deliberately the ONE place idempotency lives for this family —
 *     removing it is the single edit that mutation-proves the property
 *     (see the store's own docstring).
 *
 * `rl_kernel_build`, `rl_policy`, `rl_task`, `rl_task_family` are
 * deduplicated analytical rollups of identity fields that already live
 * (redundantly, as JSONB) inside `rl_run.provenance` — convenient to query
 * without unpacking JSON, not a normalization the fact tables depend on. No
 * foreign key ties them to `rl_run`/`rl_episode`: an unattributable run
 * (absent kernel/policy/task identity — see rows.mjs) has nothing to
 * dimension, and a nullable FK would buy nothing a plain lookup table
 * doesn't already give.
 *
 * `rl_quarantine` is the only table a malformed file ever reaches: rows.mjs
 * never throws and never drops a bad file silently, and neither does this
 * store — a file that could not be parsed is a FACT about the corpus, kept
 * with its reason, keyed on `path` (one quarantine verdict per file).
 */

const STATEMENTS = [
  `CREATE TABLE IF NOT EXISTS rl_task_family (
    name TEXT PRIMARY KEY
  )`,

  `CREATE TABLE IF NOT EXISTS rl_task (
    task_digest TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    family TEXT
  )`,

  `CREATE TABLE IF NOT EXISTS rl_policy (
    policy_digest TEXT PRIMARY KEY,
    kind TEXT,
    describe JSONB NOT NULL
  )`,

  `CREATE TABLE IF NOT EXISTS rl_kernel_build (
    sha TEXT PRIMARY KEY,
    dirty BOOLEAN,
    reported_by TEXT,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
  )`,

  // `run_id` is a digest over {schema_version, kernel, mcp, policy, harness,
  // split} (rows.mjs), so `schema_version`, `split` and `attributable` (which
  // is derived from those same four identity dimensions) are identical for
  // every episode of one run by construction — which is what makes
  // `ON CONFLICT (run_id) DO NOTHING` safe rather than lossy for them.
  // `kernel_claimed`, `mcp_version` and `tool_allowlist` were here and were
  // NOT covered: they are per-episode facts, so the first episode ingested
  // froze them for every later sibling. They now live only where they are
  // true — in the trajectory file, and (for mcp_version) inside `provenance`.
  //
  // ONE COLUMN IS NOT FULLY COVERED, AND IT IS STATED RATHER THAN CLAIMED
  // AWAY: `provenance` is stored whole, and it carries `provenance.task`,
  // which `run_id` deliberately EXCLUDES ("that is per-episode by
  // construction", rows.mjs). So under `DO NOTHING` the first sibling's task
  // block is the one this JSONB keeps for the whole run — the same
  // first-writer-wins shape the three dropped columns had, surviving inside
  // the document. It is left as-is on purpose: every episode's own task
  // identity is recorded per-episode in `rl_task`/`rl_episode` and in the
  // trajectory file, so nothing is lost, and changing what `rl_run.provenance`
  // stores is a design decision about the run document rather than a defect
  // fix. Anyone querying `rl_run.provenance -> 'task'` must read it as "one
  // episode's task", never as "this run's task".
  `CREATE TABLE IF NOT EXISTS rl_run (
    run_id TEXT PRIMARY KEY,
    schema_version TEXT,
    split TEXT,
    provenance JSONB,
    attributable BOOLEAN NOT NULL,
    first_ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
  )`,

  `CREATE TABLE IF NOT EXISTS rl_episode (
    episode_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES rl_run(run_id),
    path TEXT,
    task_id TEXT NOT NULL,
    seed DOUBLE PRECISION,
    started_at TIMESTAMPTZ,
    outcome TEXT NOT NULL,
    attributable BOOLEAN NOT NULL,
    reward_final JSONB,
    tokens DOUBLE PRECISION,
    wall_ms DOUBLE PRECISION,
    error TEXT,
    model_scope JSONB,
    source_digest TEXT NOT NULL,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
  )`,
  `CREATE INDEX IF NOT EXISTS rl_episode_run_id_idx ON rl_episode(run_id)`,

  `CREATE TABLE IF NOT EXISTS rl_step (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    step_index INT NOT NULL,
    tool TEXT,
    args JSONB,
    result_digest TEXT,
    reward JSONB,
    duration_ms DOUBLE PRECISION
  )`,
  `CREATE INDEX IF NOT EXISTS rl_step_episode_id_idx ON rl_step(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_refusal (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    step_index INT NOT NULL,
    gate TEXT,
    reason TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS rl_refusal_episode_id_idx ON rl_refusal(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_claim_result (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    verified BOOLEAN,
    expected DOUBLE PRECISION,
    computed DOUBLE PRECISION,
    abs_error DOUBLE PRECISION,
    tolerance_used DOUBLE PRECISION,
    absent TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS rl_claim_result_episode_id_idx ON rl_claim_result(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_recipe (
    episode_id TEXT PRIMARY KEY REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    retrieved_by TEXT,
    reference TEXT,
    source JSONB,
    step_count INT,
    sequence_range JSONB,
    sequence_contiguous BOOLEAN,
    undecodable_events INT,
    checkpoints JSONB,
    certificate_summary JSONB,
    note TEXT,
    absent TEXT
  )`,

  `CREATE TABLE IF NOT EXISTS rl_recipe_step (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    sequence INT NOT NULL,
    op_kind TEXT,
    params JSONB,
    inputs JSONB,
    outputs JSONB,
    intent TEXT,
    checkpoint JSONB,
    checkpoint_absent_reason TEXT,
    reissue JSONB,
    reissue_absent_reason TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS rl_recipe_step_episode_id_idx ON rl_recipe_step(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_solid (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    token TEXT NOT NULL,
    produced_by_sequence INT,
    op_kind TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS rl_solid_episode_id_idx ON rl_solid(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_lineage_edge (
    id BIGSERIAL PRIMARY KEY,
    episode_id TEXT NOT NULL REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    from_token TEXT NOT NULL,
    to_token TEXT NOT NULL,
    via_sequence INT,
    op_kind TEXT
  )`,
  `CREATE INDEX IF NOT EXISTS rl_lineage_edge_episode_id_idx ON rl_lineage_edge(episode_id)`,

  `CREATE TABLE IF NOT EXISTS rl_certificate (
    episode_id TEXT PRIMARY KEY REFERENCES rl_episode(episode_id) ON DELETE CASCADE,
    steps_total INT,
    steps_with_recorded_certificate INT,
    sound INT,
    unsound INT,
    indeterminate INT,
    last_certified_sequence INT,
    note TEXT
  )`,

  `CREATE TABLE IF NOT EXISTS rl_quarantine (
    path TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    source_digest TEXT,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT now()
  )`,
];

/** Create every `rl_` table (and its indexes) if it does not already exist. */
export async function ensureSchema(client) {
  for (const statement of STATEMENTS) {
    await client.query(statement);
  }
}
