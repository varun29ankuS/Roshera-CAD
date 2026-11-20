/**
 * The agent sees a sheet as its spaces (2026-11-20, agent eyes slice 1).
 *
 * `drawing_spaces` reads GET /api/drawings/{id}/spaces (api-server
 * drawing_mgr::drawing_spaces) and answers with a compact summary -- the
 * grade (the WORST space), below_bar, the spaces below a perfect score worst
 * first, each with its findings as readable lines -- plus the whole map.
 *
 *   (s1) the tool hits the route; `space` goes on the query string, encoded;
 *   (s2) the summary orders spaces worst first and carries each finding's
 *        rule, measured value and tolerance; the full map rides beside it;
 *   (s3) a refused space (422 invalid_parameter) comes back as an error with
 *        the server's reason;
 *   (s4) disconnection gate: the tool is in the PRODUCTION table, benched
 *        `drawing`, on the full surface, and `invoke` dispatches
 *        it to the route;
 *   (s5) find_tool ranks it first for the words an agent would use.
 *
 *     npm run test:gates:build && node test/drawing_spaces.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DRAWING = "aaaaaaaa-1111-4222-8333-444444444444";

/** drawing::spaces::SpaceMap as the route serialises it, trimmed. */
const finding = (rule, space, severity, measured, min, max, message) => ({
  rule, space, severity, measured, unit: "mm", tolerance: { min, max }, message, subjects: [],
});
const MAP = {
  grade: 0.5,
  bar: 0.8,
  below_bar: 2,
  worst: "view:FRONT/band:top/lane:1",
  spaces: [
    { id: "frame", kind: "frame", rect: { x0: 20, y0: 10, x1: 410, y1: 287 }, parent: null, side: null,
      contents: { dimensions: [], placements: [], views: [], tables: [] }, findings: [], score: 1.0 },
    { id: "view:FRONT/band:top", kind: "band", rect: { x0: 100, y0: 80, x1: 160, y1: 100 }, parent: "view:FRONT", side: "top",
      contents: { dimensions: [], placements: [], views: [], tables: [] },
      findings: [finding("LANE-PITCH", "view:FRONT/band:top", "warning", 13, 6.5, 8.5, "lane:1 and lane:2 are 13.00 mm apart")],
      score: 0.85 },
    { id: "view:FRONT/band:top/lane:1", kind: "lane", rect: { x0: 100, y0: 85, x1: 160, y1: 90 }, parent: "view:FRONT/band:top", side: "top",
      contents: { dimensions: ["d1", "d2"], placements: [0, 1], views: [], tables: [] },
      findings: [finding("ALIGN-ROW", "view:FRONT/band:top/lane:1", "error", 1.5, null, 0.1, "the 2 dimension lines of this row spread 1.50 mm")],
      score: 0.5 },
    { id: "title-block", kind: "title_block", rect: { x0: 225, y0: 239, x1: 410, y1: 287 }, parent: "frame", side: null,
      contents: { dimensions: [], placements: [], views: [], tables: [] },
      findings: [finding("TEXT-HEIGHT", "title-block", "warning", 2.52, 3.5, null, "15 text(s) with 2.52 mm capitals"),
        finding("TRAILING-ZERO", "title-block", "warning", 1, null, 0, "1 value(s) with trailing zeros")],
      score: 0.7225 },
  ],
  findings: [],
  by_rule: { "ALIGN-ROW": [1, 0], "LANE-PITCH": [0, 1], "TEXT-HEIGHT": [0, 1], "TRAILING-ZERO": [0, 1] },
  rules: [],
};

/** A big sheet: 40 spaces with 8 warnings each, as a dense real sheet has. */
const BIG = {
  ...MAP,
  spaces: Array.from({ length: 40 }, (_, i) => ({
    id: `view:FRONT/band:left/lane:${i + 1}`, kind: "lane", rect: { x0: 0, y0: 0, x1: 1, y1: 1 },
    parent: "view:FRONT/band:left", side: "left",
    contents: { dimensions: ["d"], placements: [i], views: [], tables: [] },
    findings: Array.from({ length: 8 }, (_, k) =>
      finding("LEADER-ANGLE", `view:FRONT/band:left/lane:${i + 1}`, "warning", 10 + k, 30, 60, "the leader meets its shoulder at a shallow angle")),
    score: 0.272 + i * 0.001,
  })),
};

let seen = [];
const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (status, obj) => {
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  if (url.startsWith(`/api/drawings/${DRAWING}/spaces`)) {
    seen.push({ method: req.method, url });
    const q = new URL(url, "http://x").searchParams.get("space");
    if (q === "view:NOPE") {
      return send(422, {
        success: false,
        error_code: "invalid_parameter",
        error: `drawing ${DRAWING} has no space 'view:NOPE'`,
        retryable: false,
      });
    }
    if (q === "big") return send(200, BIG);
    if (q) {
      return send(200, { ...MAP, focus: q, spaces: MAP.spaces.filter((s) => s.id.startsWith(q)) });
    }
    return send(200, MAP);
  }
  send(404, {});
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable, exposedNamesFor } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { metaFor } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);
const { SWITCHABLE_BENCHES } = await import(pathToFileURL(join(HERE, ".build", "workbench.js")).href);

const table = buildTable();
const call = (name, args = {}) => {
  const entry = table.get(name);
  assert.ok(entry, `${name} is registered in the production table`);
  return entry.handler(args, {});
};
const textJson = (r) => JSON.parse(r.content[0].text);

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(s1) drawing_spaces GETs the route; `space` is encoded on the query", async () => {
  const r = await call("drawing_spaces", { drawing_id: DRAWING });
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.deepEqual(seen, [{ method: "GET", url: `/api/drawings/${DRAWING}/spaces` }]);
  seen = [];
  const f = await call("drawing_spaces", { drawing_id: DRAWING, space: "view:FRONT/band:top" });
  assert.notEqual(f.isError, true, f.content[0].text);
  assert.deepEqual(seen, [
    { method: "GET", url: `/api/drawings/${DRAWING}/spaces?space=view%3AFRONT%2Fband%3Atop` },
  ]);
  assert.equal(textJson(f).focus, "view:FRONT/band:top");
});

await check("(s2) worst spaces first, each finding readable, the whole map beside", async () => {
  const r = await call("drawing_spaces", { drawing_id: DRAWING });
  const out = textJson(r);
  assert.equal(out.grade, 0.5);
  assert.equal(out.below_bar, 2);
  assert.equal(out.worst, "view:FRONT/band:top/lane:1");
  assert.deepEqual(
    out.worst_spaces.map((s) => s.space),
    ["view:FRONT/band:top/lane:1", "title-block", "view:FRONT/band:top"],
    "worst first; the clean frame is not listed",
  );
  assert.equal(out.clean_spaces, 1);
  assert.deepEqual(out.worst_spaces.map((s) => s.below_bar), [true, true, false]);
  const line = out.worst_spaces[0].findings[0];
  assert.match(line, /^ERROR ALIGN-ROW: /);
  assert.match(line, /\[1\.5 mm vs <= 0\.1\]/);
  assert.match(out.worst_spaces[2].findings[0], /LANE-PITCH.*\[13 mm vs 6\.5\.\.8\.5\]/);
  assert.match(out.headline, /grade 0\.5 .*2 of 4 spaces below the bar 0\.8/);
  assert.equal(out.map, undefined, "the text the agent reads stays compact");
  assert.deepEqual(r.structuredContent?.map, MAP, "the server's map, whole, as structured content");
  assert.equal(r.structuredContent?.grade, 0.5);
  assert.deepEqual(out.by_rule, MAP.by_rule);
});

await check("(s2b) a dense sheet's text is bounded: 12 spaces, 5 findings each, the rest counted", async () => {
  const r = await call("drawing_spaces", { drawing_id: DRAWING, space: "big" });
  const out = textJson(r);
  assert.equal(out.worst_spaces.length, 12);
  assert.equal(out.more_spaces_with_findings, 28);
  assert.equal(out.worst_spaces[0].findings.length, 6);
  assert.equal(out.worst_spaces[0].findings[5], "+3 more");
  assert.ok(r.content[0].text.length < 12000, `text is ${r.content[0].text.length} chars`);
  assert.equal(r.structuredContent?.map?.spaces?.length, 40, "the whole map stays reachable");
});

await check("(s3) a refused space carries the server's reason", async () => {
  const r = await call("drawing_spaces", { drawing_id: DRAWING, space: "view:NOPE" });
  assert.equal(r.isError, true);
  assert.match(r.content[0].text, /has no space 'view:NOPE'/);
});

await check("(s4) production table, drawing bench, full surface, invoke reaches the route", async () => {
  assert.ok(table.has("drawing_spaces"));
  assert.equal(metaFor("drawing_spaces").bench, "drawing");
  assert.ok(SWITCHABLE_BENCHES.includes("drawing"));
  assert.ok(new Set(exposedNamesFor(table, "full")).has("drawing_spaces"));
  seen = [];
  const r = await table.get("invoke").handler({ name: "drawing_spaces", args: { drawing_id: DRAWING } }, {});
  assert.notEqual(r.isError, true, r.content?.[0]?.text);
  assert.deepEqual(seen.map((s) => [s.method, s.url]), [["GET", `/api/drawings/${DRAWING}/spaces`]]);
});

await check("(s5) find_tool ranks drawing_spaces first for an agent's words", async () => {
  const find = table.get("find_tool");
  const cases = [
    "which spaces of the sheet are worst",
    "score the drawing layout space by space",
    "are the dimension rows evenly pitched on the sheet",
  ];
  const misses = [];
  for (const query of cases) {
    const r = await find.handler({ query }, {});
    const names = (textJson(r).results ?? textJson(r).matches ?? []).map((m) => m.name);
    if (names[0] !== "drawing_spaces") misses.push(`${query} -> [${names.slice(0, 3).join(", ")}]`);
  }
  assert.deepEqual(misses, [], misses.join("\n"));
});

stub.close();
console.log(`drawing_spaces: ${passed}/6 passed`);
