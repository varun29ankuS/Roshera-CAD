// What every agent-eval mode shares: the REST client the harness checks
// results through, the agent's scrubbed environment, and one headless
// Claude Code run against a per-episode MCP server.
//
// Claude Code runs on the user's subscription (no API key): ANTHROPIC_* and
// CLAUDE_CODE_USE_* are removed from the agent's environment, so a run can
// only ever use the logged-in subscription. Each episode's MCP server is
// pinned to its own document and part model (ROSHERA_DOCUMENT /
// ROSHERA_PART), so episodes cannot see each other's solids.

import { spawn } from "node:child_process";
import { readFileSync, writeFileSync, createWriteStream, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
export const BASE = process.env.ROSHERA_URL ?? "http://127.0.0.1:8081";
export const MCP_ENTRY = resolve(here, "..", "..", "roshera-mcp", "dist", "index.js");

const KEY = readFileSync(join(homedir(), ".roshera", "api_key"), "utf8").trim();
const AUTH = { Authorization: `ApiKey ${KEY}` };
/** The harness's own credential header (for roshera-rl's /health identity read). */
export const AUTH_HEADER = Object.freeze({ ...AUTH });

/** One REST call; never throws on an HTTP error status. -> { status, json } */
export async function api(method, path, body, headers = {}) {
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

/** One REST call returning raw bytes (a PNG). -> { status, bytes } */
export async function apiBytes(path, headers = {}) {
  const res = await fetch(`${BASE}${path}`, { headers: { ...AUTH, ...headers } });
  return { status: res.status, bytes: Buffer.from(await res.arrayBuffer()) };
}

export function agentEnv() {
  const env = { ...process.env };
  for (const k of Object.keys(env)) {
    if (k.startsWith("ANTHROPIC_") || k.startsWith("CLAUDE_CODE_USE_")) delete env[k];
  }
  return env;
}

/** A fresh document and part model for one episode. -> { documentId, partId } | { error } */
export async function freshEpisodeScope(name) {
  const doc = await api("POST", "/api/documents", { name });
  const part = await api("POST", "/api/parts", { name });
  const documentId = doc.json?.id;
  const partId = part.json?.id;
  if (!documentId || !partId) return { error: { doc, part } };
  return { documentId, partId };
}

/** The headers that scope a harness REST call to the episode's own model. */
export function scopeHeaders({ documentId, partId }) {
  return { "X-Roshera-Document": documentId, "X-Roshera-Part-Id": partId };
}

/**
 * Run headless Claude Code once with only the roshera MCP tools, streaming
 * its stream-json to `streamPath`. -> { code, signal, stderr, timedOut }
 */
export async function runClaude({ prompt, model, work, streamPath, documentId, partId, timeoutMs }) {
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
  const stream = createWriteStream(streamPath);
  const claudeArgs = [
    "-p",
    prompt,
    "--model",
    model,
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
    let timedOut = false;
    child.stdout.pipe(stream);
    child.stderr.on("data", (d) => (stderr += d));
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill();
    }, timeoutMs);
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      done({ code, signal, stderr: stderr.slice(-2000), timedOut });
    });
  });
  await new Promise((r) => stream.end(r));
  return exit;
}

/** Parse `--a b --flag` argv into an object (a bare flag reads "true"). */
export function parseArgs(argv) {
  return Object.fromEntries(
    argv.reduce((acc, a, i, all) => {
      if (a.startsWith("--")) acc.push([a.slice(2), all[i + 1]?.startsWith("--") ? "true" : all[i + 1]]);
      return acc;
    }, []),
  );
}
