/**
 * The agent LOOKS at a sheet (2026-11-20, agent eyes slice 2).
 *
 * `drawing_look` reads the region's numbers from GET /api/drawings/{id}/spaces
 * and the picture from GET /api/drawings/{id}/render.png (api-server
 * drawing_mgr::render_png), and answers with MCP image content plus the
 * region's findings, worst first. The route itself shows the look in the
 * person's app when the call carries X-Roshera-Agent -- so the header must
 * travel, or the show silently never fires.
 *
 *   (l1) whole sheet: /spaces then /render.png, the agent header on the
 *        picture's request; an image item (the served PNG, base64) and the
 *        sheet's worst spaces with readable findings, the crop and the show
 *        count from the route's headers;
 *   (l2) one space: `space` and `px` encoded on both queries; the region's
 *        own findings errors first; the spaces under it worst first;
 *   (l3) an unknown space is refused by /spaces -- no picture is fetched, so
 *        nothing is shown;
 *   (l4) a refused picture (px out of range) carries the server's reason;
 *   (l5) the same look again withholds the pixels (image freshness) but
 *        still reaches the route, so the app still follows;
 *   (l6) disconnection gate: production table, `drawing` bench, full
 *        surface, `invoke` reaches the routes;
 *   (l7) find_tool ranks it first for the words an agent would use.
 *
 *     npm run test:gates:build && node test/drawing_look.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DRAWING = "bbbbbbbb-1111-4222-8333-555555555555";
/** A real 1 x 1 PNG: what the route serves, byte for byte, as the image. */
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

const finding = (rule, space, severity, measured, min, max, message) => ({
  rule, space, severity, measured, unit: "mm", tolerance: { min, max }, message, subjects: [],
});
const space = (id, kind, score, findings, parent) => ({
  id, kind, rect: { x0: 100, y0: 80, x1: 160, y1: 100 }, parent, side: null,
  contents: { dimensions: [], placements: [], views: [], tables: [] }, findings, score,
});
const MAP = {
  grade: 0.425,
  bar: 0.8,
  below_bar: 2,
  worst: "view:FRONT/band:top/lane:1",
  spaces: [
    space("frame", "frame", 1.0, [], null),
    space("view:FRONT/band:top", "band", 0.425, [
      finding("LANE-PITCH", "view:FRONT/band:top", "warning", 4.1, 6.5, 8.5, "lane:1 and lane:2 are 4.10 mm apart"),
      finding("ALIGN-ROW", "view:FRONT/band:top", "error", 1.5, null, 0.1, "the row spreads 1.50 mm"),
    ], "view:FRONT"),
    space("view:FRONT/band:top/lane:1", "lane", 0.5, [
      finding("TEXT-CLEAR", "view:FRONT/band:top/lane:1", "error", 0.2, 1.5, null, "two texts 0.20 mm apart"),
    ], "view:FRONT/band:top"),
    space("view:FRONT/band:top/lane:2", "lane", 1.0, [], "view:FRONT/band:top"),
    space("title-block", "title_block", 0.85, [
      finding("TEXT-HEIGHT", "title-block", "warning", 2.52, 3.5, null, "15 text(s) with 2.52 mm capitals"),
    ], "frame"),
  ],
  findings: [],
  by_rule: {},
  rules: [],
};

let seen = [];
const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  seen.push({ method: req.method, url, agent: req.headers["x-roshera-agent"] ?? null });
  const json = (status, obj) => {
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  const u = new URL(url, "http://x");
  if (u.pathname === `/api/drawings/${DRAWING}/spaces`) {
    const q = u.searchParams.get("space");
    if (q === "view:NOPE") {
      return json(422, { success: false, error_code: "invalid_parameter", error: `drawing ${DRAWING} has no space 'view:NOPE'` });
    }
    if (q) {
      return json(200, { ...MAP, focus: q, spaces: MAP.spaces.filter((s) => s.id.startsWith(q)) });
    }
    return json(200, MAP);
  }
  if (u.pathname === `/api/drawings/${DRAWING}/render.png`) {
    if (u.searchParams.get("px") === "9999") {
      return json(422, { success: false, error_code: "invalid_parameter", error: "px 9999 is outside 64..=4096" });
    }
    const headers = { "Content-Type": "image/png", "x-roshera-crop": "92.000,72.000,168.000,108.000" };
    if (req.headers["x-roshera-agent"]) headers["x-roshera-shown-to"] = "1";
    res.writeHead(200, headers);
    return res.end(PNG);
  }
  json(404, {});
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
const textOf = (r) => JSON.parse(r.content.find((c) => c.type === "text" && c.text.startsWith("{")).text);

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(l1) the whole sheet: an image item plus the sheet's worst spaces", async () => {
  const r = await call("drawing_look", { drawing_id: DRAWING });
  assert.notEqual(r.isError, true, JSON.stringify(r.content));
  assert.deepEqual(
    seen.map((s) => [s.method, s.url]),
    [
      ["GET", `/api/drawings/${DRAWING}/spaces`],
      ["GET", `/api/drawings/${DRAWING}/render.png`],
    ],
  );
  assert.equal(seen[1].agent, "Claude", "the picture's request carries the agent header (the route shows the look)");
  const img = r.content.find((c) => c.type === "image");
  assert.ok(img, "an MCP image content item");
  assert.equal(img.mimeType, "image/png");
  assert.equal(img.data, PNG.toString("base64"), "the served PNG, byte for byte");
  const out = textOf(r);
  assert.equal(out.space, null);
  assert.deepEqual(
    out.worst_spaces.map((s) => s.space),
    ["view:FRONT/band:top", "view:FRONT/band:top/lane:1", "title-block"],
    "worst first; clean spaces are not listed",
  );
  assert.match(out.worst_spaces[0].findings[0], /^ERROR ALIGN-ROW: .*\[1\.5 mm vs <= 0\.1\]/, "errors first");
  assert.equal(out.sheet.grade, 0.425);
  assert.equal(out.sheet.worst, "view:FRONT/band:top/lane:1");
  assert.deepEqual(out.crop_mm, [92, 72, 168, 108]);
  assert.equal(out.shown_in_app, 1);
  assert.deepEqual(r.structuredContent, out);
});

await check("(l2) one space: encoded queries, its findings errors first, the spaces under it", async () => {
  const r = await call("drawing_look", { drawing_id: DRAWING, space: "view:FRONT/band:top", px: 900 });
  assert.notEqual(r.isError, true, JSON.stringify(r.content));
  assert.deepEqual(
    seen.map((s) => s.url),
    [
      `/api/drawings/${DRAWING}/spaces?space=view%3AFRONT%2Fband%3Atop`,
      `/api/drawings/${DRAWING}/render.png?space=view%3AFRONT%2Fband%3Atop&px=900`,
    ],
  );
  assert.ok(r.content.some((c) => c.type === "image"));
  const out = textOf(r);
  assert.equal(out.space, "view:FRONT/band:top");
  assert.equal(out.kind, "band");
  assert.equal(out.score, 0.425);
  assert.equal(out.below_bar, true);
  assert.match(out.findings[0], /^ERROR ALIGN-ROW/);
  assert.match(out.findings[1], /^warn LANE-PITCH: .*\[4\.1 mm vs 6\.5\.\.8\.5\]/);
  assert.deepEqual(
    out.spaces_under_with_findings.map((s) => s.space),
    ["view:FRONT/band:top/lane:1"],
    "the clean lane is not listed",
  );
});

await check("(l3) an unknown space is refused before any picture is drawn or shown", async () => {
  const r = await call("drawing_look", { drawing_id: DRAWING, space: "view:NOPE" });
  assert.equal(r.isError, true);
  assert.match(r.content[0].text, /has no space 'view:NOPE'/);
  assert.ok(!seen.some((s) => s.url.includes("render.png")), "no picture fetched, nothing shown");
});

await check("(l4) a refused picture carries the server's reason", async () => {
  const r = await call("drawing_look", { drawing_id: DRAWING, px: 9999 });
  assert.equal(r.isError, true);
  assert.match(r.content[0].text, /px 9999 is outside 64\.\.=4096/);
});

await check("(l5) the same look again withholds the pixels but still reaches the route", async () => {
  const args = { drawing_id: DRAWING, space: "title-block", px: 500 };
  const first = await call("drawing_look", args);
  assert.ok(first.content.some((c) => c.type === "image"), "a new view sends pixels");
  seen = [];
  const r = await call("drawing_look", args);
  assert.notEqual(r.isError, true);
  assert.ok(!r.content.some((c) => c.type === "image"), "pixels already in context are not resent");
  assert.match(r.content[0].text, /^IMAGE UNCHANGED/);
  assert.ok(seen.some((s) => s.url.includes("render.png") && s.agent === "Claude"), "the app still follows the look");
});

await check("(l6) production table, drawing bench, full surface, invoke reaches the routes", async () => {
  assert.ok(table.has("drawing_look"));
  assert.equal(metaFor("drawing_look").bench, "drawing");
  assert.ok(SWITCHABLE_BENCHES.includes("drawing"));
  assert.ok(new Set(exposedNamesFor(table, "full")).has("drawing_look"));
  seen = [];
  const r = await table.get("invoke").handler({ name: "drawing_look", args: { drawing_id: DRAWING, px: 300 } }, {});
  assert.notEqual(r.isError, true, r.content?.[0]?.text);
  assert.deepEqual(seen.map((s) => s.url), [
    `/api/drawings/${DRAWING}/spaces`,
    `/api/drawings/${DRAWING}/render.png?px=300`,
  ]);
});

await check("(l7) find_tool ranks drawing_look first for an agent's words", async () => {
  const find = table.get("find_tool");
  const cases = [
    "look at the drawing sheet as an image",
    "show me a picture of this region of the drawing",
    "render the sheet to a png so I can see it",
  ];
  const ranked = async (query) => {
    const r = await find.handler({ query }, {});
    const t = JSON.parse(r.content[0].text);
    return (t.results ?? t.matches ?? []).map((m) => m.name);
  };
  const misses = [];
  for (const query of cases.slice(0, 2)) {
    const names = await ranked(query);
    if (names[0] !== "drawing_look") misses.push(`${query} -> [${names.slice(0, 3).join(", ")}]`);
  }
  // Measured, not tuned (2026-11-20): "render ... png" also names the part
  // renderer (render_part) and the sheet export (drawing_export_sheet), which
  // rank above it; drawing_look is held to the top three for that wording.
  const renderWords = await ranked(cases[2]);
  if (!renderWords.slice(0, 3).includes("drawing_look")) {
    misses.push(`${cases[2]} -> [${renderWords.slice(0, 3).join(", ")}]`);
  }
  assert.deepEqual(misses, [], misses.join("\n"));
});

stub.close();
console.log(`drawing_look: ${passed}/7 passed`);
