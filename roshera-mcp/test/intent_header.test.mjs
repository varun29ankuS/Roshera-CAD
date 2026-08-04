/**
 * Intent-header proof (2026-08-04) — the wire half of the op→intent link.
 *
 * The intent gate already forces a real checkpoint phrase before any
 * solid-mutating call; this test proves the phrase is no longer thrown
 * away: once an intent is open, EVERY backend call `api()` makes carries
 * `X-Roshera-Intent` (the phrase, URL-encoded — it is free text and may
 * contain non-ASCII) and `X-Roshera-Intent-Turn`; before one is open,
 * neither header exists (absence stays absent on the wire).
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + gates.ts +
 * core.ts api(), compiled from src to test/.build) against a local stub
 * backend that records the headers of every request it receives.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/intent_header.test.mjs
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID = "11111111-2222-4333-8444-555555555555";

// The checkpoint phrase deliberately contains non-ASCII (Ø, —, ×): a raw
// header value with these bytes makes undici's fetch throw, so this also
// proves the URL-encoding is load-bearing, not decorative.
const INTENT_NAME = "Ø160 bolt circle — 8 × M8, close fit";

// ─── Stub backend: records every request's intent headers ──────────────────

/** @type {{ method: string, url: string, intent: string | null, turn: string | null }[]} */
const seen = [];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  seen.push({
    method: req.method ?? "",
    url,
    intent: req.headers["x-roshera-intent"] ?? null,
    turn: req.headers["x-roshera-intent-turn"] ?? null,
  });
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({
        objects: [{ id: UUID, analytical_geometry: { solid_id: 7 } }],
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7/perception") {
      return send({ valid: true, watertight: true, open_edges: 0 });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7") {
      return send({
        id: 7,
        topology: { face_count: 6 },
        volume: 1000,
        location: { center_world: [0, 0, 0], dimensions_world: [1, 1, 1] },
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ id: 7 }]);
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    if (req.method === "POST" && url === "/api/geometry/shell") {
      return send({
        object: { id: UUID },
        solid_id: 7,
        stats: { triangle_count: 12 },
        valid: true,
        watertight: true,
      });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      return send({ id: "cp-1", name: JSON.parse(body || "{}").name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      return send({
        id: "bb-1",
        text: JSON.parse(body || "{}").text,
        author: "agent",
        createdAt: 0,
        updatedAt: 0,
      });
    }
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end("{}");
  });
});

stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;

// BASE is read at core.js module load — set it BEFORE importing anything.
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;

const { buildTable } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);

const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── 1. no open intent → no header, on any call ────────────────────────────

await call("list_parts", {});
check("before any checkpoint, NO request carries an intent header", () => {
  assert.ok(seen.length > 0, "the stub saw the list_parts request");
  for (const r of seen) {
    assert.equal(r.intent, null, `${r.method} ${r.url} must carry no X-Roshera-Intent`);
    assert.equal(r.turn, null, `${r.method} ${r.url} must carry no X-Roshera-Intent-Turn`);
  }
});

// ─── 2. opening the checkpoint itself is still intent-less on the wire ─────

seen.length = 0;
const cp = await call("timeline_checkpoint", { name: INTENT_NAME, branch: "main" });
check("the checkpoint-opening call itself precedes the open intent (no header)", () => {
  assert.notEqual(cp.isError, true, "checkpoint call succeeds");
  const post = seen.find((r) => r.url === "/api/timeline/checkpoint");
  assert.ok(post, "the checkpoint POST reached the backend");
  assert.equal(post.intent, null, "the intent opens only AFTER the checkpoint succeeds");
});

// ─── 3. open intent → every backend call carries it, URL-encoded ───────────

seen.length = 0;
const shellResult = await call("shell", {
  object: UUID,
  thickness: 2,
  faces_to_remove: [],
});
check("a mutating call under an open intent carries the encoded phrase", () => {
  assert.notEqual(shellResult.isError, true, "shell proceeds (gate satisfied)");
  const post = seen.find((r) => r.method === "POST" && r.url === "/api/geometry/shell");
  assert.ok(post, "the shell POST reached the backend");
  assert.ok(post.intent !== null, "X-Roshera-Intent is present");
  assert.match(
    post.intent,
    /^[\x00-\x7F]*$/,
    "the header value must be ASCII-safe (URL-encoded), never raw non-ASCII",
  );
  assert.equal(
    decodeURIComponent(post.intent),
    INTENT_NAME,
    "decoding the header must recover the checkpoint phrase exactly",
  );
  assert.ok(post.turn !== null, "X-Roshera-Intent-Turn is present");
  assert.match(post.turn, /^\d+$/, "the turn is the gate's numeric turn counter");
});

check("EVERY backend call in the dispatch carries the same intent", () => {
  for (const r of seen) {
    assert.equal(
      r.intent === null ? null : decodeURIComponent(r.intent),
      INTENT_NAME,
      `${r.method} ${r.url} must carry the open intent`,
    );
  }
});

// ─── 4. reads under an open intent carry it too ────────────────────────────

seen.length = 0;
await call("list_parts", {});
check("a read-only call under an open intent still carries the header", () => {
  const get = seen.find((r) => r.method === "GET" && r.url === "/api/agent/parts");
  assert.ok(get, "the list_parts GET reached the backend");
  assert.equal(
    get.intent === null ? null : decodeURIComponent(get.intent),
    INTENT_NAME,
    "the header rides every api() call while the intent is open",
  );
});

stub.close();
console.log(`\nintent_header: ${passed} checks passed`);
