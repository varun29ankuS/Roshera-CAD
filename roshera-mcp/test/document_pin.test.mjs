/**
 * Document-pin proof — the precondition for parallel episodes.
 *
 * `bindSessionDocument` bound to whichever document the server reported as
 * `active`, a single global notion, so two MCP processes starting
 * concurrently raced for the same document. This pins the explicit override:
 * ROSHERA_DOCUMENT names the birth document directly, discovery still runs
 * when it is absent, and unbound legacy behaviour is untouched when both are.
 *
 *   npx tsc -p tsconfig.json --outDir test/.build
 *   node test/document_pin.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ACTIVE_DOC = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const PINNED_DOC = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

/** @type {{ url: string, doc: string | null }[]} */
let seen = [];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  seen.push({ url, doc: req.headers["x-roshera-document"] ?? null });
  res.writeHead(200, { "Content-Type": "application/json" });
  if (req.method === "GET" && url === "/api/documents") {
    return res.end(JSON.stringify([
      { id: "cccccccc-3333-4333-8333-cccccccccccc", active: false },
      { id: ACTIVE_DOC, active: true },
    ]));
  }
  res.end(JSON.stringify({ ok: true }));
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;

const coreUrl = pathToFileURL(join(HERE, ".build", "core.js")).href;

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

// A fresh module instance per case: `boundDocument` is module state, and
// bindSessionDocument is a once-at-startup call, so each case must import a
// clean copy rather than re-binding a dirty one.
async function freshCore() {
  return await import(`${coreUrl}?case=${checks.length}-${Date.now()}`);
}

check("an explicit ROSHERA_DOCUMENT pin wins, and makes no discovery call", async () => {
  seen = [];
  process.env.ROSHERA_DOCUMENT = PINNED_DOC;
  const core = await freshCore();
  await core.bindSessionDocument();
  assert.equal(
    seen.filter((s) => s.url === "/api/documents").length, 0,
    "a pinned document must not fetch the document list at all",
  );
  await core.api("GET", "/api/agent/parts").catch(() => {});
  const call = seen.find((s) => s.url === "/api/agent/parts");
  assert.ok(call, "the probe call reached the stub");
  assert.equal(call.doc, PINNED_DOC, "the pinned id rides on the wire");
});

check("with no pin, active-document discovery still runs", async () => {
  seen = [];
  delete process.env.ROSHERA_DOCUMENT;
  const core = await freshCore();
  await core.bindSessionDocument();
  assert.ok(
    seen.some((s) => s.url === "/api/documents"),
    "discovery must still fetch the list when nothing is pinned",
  );
  await core.api("GET", "/api/agent/parts").catch(() => {});
  const call = seen.find((s) => s.url === "/api/agent/parts");
  assert.equal(call.doc, ACTIVE_DOC, "falls back to the active document");
});

check("an empty pin is treated as absent, never as an empty document id", async () => {
  seen = [];
  process.env.ROSHERA_DOCUMENT = "   ";
  const core = await freshCore();
  await core.bindSessionDocument();
  assert.ok(
    seen.some((s) => s.url === "/api/documents"),
    "whitespace is not a document id — discovery must run",
  );
  delete process.env.ROSHERA_DOCUMENT;
});

for (const [name, fn] of checks) {
  await fn();
  process.stdout.write(`  ok - ${name}\n`);
}
stub.close();
process.stdout.write(`\ndocument_pin: ${checks.length} checks passed\n`);
