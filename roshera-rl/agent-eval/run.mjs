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

import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, createWriteStream, rmSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { TASKS, promptFor } from "./tasks.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => {
    if (a.startsWith("--")) acc.push([a.slice(2), all[i + 1]?.startsWith("--") ? "true" : all[i + 1]]);
    return acc;
  }, []),
);
const MODEL = args.model ?? "sonnet";
const LABEL = args.label ?? "baseline";
const OUT = resolve(args.out ?? join(here, "..", "runs", "agent-eval", LABEL));
const ONLY = args.only ? new Set(args.only.split(",")) : null;
const REPEATS = Number(args.repeats ?? 1);
const TIMEOUT_MS = Number(args["timeout-min"] ?? 20) * 60_000;
const BASE = process.env.ROSHERA_URL ?? "http://127.0.0.1:8081";
const MCP_ENTRY = resolve(here, "..", "..", "roshera-mcp", "dist", "index.js");

const KEY = readFileSync(join(homedir(), ".roshera", "api_key"), "utf8").trim();
const AUTH = { Authorization: `ApiKey ${KEY}` };

async function api(method, path, body, headers = {}) {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: { "Content-Type": "application/json", ...AUTH, ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = { raw: text };
  }
  return { status: res.status, json };
}

function agentEnv() {
  const env = { ...process.env };
  for (const k of Object.keys(env)) {
    if (k.startsWith("ANTHROPIC_") || k.startsWith("CLAUDE_CODE_USE_")) delete env[k];
  }
  return env;
}

/** Run one episode; resolves to its record. */
async function episode(task, rep) {
  const name = `agent-eval:${LABEL}:${task.id}:${rep}`;
  const doc = await api("POST", "/api/documents", { name });
  const part = await api("POST", "/api/parts", { name });
  const documentId = doc.json?.id;
  const partId = part.json?.id;
  if (!documentId || !partId) {
    return { task: task.id, rep, outcome: "SETUP_FAILED", detail: { doc, part } };
  }
  const work = join(tmpdir(), `roshera-agent-eval-${process.pid}-${task.id}-${rep}`);
  mkdirSync(work, { recursive: true });
  const mcpConfig = join(work, "mcp.json");
  writeFileSync(
    mcpConfig,
    JSON.stringify({
      mcpServers: {
        roshera: {
          command: "node",
          args: [MCP_ENTRY],
          // No secret here: the MCP server reads ~/.roshera/api_key itself.
          env: {
            ROSHERA_URL: BASE,
            ROSHERA_DOCUMENT: documentId,
            ROSHERA_PART: partId,
            ROSHERA_AMBIENT_PERCEPTION: "cert",
          },
        },
      },
    }),
  );
  mkdirSync(OUT, { recursive: true });
  const streamPath = join(OUT, `${task.id}-${rep}.jsonl`);
  const stream = createWriteStream(streamPath);
  const started = Date.now();
  const claudeArgs = [
    "-p",
    promptFor(task),
    "--model",
    MODEL,
    "--output-format",
    "stream-json",
    "--verbose",
    "--mcp-config",
    mcpConfig,
    "--strict-mcp-config",
    "--tools",
    "",
    "--allowedTools",
    "mcp__roshera",
    "--no-session-persistence",
  ];
  const exit = await new Promise((done) => {
    const child = spawn("claude", claudeArgs, {
      cwd: work,
      env: agentEnv(),
      shell: process.platform === "win32",
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stdout.pipe(stream);
    child.stderr.on("data", (d) => (stderr += d));
    const timer = setTimeout(() => child.kill(), TIMEOUT_MS);
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      done({ code, signal, stderr: stderr.slice(-2000) });
    });
  });
  stream.end();
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
