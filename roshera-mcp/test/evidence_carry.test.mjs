/**
 * Evidence the server sends must reach the agent (MCP_AUDIT.md 1.6-1.8, §3).
 *
 *   (e1) `transform`: the route answers `{object, solid_id, perception}` —
 *        `perception` IS the certificate (`certified_response`, main.rs). The
 *        tool used to answer `{object_uuid, moved: true, note}` and drop it.
 *        Now: the certified one-line verdict in the text, like every other
 *        mutating tool, and the server's whole certificate + the part id in
 *        structuredContent.
 *   (e2) `psketch_solve`: `entities_skipped` (entities the solver did NOT
 *        handle) and `dof.unverified_components` qualify "converged"; both
 *        were projected away. Now carried, in text and structuredContent.
 *   (e3) `gdt_fcf`: the route answers `annotation_id` (the handle PATCH /
 *        DELETE take) and `modifier`; both were dropped. Now carried, with the
 *        full verdict object beside the compact line.
 *   (e4) `gdt_fcf` refusal: the route's typed catalogue body reaches
 *        structuredContent (error_code / details) instead of being rebuilt as
 *        prose (MCP_AUDIT.md §2).
 *
 *     npm run test:gates:build && node test/evidence_carry.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID_A = "aaaaaaaa-1111-4222-8333-444444444444";
const SK = "bbbbbbbb-1111-4222-8333-444444444444";

const CERT = {
  sound: true,
  brep_valid: true,
  watertight: true,
  manifold: true,
  shells_outward: true,
  self_intersection_free: true,
  boundary_edges: 0,
  nonmanifold_edges: 0,
  errors: [],
};
const TRANSFORM_ANSWER = {
  success: true,
  object: UUID_A,
  solid_id: 7,
  perception: { sound: true, cert: CERT, face_count: 6, volume: 1000, dims: [10, 10, 10] },
};
const SOLVE_ANSWER = {
  status: "Converged",
  violations: [],
  solve_time_ms: 1.5,
  entities_solved: 4,
  constraints_solved: 3,
  entities_skipped: [{ entity: { Polyline: 3 }, reason: "polyline not solvable in v1" }],
  dof: {
    status: "UnderConstrained",
    total_free_dofs: 2,
    redundant: [],
    conflicts: [],
    unsupported: [],
    singular_configuration: false,
    unverified_components: [1],
  },
  certificate: { verdict: "solved, under-constrained" },
};
const FCF_ANSWER = {
  part_id: 5,
  annotation_id: "dddddddd-1111-4222-8333-444444444444",
  annotation_pid: "0123456789abcdef0123456789abcdef",
  modifier: "mmc",
  // handlers/gdt.rs::VerdictWire (the fields the compact line reads).
  verdict: {
    characteristic: "flatness",
    tolerance_mm: 0.05,
    tolerance_label: "0.05mm",
    measured_mm: 0.001,
    measured_label: "0.001mm",
    conforms: "in_spec",
    reason: null,
    fit_residual_mm: null,
    datum_statuses: [],
    frame: null,
  },
  persistence: "timeline",
};
let fcfRefuse = false;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const send = (status, obj) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send(200, { objects: [{ id: UUID_A, analytical_geometry: { solid_id: 7 } }] });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/perception/.test(url)) {
      return send(200, { valid: true, watertight: true, open_edges: 0, sound: true, cert: CERT });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      return send(200, { id: 7, topology: { face_count: 6 }, volume: 1000, location: { center_world: [0, 0, 0], dimensions_world: [10, 10, 10] } });
    }
    if (req.method === "GET" && url === "/api/agent/parts") return send(200, [{ id: 7 }]);
    if (req.method === "GET" && url === "/api/document/units") return send(200, { unit: "mm" });
    if (req.method === "POST" && url === "/api/timeline/checkpoint") return send(200, { id: "cp-1" });
    if (req.method === "POST" && url === "/api/blackboard/entries") return send(200, { id: "bb-1" });
    if (req.method === "POST" && url === "/api/geometry/transform") return send(200, TRANSFORM_ANSWER);
    if (req.method === "POST" && url === `/api/csketch/${SK}/solve`) return send(200, SOLVE_ANSWER);
    if (req.method === "GET" && url === `/api/csketch/${SK}`) return send(200, { points: [] });
    if (req.method === "POST" && url === "/api/agent/parts/5/fcf") {
      if (fcfRefuse) {
        return send(422, {
          success: false,
          error_code: "invalid_parameter",
          error: "datum 'B' is not designated on part 5",
          retryable: false,
          details: { datum: "B" },
        });
      }
      return send(200, FCF_ANSWER);
    }
    send(404, {});
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
const check = async (label, fn) => {
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await call("timeline_checkpoint", { name: "evidence proof — move the verification cube 10 mm in x", branch: "main" });

await check("(e1) transform carries the certificate the server sent", async () => {
  const r = await call("transform", { object: UUID_A, translation: [10, 0, 0] });
  assert.notEqual(r.isError, true, r.content[0].text);
  const out = firstJson(r);
  assert.equal(out.object_uuid, UUID_A);
  assert.equal(out.part_id, 7, "the integer part id the move kept");
  assert.match(String(out.perception), /SOUND/, `a certified verdict line, got ${out.perception}`);
  assert.deepEqual(r.structuredContent?.certificate, TRANSFORM_ANSWER.perception, "the server's certificate, whole");
  assert.equal(r.structuredContent?.part_id, 7);
});

await check("(e2) psketch_solve keeps entities_skipped and unverified components", async () => {
  const r = await call("psketch_solve", { csketch_id: SK });
  assert.notEqual(r.isError, true, r.content[0].text);
  const out = firstJson(r);
  assert.deepEqual(out.entities_skipped, SOLVE_ANSWER.entities_skipped);
  assert.deepEqual(out.dof.unverified_components, [1]);
  assert.deepEqual(r.structuredContent?.entities_skipped, SOLVE_ANSWER.entities_skipped);
});

await check("(e3) gdt_fcf keeps annotation_id, modifier and the verdict object", async () => {
  const r = await call("gdt_fcf", { part_id: 5, characteristic: "flatness", tolerance_mm: 0.05, target_face: 2 });
  assert.notEqual(r.isError, true, r.content[0].text);
  const out = firstJson(r);
  assert.equal(out.annotation_id, FCF_ANSWER.annotation_id);
  assert.equal(out.modifier, "mmc");
  assert.equal(typeof out.verdict, "string", "the compact line stays");
  assert.deepEqual(out.verdict_detail, FCF_ANSWER.verdict);
  assert.equal(r.structuredContent?.annotation_id, FCF_ANSWER.annotation_id);
});

await check("(e4) a refused FCF reaches the agent typed", async () => {
  fcfRefuse = true;
  const r = await call("gdt_fcf", { part_id: 5, characteristic: "parallelism", tolerance_mm: 0.05, target_face: 2, datum_refs: ["B"] });
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.error_code, "invalid_parameter");
  assert.deepEqual(r.structuredContent?.details, { datum: "B" });
});

stub.close();
console.log(`\nevidence_carry: ${passed} checks passed`);
