/**
 * The agent's HANDS on a sheet (2026-11-20, agent eyes slice 3).
 *
 * Six tools, one layout verb each, all on POST /api/drawings/{id}/layout
 * (api-server drawing_hands.rs). The server plans, gates, records and shows;
 * the tool sends the verb with the agent header (the route shows the act and
 * the judgement only for an agent's call) and answers the space-map delta
 * compactly.
 *
 *   (h1) every tool reaches the route with its verb and arguments, the agent
 *        header on the request; absent optional arguments are not sent;
 *   (h2) the answer: what was done (the server's caption), grade and below-bar
 *        as "before -> after", each changed space with before/after and
 *        whether it got better, the issues created and resolved as lines, the
 *        certificate, how many apps saw it; the whole response in
 *        structuredContent;
 *   (h3) a refusal (409 layout_verb_refused) comes back isError with the
 *        server's sentence as the top-level `reason` and its code in
 *        `details.reason`;
 *   (h4) disconnection gate: production table, `drawing` bench, full surface,
 *        `invoke` reaches the route;
 *   (h5) find_tool finds each verb for the words an agent would use.
 *
 *     npm run test:gates:build && node test/drawing_hands.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DRAWING = "cccccccc-1111-4222-8333-555555555555";

const ANSWER = {
  success: true,
  drawing_id: DRAWING,
  verb: "reattach_leader",
  done: "re-routed the leader of '28.5°' on FRONT: 62 mm -> 20 mm at 55 degrees",
  caption:
    "re-routed the leader of '28.5°' on FRONT: 62 mm -> 20 mm at 55 degrees -- fixes LEADER-ANGLE: the leader of '28.5°' meets its shoulder at 14.1 degrees",
  spaces_changed: [
    { space: "view:FRONT/band:top", before: 1.0, after: 0.85, rect: { x0: 1, y0: 2, x1: 3, y1: 4 } },
    { space: "view:FRONT/band:right", before: 0.72, after: 1.0, rect: { x0: 5, y0: 6, x1: 7, y1: 8 } },
  ],
  grade_before: 0.3205770883,
  grade_after: 0.425,
  below_bar_before: 7,
  below_bar_after: 6,
  issues_created: [
    { rule: "TEXT-HEIGHT", space: "view:FRONT/band:top", severity: "warning", message: "1 text(s) with 2.52 mm capitals" },
  ],
  issues_resolved: [
    { rule: "LEADER-ANGLE", space: "view:FRONT/band:right", severity: "warning", message: "the leader of '28.5°' meets its shoulder at 14.1 degrees" },
  ],
  certificate: { sound: true, complete: true, counts: {}, omission_summary: [] },
  shown_to: 1,
};

let seen = [];
let refuseNext = false;
const stub = http.createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const url = req.url ?? "";
    seen.push({
      method: req.method,
      url,
      agent: req.headers["x-roshera-agent"] ?? null,
      body: raw ? JSON.parse(raw) : null,
    });
    const json = (status, obj) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "POST" && url === `/api/drawings/${DRAWING}/layout`) {
      if (refuseNext) {
        refuseNext = false;
        return json(409, {
          success: false,
          error_code: "layout_verb_refused",
          error: "the sheet would no longer state '48' (length on FRONT); nothing was changed",
          retryable: false,
          details: { gate: "layout_verb", reason: "drops_value", verb: "remove_view", drawing_id: DRAWING },
        });
      }
      const body = raw ? JSON.parse(raw) : {};
      return json(200, { ...ANSWER, verb: body.verb });
    }
    json(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable, exposedNamesFor } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { metaFor } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);

const table = buildTable();
const call = (name, args = {}) => {
  const entry = table.get(name);
  assert.ok(entry, `${name} is registered in the production table`);
  return entry.handler(args, {});
};
const textOf = (r) => JSON.parse(r.content[0].text);

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

const TOOLS = [
  ["drawing_move_dimension", { dimension: "d20", lane: 4 }, { verb: "move_dimension", dimension: "d20", lane: 4 }],
  ["drawing_move_dimension", { dimension: "d23", band: "top", offset_mm: -3.5 }, { verb: "move_dimension", dimension: "d23", band: "top", offset_mm: -3.5 }],
  ["drawing_align", { dimensions: ["d20", "d24"] }, { verb: "align", dimensions: ["d20", "d24"] }],
  ["drawing_move_view", { view: "RIGHT", to: [210.5, 103.3] }, { verb: "move_view", view: "RIGHT", to: [210.5, 103.3] }],
  ["drawing_move_view", { view: "ISOMETRIC", align_with: "TOP" }, { verb: "move_view", view: "ISOMETRIC", align_with: "TOP" }],
  ["drawing_reattach_leader", { dimension: "fcf:2", target: "nearest" }, { verb: "reattach_leader", dimension: "fcf:2", target: "nearest" }],
  ["drawing_reattach_leader", { dimension: "d27", side: "left" }, { verb: "reattach_leader", dimension: "d27", side: "left" }],
  ["drawing_remove_view", { view: "RIGHT" }, { verb: "remove_view", view: "RIGHT" }],
  ["drawing_remove_dimension", { dimension: "d19", by: "agent", reason: "stated in the section" }, { verb: "remove_dimension", dimension: "d19", by: "agent", reason: "stated in the section" }],
];

await check("(h1) every tool reaches the layout route with its verb, and the agent header", async () => {
  for (const [name, args, body] of TOOLS) {
    seen = [];
    const r = await call(name, { drawing_id: DRAWING, ...args });
    assert.notEqual(r.isError, true, `${name}: ${r.content?.[0]?.text}`);
    assert.equal(seen.length, 1, `${name}: one request`);
    assert.equal(seen[0].method, "POST");
    assert.equal(seen[0].url, `/api/drawings/${DRAWING}/layout`);
    assert.equal(seen[0].agent, "Claude", `${name}: the route shows an agent's verb`);
    assert.deepEqual(seen[0].body, body, `${name}: the verb and only the arguments given`);
  }
});

await check("(h2) the answer is the space-map delta, compact, worst first as served", async () => {
  const r = await call("drawing_reattach_leader", { drawing_id: DRAWING, dimension: "d27" });
  const out = textOf(r);
  assert.equal(out.verb, "reattach_leader");
  assert.match(out.done, /fixes LEADER-ANGLE/, "the caption names the rule");
  assert.equal(out.grade, "0.321 -> 0.425");
  assert.equal(out.below_bar, "7 -> 6");
  assert.deepEqual(
    out.spaces_changed.map((s) => [s.space, s.before, s.after, s.better]),
    [
      ["view:FRONT/band:top", 1, 0.85, false],
      ["view:FRONT/band:right", 0.72, 1, true],
    ],
  );
  assert.match(out.issues_created[0], /^warn TEXT-HEIGHT in view:FRONT\/band:top: /);
  assert.match(out.issues_resolved[0], /^warn LEADER-ANGLE in view:FRONT\/band:right: /);
  assert.deepEqual(out.certificate, { sound: true, complete: true });
  assert.equal(out.shown_in_app, 1);
  assert.match(out.note, /created issues/, "a created issue is called out");
  assert.deepEqual(r.structuredContent.response.spaces_changed, ANSWER.spaces_changed, "the whole response rides along");
});

await check("(h3) a refusal carries the server's sentence as the top-level reason", async () => {
  refuseNext = true;
  const r = await call("drawing_remove_view", { drawing_id: DRAWING, view: "FRONT" });
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent.reason, "the sheet would no longer state '48' (length on FRONT); nothing was changed");
  assert.equal(r.structuredContent.error_code, "layout_verb_refused");
  assert.equal(r.structuredContent.details.reason, "drops_value");
  assert.equal(r.structuredContent.refusal, "drops_value", "the stable code beside the sentence");
  assert.match(r.content[0].text, /no longer state '48'/);
});

await check("(h4) production table, drawing bench, full surface, invoke reaches the route", async () => {
  const names = TOOLS.map((t) => t[0]);
  const full = new Set(exposedNamesFor(table, "full"));
  for (const name of new Set(names)) {
    assert.ok(table.has(name), name);
    assert.equal(metaFor(name).bench, "drawing", name);
    assert.ok(full.has(name), `${name} in the full surface`);
  }
  seen = [];
  const r = await table
    .get("invoke")
    .handler({ name: "drawing_remove_view", args: { drawing_id: DRAWING, view: "RIGHT" } }, {});
  assert.notEqual(r.isError, true, r.content?.[0]?.text);
  assert.deepEqual(seen.map((s) => [s.method, s.url]), [["POST", `/api/drawings/${DRAWING}/layout`]]);
});

await check("(h5) find_tool finds each verb for an agent's words", async () => {
  const find = table.get("find_tool");
  const ranked = async (query) => {
    const r = await find.handler({ query }, {});
    const t = JSON.parse(r.content[0].text);
    return (t.results ?? t.matches ?? []).map((m) => m.name);
  };
  const cases = [
    ["move this dimension to another lane of the view", "drawing_move_dimension"],
    ["align these dimensions on one line", "drawing_align"],
    ["move a view on the drawing sheet", "drawing_move_view"],
    ["re-route the leader of the feature control frame", "drawing_reattach_leader"],
    ["remove the right view from the drawing", "drawing_remove_view"],
    ["remove a duplicate dimension from the sheet", "drawing_remove_dimension"],
  ];
  // Measured, not tuned (2026-11-20): each verb in the top three for its
  // wording. "line up these dimensions on one line" ranks gdt_dimensional_delete,
  // drawing_move_dimension, drawing_set_dimension above drawing_align (the
  // word "dimensions" is spread over many tools); an agent says "align".
  const misses = [];
  for (const [query, want] of cases) {
    const names = await ranked(query);
    if (!names.slice(0, 3).includes(want)) misses.push(`${query} -> [${names.slice(0, 3).join(", ")}]`);
  }
  assert.deepEqual(misses, [], misses.join("\n"));
});

await check("(h6) out of a sheet's context the part's verbs outrank the sheet verbs", async () => {
  const find = table.get("find_tool");
  const ranked = async (query) => {
    const r = await find.handler({ query }, {});
    const t = JSON.parse(r.content[0].text);
    return (t.results ?? t.matches ?? []).map((m) => m.name);
  };
  // No sheet in the query: the part's verb first, though the layout verbs'
  // names carry "move", "dimension" and "view".
  for (const [query, want] of [
    ["move the part 10mm to the left", "transform"],
    ["what are the overall dimensions of this part", "dimension_part"],
    ["give me a sectioned view", "section_view"],
  ]) {
    const names = await ranked(query);
    assert.equal(names[0], want, `${query} -> [${names.slice(0, 3).join(", ")}]`);
  }
  // The score factor itself (not only the name rarity): without it two sheet
  // verbs, sharing "size" / "dimension" vocabulary, push kb_lookup out of the
  // top five for a fastener question that names no sheet.
  const bolt = await ranked("what clearance hole size does an M8 bolt need");
  assert.ok(bolt.includes("kb_lookup"), bolt.join(", "));
  assert.ok(!bolt.some((n) => n === "drawing_remove_dimension"), bolt.join(", "));
  // A sheet in the query: the layout verb is a candidate again.
  const onSheet = await ranked("move this dimension on the sheet to another lane");
  assert.equal(onSheet[0], "drawing_move_dimension", onSheet.join(", "));
  // A bench filter is the context itself.
  const r = await find.handler({ query: "move the part 10mm to the left", bench: "drawing" }, {});
  const t = JSON.parse(r.content[0].text);
  const names = (t.results ?? t.matches ?? []).map((m) => m.name);
  assert.ok(names[0]?.startsWith("drawing_move_"), `${names.join(", ")} | ${r.content[0].text.slice(0, 200)}`);
});

stub.close();
console.log(`drawing_hands: ${passed}/6 passed`);
