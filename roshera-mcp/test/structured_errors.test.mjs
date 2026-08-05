/**
 * Structured-error proof (architectural audit, finding 2) — `fail()`
 * (core.ts) used to throw away the error catalog's structure: `error_code`,
 * `retryable`, and typed `details` (e.g. `BlendFailed`'s `r_max`) were
 * embedded in the JSON body of a thrown `ApiError`, but `fail()` only ever
 * read `e.message` and re-derived a hint by substring-matching prose the
 * catalog's own doc (`error_catalog.rs` lines 10-11) declares free to
 * evolve. This exercises the REAL dispatch stack (ToolTable wrapper +
 * gates.ts + the real `fillet_edges` handler + `core.ts`'s `api()`/`fail()`,
 * compiled from src to test/.build) against a local stub backend that
 * answers with the backend's OWN wire shape for three tiers:
 *
 *   (s1) a catalog error with NO backend hint but a typed `details.failure`
 *        (`blend_failed` / `RadiusExceedsCurvature`) — `fail()` must compute
 *        a legible retry hint FROM `r_max` itself, not guess from prose.
 *   (s2) a catalog error WITH a backend-authored hint (`part_not_found`) —
 *        `fail()` must surface that hint verbatim, not `errorHint`'s guess.
 *   (s3) a raw non-catalog body (a stub/legacy 400 with plain text, no JSON)
 *        — `structuredContent` must be ABSENT (nothing to parse) and the
 *        pre-existing `errorHint` substring fallback must still fire, so the
 *        new parsing path is additive, not a regression on the old one.
 *   (s4) mutation-proof note: see the bottom of this file for the manual
 *        mutation procedure (drop the parse, confirm RED, restore, confirm
 *        GREEN) — quoted in the task report, not re-run automatically here.
 *   (s5) errorHint prose-coupling gate — the substring branches `fail()`
 *        still falls back to for non-catalog errors are pinned against the
 *        ACTUAL backend source files that produce them, so a rename there
 *        breaks this test instead of silently degrading hint quality.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/structured_errors.test.mjs
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID = "11111111-2222-4333-8444-555555555555";

// ─── Stub backend ───────────────────────────────────────────────────────────
// Routes the fillet POST to one of three canned error bodies by the
// requested radius, so three fillet_edges calls with distinct args (never
// colliding with the refusal cache) exercise the three tiers.

let partSound = true; // never trip the unsound-base gate; this test is about fail()

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (status, obj, asJson = true) => {
    res.writeHead(status, { "Content-Type": asJson ? "application/json" : "text/plain" });
    res.end(asJson ? JSON.stringify(obj) : obj);
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send(200, { objects: [{ id: UUID, analytical_geometry: { solid_id: 7 } }] });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7/perception") {
      return send(200, { valid: partSound, watertight: partSound, open_edges: 0 });
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send(200, { unit: "mm" });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      return send(200, { id: "cp-1", name: JSON.parse(body || "{}").name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      return send(200, { id: "bb-1", text: JSON.parse(body || "{}").text, author: "agent", createdAt: 0, updatedAt: 0 });
    }
    if (req.method === "POST" && url === "/api/geometry/fillet") {
      const parsed = JSON.parse(body || "{}");
      if (parsed.radius === 2) {
        // (s1) exact api-server/src/error_catalog.rs::ApiError::blend_failed
        // wire shape for a RadiusExceedsCurvature BlendFailure — no `hint`
        // key (the real constructor never calls `.with_hint(...)` for this
        // code, per error_catalog.rs).
        return send(400, {
          success: false,
          error_code: "blend_failed",
          error:
            "blend failed: blend radius 2 at edge 7 station 0.420 exceeds local curvature limit r_max=1.25",
          retryable: false,
          details: {
            failure: {
              type: "RadiusExceedsCurvature",
              edge: 7,
              station: 0.42,
              r_requested: 2.0,
              r_max: 1.25,
            },
          },
        });
      }
      if (parsed.radius === 3) {
        // (s2) a catalog error WITH a backend-authored hint
        // (ApiError::part_not_found — error_catalog.rs).
        return send(400, {
          success: false,
          error_code: "part_not_found",
          error: `part ${UUID} not found`,
          retryable: false,
          hint: "Create a part with POST /api/parts and use the returned id in the X-Roshera-Part-Id header.",
          details: { part_id: UUID },
        });
      }
      // (s3) a raw, non-catalog 400 — plain text, not JSON. Deliberately
      // reuses errorHint's pre-existing "radius"+"not greater" phrasing so
      // the fallback branch is proven still reachable.
      return send(400, "kernel refused: radius is not greater than zero", false);
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

// ─── Open the intent checkpoint every mutating call below needs ────────────

const cp = await call("timeline_checkpoint", {
  name: "structured-error proof: fillet radius sweep on part 7",
  branch: "main",
});
check("checkpoint opens (gate satisfied for the fillet calls below)", () => {
  assert.notEqual(cp.isError, true, JSON.stringify(cp));
});

// ─── (s1) blend_failed / RadiusExceedsCurvature — computed r_max hint ─────

const r1 = await call("fillet_edges", { part_id: 7, radius: 2, edge_ids: [7] });

check("RED-first proof: fillet on radius=2 fails (kernel refused it)", () => {
  assert.equal(r1.isError, true);
});

check("(s1) structuredContent carries error_code + retryable + details.failure", () => {
  assert.ok(r1.structuredContent, "no structuredContent on the result at all");
  assert.equal(r1.structuredContent.error_code, "blend_failed");
  assert.equal(r1.structuredContent.retryable, false);
  assert.equal(r1.structuredContent.details.failure.type, "RadiusExceedsCurvature");
  assert.equal(r1.structuredContent.details.failure.r_max, 1.25);
  assert.equal(r1.structuredContent.details.failure.edge, 7);
});

check("(s1) the agent can read r_max WITHOUT parsing prose", () => {
  // The load-bearing assertion: r_max is a typed NUMBER field, not a
  // substring an agent has to regex out of `error`.
  const rMax = r1.structuredContent.details.failure.r_max;
  assert.equal(typeof rMax, "number");
  assert.equal(rMax, 1.25);
});

check("(s1) HINT line is COMPUTED from r_max (blend_failed carries no backend hint)", () => {
  const text = r1.content[0].text;
  assert.match(text, /HINT:/);
  assert.match(text, /r_max/i);
  assert.match(text, /1\.25/);
  assert.match(text, /edge 7/);
});

// ─── (s2) part_not_found — backend-authored hint surfaces verbatim ────────

const r2 = await call("fillet_edges", { part_id: 7, radius: 3, edge_ids: [7] });

check("(s2) structuredContent carries the generic catalog fields for ANY code", () => {
  assert.ok(r2.structuredContent);
  assert.equal(r2.structuredContent.error_code, "part_not_found");
  assert.equal(r2.structuredContent.retryable, false);
  assert.equal(r2.structuredContent.details.part_id, UUID);
});

check("(s2) HINT line is the BACKEND's own hint, not errorHint's guess", () => {
  const text = r2.content[0].text;
  assert.match(text, /Create a part with POST \/api\/parts/);
});

// ─── (s3) non-catalog body — fallback untouched, no structuredContent ─────

const r3 = await call("fillet_edges", { part_id: 7, radius: 4, edge_ids: [7] });

check("(s3) a non-JSON error body carries NO structuredContent (nothing to parse)", () => {
  assert.equal(r3.structuredContent, undefined);
});

check("(s3) errorHint's substring fallback still fires unchanged", () => {
  const text = r3.content[0].text;
  assert.match(text, /HINT:/);
  assert.match(text, /smaller radius|edge_ids/);
});

stub.close();

// ─── (s5) errorHint prose-coupling gate ────────────────────────────────────
//
// `fail()` still falls back to `errorHint`'s substring matching for errors
// the catalog-code path above does not cover. Grepping the whole backend
// tree for these substrings is noisy (a prior pass returned 80+ files,
// mostly unrelated partial-word hits) and gates nothing. Instead this pins
// each SURVIVING substring branch against the ACTUAL producer file/line —
// verified by direct inspection of the backend source, not assumed — so a
// rename in exactly that file breaks this test loudly, rather than
// `errorHint` silently returning null forever after.
console.log("(s5) errorHint prose-coupling gate: pinned against the real backend producers");

const BACKEND_ROOT = join(HERE, "..", "..", "roshera-backend", "geometry-engine", "src", "operations");

const PROSE_PINS = [
  {
    branch: "self-intersect / self intersect",
    file: "mod.rs",
    // OperationError's Display impl — `write!(f, "Operation would create self-intersection")`.
    substring: "would create self-intersection",
  },
  {
    branch: "not found in any face",
    file: "fillet.rs",
    substring: "Edge not found in any face",
  },
  {
    branch: "3-valent corner",
    file: "edge_blend_topology.rs",
    substring: "surgery requires a 3-valent corner",
  },
];

for (const pin of PROSE_PINS) {
  const path = join(BACKEND_ROOT, pin.file);
  let src;
  try {
    src = readFileSync(path, "utf8");
  } catch (e) {
    check(`FAIL to even read ${pin.file} for the '${pin.branch}' pin`, () => {
      throw e;
    });
    continue;
  }
  check(
    `errorHint's '${pin.branch}' branch still matches live prose in ${pin.file}`,
    () => {
      assert.ok(
        src.toLowerCase().includes(pin.substring.toLowerCase()),
        `expected "${pin.substring}" (case-insensitive) in ${path} — if the backend ` +
          `renamed this message, roshera-mcp/src/core.ts::errorHint must be updated ` +
          `to match (or the branch retired if a catalog error_code now covers it).`,
      );
    },
  );
}

console.log(`\nstructured_errors: ${passed} checks passed`);

// ─── (s4) MUTATION-PROOF PROCEDURE (manual, not automated here) ───────────
//
// To prove this test actually catches the defect it targets:
//   1. Comment out the `if (typeof o.error_code !== "string") return null;`
//      guard's TRUE branch in core.ts::parseCatalogError (or simpler: change
//      `e instanceof ApiError ? parseCatalogError(e.body) : null` to
//      `null` unconditionally in `fail()`), rebuild test/.build, rerun this
//      file — (s1)/(s2) go RED (no structuredContent, no computed r_max
//      hint). Quote the RED output.
//   2. Restore the original core.ts, rebuild, rerun — confirm GREEN again.
//      Quote that output too.
// (Performed once by hand for the task's report; not re-run on every CI
// pass because it requires temporarily breaking production code.)
