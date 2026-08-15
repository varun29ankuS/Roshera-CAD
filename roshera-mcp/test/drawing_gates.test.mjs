/**
 * Drawing-gate proof (2026-08-01 drawing-harness pass) — exercises the REAL
 * dispatch modules (ToolTable wrapper + gates.ts + the real io.ts handlers,
 * compiled from src to test/.build) against a local stub backend, so every
 * assertion runs the exact code path a live call runs, minus only the Rust
 * kernel.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/drawing_gates.test.mjs
 *
 * Proves:
 *   1. unsound-part drawing gate — make_drawing of a part whose live verdict
 *      is sound==false is refused (drawing endpoint untouched), is NEVER
 *      cached (re-issues re-read the live verdict), proceeds with
 *      acknowledge_unsound:true (through the invoke funnel, proving the flag
 *      survives schema validation), and proceeds plainly once the verdict
 *      flips sound — all WITHOUT requiring an intent checkpoint, because a
 *      sheet is not a solid feature.
 *   2. sheet-export gate, stale/dangling — drawing_export_sheet is refused
 *      while the live sheet certificate carries stale/dangling facts; the
 *      refusal names the offending facts; acknowledge_layout_issues does NOT
 *      bypass it; re-issues re-read the live certificate (never cached).
 *   3. sheet-export gate, layout quality — an Error-severity layout
 *      certificate refuses the export; acknowledge_layout_issues:true (via
 *      invoke) exports the draft; a clean sheet exports plainly and the file
 *      really lands on disk.
 *   4. sheet-export gate fails CLOSED — an unreadable certificate refuses
 *      (sheet_uncertified); the artifact endpoint is never touched.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFileSync, rmSync, existsSync } from "node:fs";

const HERE = dirname(fileURLToPath(import.meta.url));
const DRAWING = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const UNKNOWN = "99999999-9999-4999-8999-999999999999";
const PDF_BYTES = Buffer.from("%PDF-1.7 stub sheet");
const OUT = join(HERE, ".build", "drawing_gate_out.pdf");
rmSync(OUT, { force: true });

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = { perception: 0, make: 0, semantic: 0, pdf: 0 };
let partSound = false;
// The exact `acknowledge_layout_issues` query value the last /pdf request
// carried (the string as received, "" when the param was absent) — item 4's
// forwarding half needs a way to prove the flag actually reached the wire,
// not merely that the export proceeded (a call that never sends the flag at
// all would also "proceed" once the certificate is clean).
let lastPdfQuery = undefined;

// The live sheet certificate the stub serves — mutated between phases.
let cert = {
  sound: false,
  counts: { consistent: 3, stale: 2, dangling: 1 },
  facts: [
    { label: "⌀9.0 THRU", live: { verdict: "stale" } },
    { label: "40.00", live: { verdict: "stale" } },
    { label: "datum A face", live: { verdict: "dangling" } },
    { label: "25.00", live: { verdict: "consistent" } },
  ],
  quality: { passed: true, sheet_utilization: 0.41, issues: [] },
};

const stub = http.createServer((req, res) => {
  const rawUrl = req.url ?? "";
  const [url, qs] = rawUrl.split("?");
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    if (req.method === "GET" && url === "/api/agent/parts/9/perception") {
      counts.perception++;
      return partSound
        ? send({ valid: true, watertight: true, open_edges: 0 })
        : send({
            valid: false,
            watertight: false,
            open_edges: 5,
            verdict: "UNSOUND — see verify_part",
          });
    }
    if (req.method === "POST" && url.startsWith("/api/parts/9/drawing")) {
      counts.make++;
      return send({
        id: DRAWING,
        quality: { passed: true, sheet_utilization: 0.4, issues: [] },
      });
    }
    if (req.method === "GET" && url === `/api/drawings/${DRAWING}/semantic`) {
      counts.semantic++;
      return send({ drawing: { name: "stub sheet" }, certificate: cert });
    }
    if (req.method === "GET" && url === `/api/drawings/${DRAWING}/pdf`) {
      counts.pdf++;
      lastPdfQuery = new URLSearchParams(qs ?? "").get("acknowledge_layout_issues");
      res.writeHead(200, { "Content-Type": "application/pdf" });
      return res.end(PDF_BYTES);
    }
    // Unknown drawing's semantic → 404 (gate must fail closed, never open).
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
const firstJson = (r) => JSON.parse(r.content[0].text);
const isRefusal = (r, gate) => {
  const j = firstJson(r);
  return j.refused === true && (gate === undefined || j.gate === gate);
};

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── 1. unsound-part drawing gate ───────────────────────────────────────────

const makeArgs = { part_id: 9, name: "gate proof sheet" };

const m1 = await call("make_drawing", makeArgs);
check("make_drawing of an unsound part is refused (gate: unsound_base)", () => {
  assert.ok(isRefusal(m1, "unsound_base"));
  assert.equal(m1.isError, true);
  assert.equal(counts.make, 0, "drawing endpoint never hit");
  assert.equal(counts.perception, 1, "live verdict was read");
  assert.match(firstJson(m1).reason, /part 9 is UNSOUND/);
});

const m2 = await call("make_drawing", makeArgs);
check("the refusal is a live fact — re-issue re-reads, never cached", () => {
  assert.ok(isRefusal(m2, "unsound_base"));
  assert.equal(m2.content.length, 1, "no refusal-cache note");
  assert.equal(counts.perception, 2, "verdict fetched again");
});

const m3 = await call("invoke", {
  name: "make_drawing",
  args: { ...makeArgs, acknowledge_unsound: true },
});
check("acknowledge_unsound:true draws the inspection sheet (via invoke — flag survives schema)", () => {
  assert.equal(firstJson(m3).refused, undefined);
  assert.equal(firstJson(m3).drawing_id, DRAWING);
  assert.equal(counts.make, 1, "drawing endpoint hit exactly once");
});

partSound = true;
const m4 = await call("make_drawing", makeArgs);
check("sound part draws plainly — and no intent checkpoint was ever needed", () => {
  assert.equal(firstJson(m4).refused, undefined);
  assert.equal(counts.make, 2);
});

// ─── 2. sheet-export gate: stale/dangling facts ─────────────────────────────

const exportArgs = {
  drawing_id: DRAWING,
  format: "pdf",
  file_name: "gate_proof.pdf",
  save_path: OUT,
};

const e1 = await call("drawing_export_sheet", exportArgs);
check("export of a stale/dangling sheet is refused (gate: sheet_unsound)", () => {
  assert.ok(isRefusal(e1, "sheet_unsound"));
  assert.equal(e1.isError, true);
  assert.equal(counts.pdf, 0, "artifact endpoint never hit");
  assert.equal(counts.semantic, 1, "live certificate was read");
  const j = firstJson(e1);
  assert.match(j.reason, /2 stale/);
  assert.match(j.reason, /1 dangling/);
  assert.equal(j.unsound_facts.length, 3, "offending facts named");
  assert.match(j.how_to_proceed, /make_drawing/);
  assert.ok(!existsSync(OUT), "no file written");
});

const e2 = await call("drawing_export_sheet", exportArgs);
check("sheet refusals are live facts — re-issue re-reads, never cached", () => {
  assert.ok(isRefusal(e2, "sheet_unsound"));
  assert.equal(e2.content.length, 1, "no refusal-cache note");
  assert.equal(counts.semantic, 2, "certificate fetched again");
});

const e3 = await call("drawing_export_sheet", {
  ...exportArgs,
  acknowledge_layout_issues: true,
});
check("acknowledge_layout_issues does NOT bypass stale/dangling facts", () => {
  assert.ok(isRefusal(e3, "sheet_unsound"));
  assert.equal(counts.pdf, 0);
});

// ─── 3. sheet-export gate: layout quality + the legitimate flows ────────────

cert = {
  ...cert,
  sound: true,
  counts: { consistent: 4, stale: 0, dangling: 0 },
  facts: cert.facts.map((f) => ({ ...f, live: { verdict: "consistent" } })),
  quality: {
    passed: false,
    sheet_utilization: 0.4,
    issues: [
      {
        severity: "error",
        kind: "view_label_collision",
        message: "view label collides with FRONT geometry",
        view: "FRONT",
      },
      {
        severity: "warning",
        kind: "sheet_underutilized",
        message: "views cover 40% of the printable area",
        view: null,
      },
    ],
  },
};

const q1 = await call("drawing_export_sheet", exportArgs);
check("Error-severity layout certificate refuses the export (gate: sheet_quality)", () => {
  assert.ok(isRefusal(q1, "sheet_quality"));
  assert.equal(counts.pdf, 0);
  const j = firstJson(q1);
  assert.equal(j.layout_errors.length, 1, "only Error-severity findings listed");
  assert.match(j.layout_errors[0], /FRONT: view label collides/);
});

const q2 = await call("invoke", {
  name: "drawing_export_sheet",
  args: { ...exportArgs, acknowledge_layout_issues: true },
});
check("acknowledge_layout_issues:true exports the draft (via invoke — flag survives schema)", () => {
  assert.equal(firstJson(q2).refused, undefined);
  assert.equal(counts.pdf, 1, "artifact fetched exactly once");
  assert.deepEqual(readFileSync(OUT), PDF_BYTES, "the sheet really landed on disk");
  // S11/item 4: the escape must be FORWARDED, not merely honoured MCP-side —
  // a call that never sent the flag at all would also "proceed" once the
  // gate reads a stale cert, so the property worth pinning is that the wire
  // actually carries the acknowledgement, not just that export succeeded.
  assert.equal(lastPdfQuery, "true", "acknowledge_layout_issues reached the export route");
});

rmSync(OUT, { force: true });
cert = { ...cert, quality: { passed: true, sheet_utilization: 0.4, issues: [] } };

const q3 = await call("drawing_export_sheet", exportArgs);
check("a certified clean sheet exports plainly", () => {
  assert.equal(firstJson(q3).refused, undefined);
  assert.equal(counts.pdf, 2);
  assert.deepEqual(readFileSync(OUT), PDF_BYTES);
  // exportArgs never sets the flag — it must not be defaulted onto the wire.
  assert.equal(lastPdfQuery, null, "no acknowledge_layout_issues query param when the flag was never passed");
});

// ─── 4. unreadable certificate fails CLOSED ─────────────────────────────────

const u1 = await call("drawing_export_sheet", {
  ...exportArgs,
  drawing_id: UNKNOWN,
});
check("unreadable certificate refuses the export (gate: sheet_uncertified)", () => {
  assert.ok(isRefusal(u1, "sheet_uncertified"));
  assert.equal(counts.pdf, 2, "artifact endpoint not touched again");
  assert.match(firstJson(u1).reason, /could not be read/);
});

rmSync(OUT, { force: true });
stub.close();
console.log(`\ndrawing_gates: ${passed} checks passed`);
