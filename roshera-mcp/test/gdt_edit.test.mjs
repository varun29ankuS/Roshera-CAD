/**
 * The agent can correct GD&T it authored (2026-11-20).
 *
 * In two recorded sessions the agent authored a position frame with the wrong
 * sign on a basic dimension; the kernel's verdict caught it, and the agent had
 * no tool to edit or delete the frame, so it undid the timeline instead. The
 * server already had the routes (handlers/gdt.rs): PATCH / DELETE
 * `/api/agent/parts/{id}/fcf/{annotation_id}`, PATCH / DELETE
 * `/api/agent/parts/{id}/datums/{label}`, DELETE
 * `/api/agent/parts/{id}/size-tolerance/{annotation_id}`. Each records a
 * `gdt_*` design event, so each edit is itself undoable.
 *
 *   (g1) gdt_fcf_update sends PATCH with ONLY the fields given (the wrong-sign
 *        fix is `{basic:[-x, y]}` and nothing else), mapping target_face /
 *        target_selector to the route's face_id / selector;
 *   (g2) an update naming no change is refused locally — no request, so no
 *        no-op event is recorded;
 *   (g3) gdt_fcf_delete / gdt_dimensional_delete send DELETE to their own
 *        route; gdt_datum_update / gdt_datum_delete address the datum by its
 *        letter (URL-encoded);
 *   (g4) every success says the edit is undoable, from the server's own
 *        `persistence: "timeline"`, never hardcoded;
 *   (g5) a refused edit carries a top-level `reason` = the server's sentence
 *        (`message`), its code as `error_code`, and the body verbatim as
 *        `refused` — a 409 datum_referenced keeps the frames it names;
 *   (g6) disconnection gate: each tool is in the PRODUCTION table, benched
 *        `drawing`, on the full surface and the drawing bench, and `invoke`
 *        dispatches it to the route;
 *   (g7) find_tool ranks each tool first for the words an agent would use.
 *
 *     npm run test:gates:build && node test/gdt_edit.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const FRAME = "dddddddd-1111-4222-8333-444444444444";
const SIZE = "eeeeeeee-1111-4222-8333-444444444444";

/** handlers/gdt.rs::FcfResponse for an edited position frame. */
const FCF_EDITED = {
  part_id: 5,
  annotation_id: FRAME,
  annotation_pid: "0123456789abcdef0123456789abcdef",
  modifier: "rfs",
  verdict: {
    characteristic: "position",
    tolerance_mm: 0.1,
    tolerance_label: "0.100mm",
    measured_mm: 0.002,
    measured_label: "0.002mm",
    conforms: "in_spec",
    reason: null,
    fit_residual_mm: 1e-6,
    datum_statuses: [{ label: "A", resolution: { status: "live", origin: [0, 0, 0], direction: [0, 0, 1] } }],
    frame: { origin: [0, 0, 0], x_axis: [1, 0, 0], y_axis: [0, 1, 0], z_axis: [0, 0, 1], derivation: "part corner" },
  },
  persistence: "timeline",
};

/** handlers/gdt.rs::get_gdt_handler: one frame and one size tolerance. */
const GDT_REPORT = {
  part_id: 5,
  datums: [{ label: "A", kind: "plane", persistent_id: "fedcba9876543210fedcba9876543210", resolution: { status: "live", origin: [0, 0, 0], direction: [0, 0, 1] } }],
  annotations: [
    { annotation_id: FRAME, kind: "geometric", status: "live", feature_pid: "01", modifier: "rfs", verdict: FCF_EDITED.verdict, target_face_id: 3, anchor_mm: [0, 0, 0] },
    {
      annotation_id: SIZE,
      kind: "dimensional",
      status: "live",
      feature_pid: "02",
      modifier: null,
      verdict: { characteristic: "size", tolerance_mm: 0, tolerance_label: "", measured_mm: null, measured_label: null, conforms: "not_evaluable", reason: "size", fit_residual_mm: null, datum_statuses: [] },
      target_face_id: 4,
      anchor_mm: [0, 0, 0],
    },
  ],
  persistence: "timeline",
};

/** Requests the stub saw: {method, url, body}. */
let seen = [];
/** A refusal the next matching request answers with: [status, body] or null. */
let refuseNext = null;
/** What the delete / datum routes report as their durability. */
let persistence = "timeline";

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const send = (status, obj) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    const isGdtEdit = /^\/api\/agent\/parts\/\d+\/(fcf|datums|size-tolerance)\//.test(url);
    if (isGdtEdit) {
      seen.push({ method: req.method, url, body: raw.length ? JSON.parse(raw) : undefined });
      if (refuseNext) {
        const [status, body] = refuseNext;
        refuseNext = null;
        return send(status, body);
      }
    }
    if (req.method === "PATCH" && url === `/api/agent/parts/5/fcf/${FRAME}`) return send(200, FCF_EDITED);
    if (req.method === "DELETE" && url === `/api/agent/parts/5/fcf/${FRAME}`) {
      return send(200, { success: true, part_id: 5, annotation_id: FRAME, persistence });
    }
    if (req.method === "DELETE" && url === `/api/agent/parts/5/size-tolerance/${SIZE}`) {
      return send(200, { success: true, part_id: 5, annotation_id: SIZE, persistence });
    }
    const datum = url.match(/^\/api\/agent\/parts\/5\/datums\/([^/]+)$/);
    if (datum && req.method === "PATCH") {
      const body = raw.length ? JSON.parse(raw) : {};
      return send(200, {
        success: true,
        part_id: 5,
        label: body.label ?? decodeURIComponent(datum[1]),
        kind: "plane",
        persistent_id: "fedcba9876543210fedcba9876543210",
        face_id: body.face_id ?? 3,
        persistence,
      });
    }
    if (datum && req.method === "DELETE") {
      return send(200, { success: true, part_id: 5, label: decodeURIComponent(datum[1]), persistence });
    }
    if (req.method === "GET" && url === "/api/agent/parts/5/gdt") return send(200, GDT_REPORT);
    send(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable, exposedNamesFor } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { metaFor } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);
const { SWITCHABLE_BENCHES } = await import(pathToFileURL(join(HERE, ".build", "workbench.js")).href);

const table = buildTable();
const NEW_TOOLS = [
  "gdt_fcf_update",
  "gdt_fcf_delete",
  "gdt_datum_update",
  "gdt_datum_delete",
  "gdt_dimensional_delete",
];
const call = (name, args = {}) => {
  const entry = table.get(name);
  assert.ok(entry, `${name} is registered in the production table`);
  return entry.handler(args, {});
};
const textJson = (r) => JSON.parse(r.content[0].text);

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  refuseNext = null;
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

// ─── (g1) gdt_fcf_update: method, path, exact body ────────────────────────────

await check("(g1a) the wrong-sign fix PATCHes only `basic`", async () => {
  const r = await call("gdt_fcf_update", { part_id: 5, annotation_id: FRAME, basic: [-20, 10] });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [
    { method: "PATCH", url: `/api/agent/parts/5/fcf/${FRAME}`, body: { basic: [-20, 10] } },
  ]);
  const out = textJson(r);
  assert.equal(out.annotation_id, FRAME);
  assert.equal(typeof out.verdict, "string", "the compact verdict line");
  assert.match(out.verdict, /IN SPEC/);
  assert.deepEqual(out.verdict_detail, FCF_EDITED.verdict, "the server's verdict object, whole");
  assert.equal(r.structuredContent?.annotation_id, FRAME);
});

await check("(g1b) every field maps onto the route's own names", async () => {
  const r = await call("gdt_fcf_update", {
    part_id: 5,
    annotation_id: FRAME,
    characteristic: "position",
    tolerance_mm: 0.1,
    datum_refs: ["A", "B"],
    modifier: "mmc",
    basic: [20, -10],
    target_selector: '{"kind":"cylindrical","extremal":"largest_area"}',
  });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen[0].body, {
    characteristic: "position",
    tolerance_mm: 0.1,
    datum_refs: ["A", "B"],
    modifier: "mmc",
    basic: [20, -10],
    selector: { kind: "cylindrical", extremal: "largest_area" },
  });
  seen = [];
  await call("gdt_fcf_update", { part_id: 5, annotation_id: FRAME, target_face: 9 });
  assert.deepEqual(seen[0].body, { face_id: 9 }, "target_face is the route's face_id");
});

// ─── (g2) an empty edit sends nothing ─────────────────────────────────────────

await check("(g2) an update naming no change is refused before any request", async () => {
  const f = await call("gdt_fcf_update", { part_id: 5, annotation_id: FRAME });
  assert.equal(f.isError, true);
  assert.equal(typeof f.structuredContent?.reason, "string");
  const d = await call("gdt_datum_update", { part_id: 5, label: "A" });
  assert.equal(d.isError, true);
  assert.equal(typeof d.structuredContent?.reason, "string");
  assert.deepEqual(seen, [], "nothing reached the server, so nothing was recorded");
});

// ─── (g3) deletes and datum edits ─────────────────────────────────────────────

await check("(g3a) gdt_fcf_delete sends DELETE to the frame's route", async () => {
  const r = await call("gdt_fcf_delete", { part_id: 5, annotation_id: FRAME });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [{ method: "DELETE", url: `/api/agent/parts/5/fcf/${FRAME}`, body: undefined }]);
  assert.equal(textJson(r).annotation_id, FRAME);
});

await check("(g3b) gdt_dimensional_delete sends DELETE to the size-tolerance route", async () => {
  const r = await call("gdt_dimensional_delete", { part_id: 5, annotation_id: SIZE });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [
    { method: "DELETE", url: `/api/agent/parts/5/size-tolerance/${SIZE}`, body: undefined },
  ]);
});

await check("(g3c) gdt_datum_update re-letters / re-targets by the current letter", async () => {
  const r = await call("gdt_datum_update", { part_id: 5, label: "A", new_label: "C" });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [{ method: "PATCH", url: "/api/agent/parts/5/datums/A", body: { label: "C" } }]);
  assert.equal(textJson(r).label, "C");
  seen = [];
  await call("gdt_datum_update", { part_id: 5, label: "A", face_id: 4 });
  assert.deepEqual(seen[0].body, { face_id: 4 });
  seen = [];
  await call("gdt_datum_update", { part_id: 5, label: "A", selector: '{"kind":"planar","normal_dir":[0,0,-1]}' });
  assert.deepEqual(seen[0].body, { selector: { kind: "planar", normal_dir: [0, 0, -1] } });
});

await check("(g3d) gdt_datum_delete sends DELETE; the letter is URL-encoded", async () => {
  const r = await call("gdt_datum_delete", { part_id: 5, label: "A" });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [{ method: "DELETE", url: "/api/agent/parts/5/datums/A", body: undefined }]);
  seen = [];
  await call("gdt_datum_delete", { part_id: 5, label: "A/B" });
  assert.equal(seen[0].url, "/api/agent/parts/5/datums/A%2FB");
});

// ─── (g4) undoability comes from the server ───────────────────────────────────

await check("(g4) every success says the edit is undoable, from persistence:timeline", async () => {
  const results = [
    await call("gdt_fcf_update", { part_id: 5, annotation_id: FRAME, tolerance_mm: 0.1 }),
    await call("gdt_fcf_delete", { part_id: 5, annotation_id: FRAME }),
    await call("gdt_datum_update", { part_id: 5, label: "A", new_label: "C" }),
    await call("gdt_datum_delete", { part_id: 5, label: "A" }),
    await call("gdt_dimensional_delete", { part_id: 5, annotation_id: SIZE }),
  ];
  for (const r of results) {
    const out = textJson(r);
    assert.equal(out.persistence, "timeline");
    assert.equal(out.undoable, true, `undoable on ${r.content[0].text}`);
  }
  // A route that did not record must not be reported undoable.
  persistence = "session";
  try {
    const r = await call("gdt_datum_delete", { part_id: 5, label: "B" });
    assert.equal(textJson(r).undoable, false, "an unrecorded edit is never claimed undoable");
    assert.match(textJson(r).undo, /cannot be undone/);
  } finally {
    persistence = "timeline";
  }
});

// ─── (g5) refusals carry a top-level reason ───────────────────────────────────

await check("(g5a) a refused delete (404 annotation_not_found) carries reason verbatim", async () => {
  const body = { error: "annotation_not_found", message: "no annotation with id 'nope' is recorded" };
  refuseNext = [404, body];
  const r = await call("gdt_fcf_delete", { part_id: 5, annotation_id: "nope" });
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.reason, body.message, "the server's sentence, not its code");
  assert.equal(r.structuredContent?.error_code, "annotation_not_found");
  assert.deepEqual(r.structuredContent?.refused, body, "the server's body, verbatim");
  assert.equal(textJson(r).reason, body.message, "the text the agent reads carries it too");
});

await check("(g5b) a 409 datum_referenced keeps the frames it names", async () => {
  const body = {
    error: "datum_referenced",
    message: "datum 'A' on part 5 is referenced by 1 frame(s); edit or delete those frames first",
    annotations: [FRAME],
  };
  refuseNext = [409, body];
  const r = await call("gdt_datum_delete", { part_id: 5, label: "A" });
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.reason, body.message);
  assert.equal(r.structuredContent?.error_code, "datum_referenced");
  assert.deepEqual(r.structuredContent?.refused?.annotations, [FRAME]);
});

await check("(g5c) a 422 kind mismatch and a refused update are typed the same way", async () => {
  const mismatch = { error: "annotation_kind_mismatch", message: `annotation '${SIZE}' is dimensional, not geometric` };
  refuseNext = [422, mismatch];
  const r = await call("gdt_fcf_delete", { part_id: 5, annotation_id: SIZE });
  assert.equal(r.structuredContent?.reason, mismatch.message);
  assert.equal(r.structuredContent?.error_code, "annotation_kind_mismatch");
  const rule = { error: "datum_label_not_in_drf", message: "datum 'Q' is not designated on part 5" };
  refuseNext = [422, rule];
  const u = await call("gdt_fcf_update", { part_id: 5, annotation_id: FRAME, datum_refs: ["Q"] });
  assert.equal(u.isError, true);
  assert.equal(u.structuredContent?.reason, rule.message);
  assert.equal(u.structuredContent?.error_code, "datum_label_not_in_drf");
});

await check("(g5d) a catalogue body keeps its sentence as the reason and its typed fields", async () => {
  const body = { success: false, error_code: "invalid_parameter", error: "label must not be empty", retryable: false, hint: "h" };
  refuseNext = [422, body];
  const r = await call("gdt_datum_update", { part_id: 5, label: "A", new_label: "B" });
  assert.equal(r.structuredContent?.reason, "label must not be empty");
  assert.equal(r.structuredContent?.error_code, "invalid_parameter");
  assert.equal(r.structuredContent?.hint, "h");
});

// ─── (g6) disconnection gate: reachable from production ───────────────────────

await check("(g6) each tool is benched, exposed, and dispatched by invoke", async () => {
  const full = new Set(exposedNamesFor(table, "full"));
  assert.ok(SWITCHABLE_BENCHES.includes("drawing"), "drawing is a switchable bench");
  for (const name of NEW_TOOLS) {
    assert.ok(table.has(name), `${name} registered`);
    assert.equal(metaFor(name).bench, "drawing", `${name} on the drawing bench`);
    assert.ok(full.has(name), `${name} on the full surface`);
  }
  const invoke = table.get("invoke");
  const routes = [
    ["gdt_fcf_update", { part_id: 5, annotation_id: FRAME, basic: [-20, 10] }, "PATCH", `/api/agent/parts/5/fcf/${FRAME}`],
    ["gdt_fcf_delete", { part_id: 5, annotation_id: FRAME }, "DELETE", `/api/agent/parts/5/fcf/${FRAME}`],
    ["gdt_datum_update", { part_id: 5, label: "A", new_label: "C" }, "PATCH", "/api/agent/parts/5/datums/A"],
    ["gdt_datum_delete", { part_id: 5, label: "A" }, "DELETE", "/api/agent/parts/5/datums/A"],
    ["gdt_dimensional_delete", { part_id: 5, annotation_id: SIZE }, "DELETE", `/api/agent/parts/5/size-tolerance/${SIZE}`],
  ];
  for (const [name, args, method, url] of routes) {
    seen = [];
    const r = await invoke.handler({ name, args }, {});
    assert.notEqual(r.isError, true, `${name} via invoke: ${r.content?.[0]?.text}`);
    assert.deepEqual(
      seen.map((s) => [s.method, s.url]),
      [[method, url]],
      `invoke(${name}) reached its route`,
    );
  }
});

// ─── (g7) discoverable through find_tool ──────────────────────────────────────

await check("(g7) find_tool ranks each tool first for an agent's words", async () => {
  const find = table.get("find_tool");
  const cases = [
    ["delete the feature control frame", "gdt_fcf_delete"],
    ["remove the wrong position callout", "gdt_fcf_delete"],
    ["fix the sign of the basic dimension on the position FCF", "gdt_fcf_update"],
    ["edit the tolerance of a feature control frame", "gdt_fcf_update"],
    ["delete datum B", "gdt_datum_delete"],
    ["re-letter datum A to C", "gdt_datum_update"],
    ["move datum A to another face", "gdt_datum_update"],
    ["delete the size tolerance on the bore", "gdt_dimensional_delete"],
  ];
  const misses = [];
  for (const [query, expected] of cases) {
    const r = await find.handler({ query }, {});
    const names = (textJson(r).results ?? textJson(r).matches ?? []).map((m) => m.name);
    if (names[0] !== expected) misses.push(`${query} → expected ${expected}, got [${names.slice(0, 3).join(", ")}]`);
  }
  assert.deepEqual(misses, [], misses.join("\n"));
});

// ─── (g8) the handles reach the agent from the report ─────────────────────────

await check("(g8) gdt_report hands the agent each annotation_id and its kind", async () => {
  const r = await call("gdt_report", { part_id: 5 });
  assert.notEqual(r.isError, true, r.content[0].text);
  const byId = Object.fromEntries(textJson(r).annotations.map((a) => [a.annotation_id, a.kind]));
  assert.deepEqual(byId, { [FRAME]: "geometric", [SIZE]: "dimensional" },
    "the frame id (gdt_fcf_update / _delete) and the size id (gdt_dimensional_delete)");
});

stub.close();
console.log(`gdt_edit: ${passed}/15 passed`);
