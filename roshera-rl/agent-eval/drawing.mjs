#!/usr/bin/env node
// Drawing agent mode (spec docs/research/2026-11-20-agent-eyes-on-drawings-design.md,
// slice 4): an agent improves the automatic sheet of a drawing-bench part
// with the layout verbs, and the harness scores the sheet before and after
// from the KERNEL, never from the agent's words.
//
// Per part, one episode at a time:
//   1. a fresh document and part model; the bench STEP imported into it;
//   2. the automatic sheet (POST /api/parts/{id}/drawing);
//   3. BEFORE: the space map, the certificate, the whole-sheet PNG;
//   4. headless Claude Code (subscription, ANTHROPIC_* scrubbed) runs the
//      manufacturing_drawing playbook's IMPROVE LOOP on that drawing;
//      while a recorder (show_recorder.mjs) keeps every look / act / judge
//      frame the app is shown, in order, as the episode's replay;
//   5. AFTER: the same three readings;
//   6. a roshera-rl trajectory (header, one step per tool call, terminal with
//      the named reward vector) and a row in results.json.
//
// Held-out and NIST parts are reported separately and never tuned on.
//
// usage: node agent-eval/drawing.mjs [--model opus] [--label slice4]
//        [--dev 5] [--held-out 3] [--nist 2] [--only id,id] [--timeout-min 25]

import { mkdirSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { api, apiBytes, AUTH_HEADER, BASE, freshEpisodeScope, parseArgs, runClaude, scopeHeaders } from "./common.mjs";
import { drawingParts, drawingPrompt, SELECTION_RULE } from "./drawing_tasks.mjs";
import { drawingOutcome, drawingReward, stepsFromStream } from "./drawing_score.mjs";
import { recordShow } from "./show_recorder.mjs";
import { openTrajectory } from "../lib/trajectory.mjs";
import { digestOf, resolveKernelIdentity } from "../lib/provenance.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv.slice(2));
const MODEL = args.model ?? "opus";
const LABEL = args.label ?? "drawing";
const OUT = resolve(args.out ?? join(here, "..", "runs", "agent-eval-drawing", LABEL));
const ONLY = args.only ? new Set(args.only.split(",")) : null;
const TIMEOUT_MS = Number(args["timeout-min"] ?? 25) * 60_000;
const PARTS = drawingParts({
  dev: Number(args.dev ?? 5),
  heldOut: Number(args["held-out"] ?? 3),
  nist: Number(args.nist ?? 2),
}).filter((p) => !ONLY || ONLY.has(p.id));

/** The sheet's three kernel readings: space map, certificate, whole-sheet PNG. */
async function readSheet(drawingId, headers, pngPath) {
  const spaces = await api("GET", `/api/drawings/${drawingId}/spaces`, undefined, headers);
  const cert = await api("GET", `/api/drawings/${drawingId}/certificate`, undefined, headers);
  const png = await apiBytes(`/api/drawings/${drawingId}/render.png?px=2400`, headers);
  if (png.status === 200) writeFileSync(pngPath, png.bytes);
  const map = spaces.status === 200 ? spaces.json : null;
  return {
    spaces: map
      ? { grade: map.grade, bar: map.bar, below_bar: map.below_bar, worst: map.worst, by_rule: map.by_rule }
      : null,
    spaces_error: map ? null : { status: spaces.status, body: spaces.json },
    certificate: cert.status === 200 ? { sound: cert.json?.sound, complete: cert.json?.complete, counts: cert.json?.counts } : null,
    certificate_error: cert.status === 200 ? null : { status: cert.status, body: cert.json },
    png: png.status === 200 ? pngPath : null,
  };
}

/** Import the part and make its automatic sheet. -> { solidId, drawingId } | { error } */
async function setUp(part, headers) {
  const imported = await api("POST", "/api/geometry/import_step", { path: part.step, content: null, name: part.id }, headers);
  const objects = imported.json?.objects ?? [];
  if (imported.status !== 200 || objects.length === 0) {
    return { error: { stage: "import", status: imported.status, body: imported.json } };
  }
  const solidId = objects[0].solid_id;
  const made = await api("POST", `/api/parts/${solidId}/drawing?name=${encodeURIComponent(part.id)}`, undefined, headers);
  if (made.status !== 200 || !made.json?.id) {
    return { error: { stage: "drawing", status: made.status, body: made.json } };
  }
  return { solidId, drawingId: made.json.id, solids: objects.length };
}

async function episode(part, kernel) {
  const folder = join(OUT, part.id);
  mkdirSync(folder, { recursive: true });
  const trajectoryPath = join(folder, "trajectory.jsonl");
  const traj = openTrajectory({
    path: trajectoryPath,
    taskId: `drawing-improve:${part.id}`,
    seed: null,
    kernelSha: kernel.sha ?? null,
    mcpVersion: null,
    toolAllowlist: ["mcp__roshera"],
    split: part.split,
    provenance: {
      kernel,
      policy: { kind: "claude-code-headless", model: MODEL, billing: "subscription" },
      task: { part: part.id, step: part.step, selection_rule: SELECTION_RULE },
    },
  });
  const started = Date.now();
  const scope = await freshEpisodeScope(`agent-eval-drawing:${LABEL}:${part.id}`);
  if (scope.error) {
    traj.close({ outcome: "SETUP_FAILED", rewardFinal: {}, claims: [], wallMs: Date.now() - started, error: scope.error });
    return { id: part.id, split: part.split, outcome: "SETUP_FAILED", error: scope.error };
  }
  const headers = scopeHeaders(scope);
  const made = await setUp(part, headers);
  if (made.error) {
    traj.close({ outcome: "SETUP_FAILED", rewardFinal: {}, claims: [], wallMs: Date.now() - started, error: made.error });
    return { id: part.id, split: part.split, outcome: "SETUP_FAILED", error: made.error };
  }
  const before = await readSheet(made.drawingId, headers, join(folder, "before.png"));
  const streamPath = join(folder, "stream.jsonl");
  const work = join(tmpdir(), `roshera-agent-eval-drawing-${process.pid}-${part.id}`);
  const recorder = await recordShow(made.drawingId, join(folder, "show.jsonl"));
  const exit = await runClaude({
    prompt: drawingPrompt({ drawingId: made.drawingId, partId: made.solidId }),
    model: MODEL,
    work,
    streamPath,
    documentId: scope.documentId,
    partId: scope.partId,
    timeoutMs: TIMEOUT_MS,
  });
  const show = await recorder.stop();
  rmSync(work, { recursive: true, force: true });
  const after = await readSheet(made.drawingId, headers, join(folder, "after.png"));
  const steps = stepsFromStream(readFileSync(streamPath, "utf8"));
  for (const s of steps) {
    traj.step({
      i: s.i,
      action: { tool: s.tool, args: s.args },
      resultDigest: digestOf({ tool: s.tool, ok: s.ok, refusal: s.refusal }),
      reward: { answered: s.ok !== null, ok: s.ok },
      refusal: s.refusal,
      ms: null,
    });
  }
  const reward = drawingReward({
    before: before.spaces,
    after: after.spaces,
    certificateAfter: after.certificate,
    steps,
  });
  const outcome = drawingOutcome(exit);
  const finalText = readFileSync(streamPath, "utf8")
    .split(/\r?\n/)
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter((e) => e?.type === "result")
    .map((e) => String(e.result ?? ""))
    .pop() ?? null;
  traj.close({
    outcome,
    rewardFinal: reward,
    // A drawing task declares no geometric claims: the sheet certificate is
    // its verdict, carried in the reward. Stated, never an empty "unchecked".
    claims: { absent: "a drawing-improvement task declares no geometric claims; the sheet certificate is the verdict" },
    recipeRef: { absent: "the episode's document is kept for replay; recipe export is not wired for drawing edits" },
    wallMs: Date.now() - started,
    error: exit.code === 0 ? null : { exit_code: exit.code, signal: exit.signal, stderr: exit.stderr },
  });
  const row = {
    id: part.id,
    split: part.split,
    model: MODEL,
    outcome,
    seconds: Math.round((Date.now() - started) / 1000),
    document_id: scope.documentId,
    part_id: scope.partId,
    drawing_id: made.drawingId,
    before,
    after,
    reward,
    agent_report: finalText,
    // The replay: every look / act / judge frame the app was shown, in order.
    show: { path: join(folder, "show.jsonl"), ...show },
    trajectory: trajectoryPath,
    stream: streamPath,
  };
  writeFileSync(join(folder, "row.json"), JSON.stringify(row, null, 1));
  return row;
}

/** Grade and below-bar before -> after, per split: the headline, never averaged across parts. */
function summary(rows) {
  const out = {};
  for (const split of ["dev", "held_out", "nist"]) {
    const sub = rows.filter((r) => r.split === split);
    if (!sub.length) continue;
    out[split] = sub.map((r) => ({
      id: r.id,
      outcome: r.outcome,
      grade: `${r.reward?.grade_before ?? "-"} -> ${r.reward?.grade_after ?? "-"}`,
      below_bar: `${r.reward?.below_bar_before ?? "-"} -> ${r.reward?.below_bar_after ?? "-"}`,
      sound_after: r.reward?.sound_after ?? null,
      verbs: `${r.reward?.verbs_applied ?? 0} applied, ${r.reward?.verbs_refused ?? 0} refused`,
    }));
  }
  return out;
}

mkdirSync(OUT, { recursive: true });
const kernel = await resolveKernelIdentity({ baseUrl: BASE, authHeader: AUTH_HEADER });
const rows = [];
for (const part of PARTS) {
  process.stdout.write(`[drawing] ${part.split} ${part.id} ... `);
  const r = await episode(part, kernel);
  rows.push(r);
  process.stdout.write(
    `${r.outcome} grade ${r.reward?.grade_before ?? "-"} -> ${r.reward?.grade_after ?? "-"}, ` +
      `below bar ${r.reward?.below_bar_before ?? "-"} -> ${r.reward?.below_bar_after ?? "-"}, ` +
      `sound ${r.reward?.sound_after ?? "-"} (${r.seconds ?? "-"} s)\n`,
  );
  writeFileSync(
    join(OUT, "results.json"),
    JSON.stringify({ label: LABEL, model: MODEL, selection_rule: SELECTION_RULE, summary: summary(rows), rows }, null, 1),
  );
}
console.log(JSON.stringify(summary(rows), null, 1));
