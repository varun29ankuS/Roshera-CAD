/**
 * Single-point-run gate proof (2026-08-09) — CONSTRAINT BEATS STEERING.
 *
 * Measured failure: an agent laid out a 256-point gear profile one
 * psketch_add_entity {kind:'point'} call at a time — 1.3M tokens for geometry
 * ONE polyline call expresses. The tool descriptions steer against it; this
 * gate makes the pattern refuse typed at the dispatch choke point.
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + gates.ts + real
 * handlers, compiled from src to test/.build) against a local stub backend.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/single_point_gate.test.mjs
 *
 * Proves:
 *   1. eight consecutive psketch_add_entity {kind:'point'} calls to one
 *      sketch all reach the backend (legitimate small sketches untouched);
 *   2. the ninth is refused TYPED (gate: single_point_run) naming the count
 *      and the bulk polyline path, and the backend is NOT hit;
 *   3. a refused/repeated point call does not reset the run — re-issue is
 *      refused again, still without touching the backend;
 *   4. the refusal is NEVER served from the refusal cache: any other tool
 *      call resets the counter and the identical point call then proceeds;
 *   5. a polyline call (the bulk path) also resets the counter;
 *   6. counters are PER SKETCH — a parallel second sketch has its own run;
 *   7. sketch_points rides the same gate when called with ONE point per
 *      call, while a legitimate multi-point call neither counts nor trips.
 *
 * Item 9 (2026-08-15, audit S11) additions — the CUMULATIVE counter that
 * closes the interleave escape the run counter alone leaves open
 * (`point×8 → filler → repeat` never trips the run cap):
 *   8. 4 interleaved 8-point bursts (32 points, one filler call between
 *      each) all reach the backend — the run cap never trips, isolating
 *      the cumulative condition — and the 33rd point (a 5th burst after
 *      the SAME filler trick) is refused typed (gate:
 *      single_point_cumulative), with a further filler call NOT unblocking
 *      it and the refusal read fresh every time, never cache-replayed;
 *   9. the cumulative counter is per sketch, same as the run counter.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SK_A = "aaaaaaaa-1111-4111-8111-111111111111";
const SK_B = "bbbbbbbb-2222-4222-8222-222222222222";

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = {
  psketchPoint: 0, // POST /api/csketch/:id/point
  polyline: 0, //     POST /api/csketch/:id/polyline
  clickPoint: 0, //   POST /api/sketch/:id/point
};

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    let m;
    if (req.method === "POST" && (m = url.match(/^\/api\/csketch\/([^/]+)\/point$/))) {
      counts.psketchPoint++;
      return send({ id: `pt-${m[1].slice(0, 4)}-${counts.psketchPoint}` });
    }
    if (req.method === "POST" && /^\/api\/csketch\/[^/]+\/polyline$/.test(url)) {
      counts.polyline++;
      return send({ id: `poly-${counts.polyline}` });
    }
    if (req.method === "POST" && /^\/api\/sketch\/[^/]+\/point$/.test(url)) {
      counts.clickPoint++;
      return send({ ok: true });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([]);
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
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
const firstJson = (r) => JSON.parse(r.content[0].text);
const isRefusal = (r, gate) => {
  const j = firstJson(r);
  return j.refused === true && (gate === undefined || j.gate === gate);
};
const addPoint = (sketch, x, y) =>
  call("psketch_add_entity", {
    csketch_id: sketch,
    kind: "point",
    params: { x, y },
  });

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── 1. eight consecutive single points pass (small sketches untouched) ─────

for (let i = 0; i < 8; i++) {
  const r = await addPoint(SK_A, i, i * 2);
  check(`point ${i + 1}/8 to sketch A reaches the backend`, () => {
    assert.equal(firstJson(r).refused, undefined);
    assert.equal(counts.psketchPoint, i + 1);
  });
}

// ─── 2. the ninth is refused typed, backend untouched ───────────────────────

const r9 = await addPoint(SK_A, 8, 16);
check("9th consecutive single point is refused (gate: single_point_run)", () => {
  assert.ok(isRefusal(r9, "single_point_run"));
  assert.equal(r9.isError, true);
  assert.equal(counts.psketchPoint, 8, "backend never saw the 9th point");
  const j = firstJson(r9);
  assert.match(j.reason, /8 consecutive single-point/);
  assert.match(j.how_to_proceed, /polyline/);
  assert.match(j.how_to_proceed, /ONE call/i);
});

// ─── 3. a refused point call does not reset the run ─────────────────────────

const r10 = await addPoint(SK_A, 9, 18);
check("10th point (after a refusal) is refused again, backend untouched", () => {
  assert.ok(isRefusal(r10, "single_point_run"));
  assert.equal(counts.psketchPoint, 8);
  assert.equal(
    r10.content.length,
    1,
    "fresh live refusal — never a refusal-cache replay",
  );
});

// ─── 4. any other tool call resets; the same point call then proceeds ───────

await call("list_parts", {});
const r11 = await addPoint(SK_A, 8, 16); // IDENTICAL args to the refused r9
check("after an intervening call the identical point call proceeds", () => {
  assert.equal(firstJson(r11).refused, undefined);
  assert.equal(counts.psketchPoint, 9, "backend hit again after the reset");
});

// ─── 5. the bulk polyline path resets the counter too ───────────────────────

for (let i = 0; i < 7; i++) await addPoint(SK_A, 20 + i, 0);
const poly = await call("psketch_add_entity", {
  csketch_id: SK_A,
  kind: "polyline",
  params: { points: [[0, 0], [10, 0], [10, 10]], closed: true },
});
const afterPoly = await addPoint(SK_A, 40, 40);
check("a polyline call resets the run — points flow again after it", () => {
  assert.equal(firstJson(poly).refused, undefined);
  assert.equal(counts.polyline, 1);
  assert.equal(firstJson(afterPoly).refused, undefined);
});

// ─── 6. counters are per sketch ─────────────────────────────────────────────

// afterPoly above left sketch A at run=1. Interleave: bring A to the limit,
// then a point to sketch B must still pass (its own run is short).
for (let i = 0; i < 7; i++) await addPoint(SK_A, 50 + i, 1);
const bFirst = await addPoint(SK_B, 0, 0);
const aNext = await addPoint(SK_A, 60, 1);
check("sketch B keeps its own counter; sketch A's run is not reset by B", () => {
  assert.equal(firstJson(bFirst).refused, undefined, "B's first point passes");
  assert.ok(isRefusal(aNext, "single_point_run"), "A is at its limit");
});

// ─── 7. sketch_points rides the same gate for 1-point calls ─────────────────

await call("list_parts", {}); // clean slate
for (let i = 0; i < 8; i++) {
  const r = await call("sketch_points", {
    sketch_id: "click-1",
    points: [[i, i]],
  });
  assert.equal(firstJson(r).refused, undefined, `1-point call ${i + 1} passes`);
}
const sp9 = await call("sketch_points", {
  sketch_id: "click-1",
  points: [[99, 99]],
});
check("9th consecutive 1-point sketch_points call is refused", () => {
  assert.ok(isRefusal(sp9, "single_point_run"));
  assert.equal(counts.clickPoint, 8);
});
const multi = await call("sketch_points", {
  sketch_id: "click-1",
  points: [[0, 0], [10, 0], [10, 10], [0, 10]],
});
const spAfter = await call("sketch_points", {
  sketch_id: "click-1",
  points: [[1, 1]],
});
check("a multi-point call neither counts nor trips, and resets the run", () => {
  assert.equal(firstJson(multi).refused, undefined);
  assert.equal(firstJson(spAfter).refused, undefined);
  // 8 single points + 4 from the multi-point call + 1 after the reset.
  assert.equal(counts.clickPoint, 13, "every allowed point reached the backend");
});

// ─── 8. the cumulative counter closes the interleave escape (item 9) ───────
//
// S11's exploit: psketch_add_entity{point}×8 → list_parts{} → repeat never
// trips the RUN counter (each burst is exactly 8, and the filler call
// resets it before the next burst starts) while still reaching arbitrarily
// large point counts at the cost of one cheap call per 8 points. A fresh
// sketch (SK_C) isolates this from every counter state built up above.

const SK_C = "cccccccc-3333-4333-8333-333333333333";
const beforeC = counts.psketchPoint;

let sawRefusalDuringBursts = false;
for (let burst = 0; burst < 4 && !sawRefusalDuringBursts; burst++) {
  for (let i = 0; i < 8; i++) {
    const r = await addPoint(SK_C, burst * 100 + i, 0);
    if (firstJson(r).refused) {
      sawRefusalDuringBursts = true;
      break;
    }
  }
  await call("list_parts", {}); // the filler — resets the RUN counter only
}
check(
  "32 points across 4 interleaved 8-point bursts all reach the backend (the run cap never trips)",
  () => {
    assert.equal(
      sawRefusalDuringBursts,
      false,
      "the filler-call trick must not trip the RUN gate itself — that would " +
        "mean this test is no longer isolating the cumulative condition",
    );
    assert.equal(
      counts.psketchPoint - beforeC,
      32,
      "all 32 points across the 4 bursts must reach the backend",
    );
  },
);

const r33 = await addPoint(SK_C, 999, 0);
check(
  "the 33rd point — a 5th burst after the SAME filler trick — is refused " +
    "(gate: single_point_cumulative), closing the interleave escape",
  () => {
    assert.ok(isRefusal(r33, "single_point_cumulative"));
    assert.equal(r33.isError, true);
    const j = firstJson(r33);
    assert.equal(j.points_placed_cumulative, 32);
    assert.match(j.how_to_proceed, /polyline/);
    assert.equal(
      counts.psketchPoint - beforeC,
      32,
      "backend never saw the 33rd point",
    );
  },
);

// The filler trick that resets the RUN counter does NOT help here — that is
// the entire point of the fix.
await call("list_parts", {});
const r34 = await addPoint(SK_C, 1000, 0);
check(
  "another filler call does not unblock the cumulative refusal — the interleave escape stays closed",
  () => {
    assert.ok(isRefusal(r34, "single_point_cumulative"));
    assert.equal(
      counts.psketchPoint - beforeC,
      32,
      "still backend-untouched after the filler",
    );
  },
);

const r35 = await addPoint(SK_C, 1001, 0);
check(
  "the cumulative refusal is a fresh live read every time, never a refusal-cache replay",
  () => {
    assert.ok(isRefusal(r35, "single_point_cumulative"));
    assert.equal(
      r35.content.length,
      1,
      "fresh live refusal — never a refusal-cache replay",
    );
  },
);

// ─── 9. the cumulative counter is per sketch ────────────────────────────────

const SK_D = "dddddddd-4444-4444-8444-444444444444";
const dFirst = await addPoint(SK_D, 0, 0);
check(
  "a fresh sketch is unaffected by SK_C's cumulative cap — counters are per sketch",
  () => {
    assert.equal(firstJson(dFirst).refused, undefined);
  },
);

stub.close();
console.log(`\nsingle_point_gate: ${passed} checks passed`);
