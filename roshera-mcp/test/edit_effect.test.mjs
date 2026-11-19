/**
 * A drawing edit reports its effect on the sheet (2026-11-19).
 *
 * Rehearsing the Volvo video, the agent set the bracket's bores to H7 and the
 * longer label "3x Ø20 H7 +0.021/0" collided with R25. The edit tool answered
 * "sound: true" and the agent moved on; only the export gate caught the
 * overlap. Every sheet-editing tool now reads the layout check before and
 * after and returns what changed: issues it CREATED, issues it RESOLVED, and
 * whether the sheet still passes -- the agent sees the consequence of its own
 * edit in the same answer.
 *
 *   npx tsc -p tsconfig.json --outDir test/.build && node test/edit_effect.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DRAWING = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

const clean = { passed: true, sheet_utilization: 0.31, issues: [] };
const overlap = {
  passed: false,
  sheet_utilization: 0.31,
  issues: [
    {
      severity: "Error",
      kind: "AnnotationOverlap",
      message: "dimension '3× Ø20 H7' overlaps 'R25'",
      view: "TOP",
    },
  ],
};
let quality = clean;
let qualityGets = 0;

const stub = http.createServer((req, res) => {
  const [url] = (req.url ?? "").split("?");
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const send = (obj) => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "GET" && url === `/api/drawings/${DRAWING}/quality`) {
      qualityGets++;
      return send(quality);
    }
    if (req.method === "PATCH" && url.startsWith(`/api/drawings/${DRAWING}/dimensions/`)) {
      // The edit lengthens the label: the sheet now has an overlap.
      quality = overlap;
      return send({
        dimension: { id: "d3", ink: "3× Ø20 H7" },
        certificate: { sound: true, complete: true, omission_summary: [] },
      });
    }
    if (req.method === "PATCH" && url === `/api/drawings/${DRAWING}/projection`) {
      // Moving to first angle clears it.
      quality = clean;
      return send({ projection: "first" });
    }
    res.writeHead(404);
    res.end("{}");
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const firstJson = (r) => JSON.parse(r.content[0].text);

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

const r1 = firstJson(
  await call("drawing_dimension_properties", { drawing_id: DRAWING, dimension_id: "d3", tolerance: "fit", fit: "H7" }),
);
check("an edit that creates an overlap says so in its own answer", () => {
  assert.ok(r1.sheet_effect, "the answer carries the sheet effect");
  assert.equal(r1.sheet_effect.layout_passes, false);
  assert.equal(r1.sheet_effect.new_issues.length, 1);
  assert.match(r1.sheet_effect.new_issues[0], /overlaps 'R25'/);
  assert.equal(r1.sheet_effect.resolved_issues.length, 0);
  assert.equal(qualityGets, 2, "read before and after the edit");
});

const r2 = firstJson(await call("drawing_set_projection", { drawing_id: DRAWING, projection: "first" }));
check("an edit that clears an issue says it resolved it", () => {
  assert.equal(r2.sheet_effect.layout_passes, true);
  assert.equal(r2.sheet_effect.new_issues.length, 0);
  assert.equal(r2.sheet_effect.resolved_issues.length, 1);
});

stub.close();
console.log(`edit_effect: ${passed} checks passed`);
