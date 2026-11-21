#!/usr/bin/env node
// Agent eval: Claude Code (headless, on the user's subscription -- no API
// key) builds each task's part through the Roshera MCP tools; the harness
// then checks the result through the REST API, independently of anything the
// agent said, and keeps every step of the agent's stream for the failure
// classifier (classify.mjs).
//
// One episode at a time (memory and the house rule). Each episode gets its
// own document and part model, pinned into the MCP server's environment
// (ROSHERA_DOCUMENT / ROSHERA_PART), so episodes cannot see each other's
// solids. ANTHROPIC_* is removed from the agent's environment so the run can
// only ever use the logged-in subscription.
//
// usage: node agent-eval/run.mjs [--model sonnet] [--out runs/agent-eval]
//        [--only id,id] [--repeats 1] [--timeout-min 20] [--label baseline]

import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { TASKS, promptFor } from "./tasks.mjs";
import { api, freshEpisodeScope, parseArgs, runClaude } from "./common.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const args = parseArgs(process.argv.slice(2));
const MODEL = args.model ?? "sonnet";
const LABEL = args.label ?? "baseline";
const OUT = resolve(args.out ?? join(here, "..", "runs", "agent-eval", LABEL));
const ONLY = args.only ? new Set(args.only.split(",")) : null;
const REPEATS = Number(args.repeats ?? 1);
const TIMEOUT_MS = Number(args["timeout-min"] ?? 20) * 60_000;

/** Run one episode; resolves to its record. */
async function episode(task, rep) {
  const name = `agent-eval:${LABEL}:${task.id}:${rep}`;
  const scope = await freshEpisodeScope(name);
  if (scope.error) {
    return { task: task.id, rep, outcome: "SETUP_FAILED", detail: scope.error };
  }
  const { documentId, partId } = scope;
  const work = join(tmpdir(), `roshera-agent-eval-${process.pid}-${task.id}-${rep}`);
  mkdirSync(OUT, { recursive: true });
  const streamPath = join(OUT, `${task.id}-${rep}.jsonl`);
  const started = Date.now();
  const exit = await runClaude({
    prompt: promptFor(task),
    model: MODEL,
    work,
    streamPath,
    documentId,
    partId,
    timeoutMs: TIMEOUT_MS,
  });
  const seconds = Math.round((Date.now() - started) / 1000);
  const check = await checkResult(task, partId);
  rmSync(work, { recursive: true, force: true });
  return {
    task: task.id,
    rep,
    model: MODEL,
    label: LABEL,
    seconds,
    exit,
    stream: streamPath,
    documentId,
    partId,
    ...check,
  };
}

/** The harness's own verdict on what the agent left in its part model. */
async function checkResult(task, partId) {
  const headers = { "X-Roshera-Part-Id": partId };
  const list = await api("GET", "/api/agent/parts", undefined, headers);
  const solids = Array.isArray(list.json) ? list.json : list.json?.parts ?? [];
  const result = { solids: solids.length, checks: {} };
  if (solids.length !== 1) {
    result.passed = false;
    result.checks.one_solid = false;
    return result;
  }
  result.checks.one_solid = true;
  const id = solids[0].id;
  const perception = await api("GET", `/api/agent/parts/${id}/perception`, undefined, headers);
  const p = perception.json ?? {};
  result.checks.sound = p.sound === true;
  const volume = p.volume ?? p.perception?.volume ?? null;
  const tol = task.volumeTol ?? 0.005;
  result.volume = volume;
  result.checks.volume =
    typeof volume === "number" && Math.abs(volume - task.volume) <= tol * task.volume;
  const dims = p.dims ?? p.perception?.dims ?? null;
  const ext = Array.isArray(dims)
    ? dims
    : dims && typeof dims === "object"
      ? [dims.x ?? dims.dx, dims.y ?? dims.dy, dims.z ?? dims.dz]
      : null;
  result.extents = ext;
  const sorted = (a) => [...a].map(Number).sort((x, y) => y - x);
  const near = (a, b) => a.length === b.length && a.every((v, i) => Math.abs(v - b[i]) <= 0.01);
  const wants = task.extents ? [task.extents] : (task.extentsAnyOf ?? []);
  result.checks.extents =
    Array.isArray(ext) && ext.every((v) => typeof v === "number")
      ? wants.some((w) => near(sorted(ext), sorted(w)))
      : null;
  result.passed = Object.values(result.checks).every((v) => v === true);
  return result;
}

const results = [];
for (const task of TASKS) {
  if (ONLY && !ONLY.has(task.id)) continue;
  for (let rep = 0; rep < REPEATS; rep++) {
    process.stdout.write(`[agent-eval] ${task.id} #${rep} ... `);
    const r = await episode(task, rep);
    results.push(r);
    process.stdout.write(`${r.passed ? "PASS" : "FAIL"} (${r.seconds ?? "-"} s) ${JSON.stringify(r.checks ?? {})}\n`);
    writeFileSync(join(OUT, "results.json"), JSON.stringify(results, null, 2));
  }
}
const passed = results.filter((r) => r.passed).length;
console.log(`[agent-eval] ${LABEL}: ${passed}/${results.length} passed -> ${join(OUT, "results.json")}`);
