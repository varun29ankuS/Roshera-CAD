/**
 * DRAWING EXPORT TRUTH — the exported sheet is scored against what a
 * MACHINIST actually receives (SVG/DXF bytes from `GET /api/drawings/{id}/svg`
 * and `/dxf`), not against the kernel's internal model of the drawing.
 *
 * Three real defects shipped in this pipeline and NO existing eval would have
 * caught any of them (09-drawing-perf scores speed; 15-drawing-comprehension
 * scores whether an agent reads a certified sheet honestly — neither looks at
 * the artifact's own ink):
 *
 *   1. EXACTNESS    — a bore drawn as a 96-vertex polyline instead of a real
 *                      circle: not CAM-recognisable as a hole.
 *   2. SILHOUETTE    — a cylinder's FRONT view open on one side: a seam edge
 *                      at x=+R, nothing at x=-R. Fixed 2026-08-16/17 by
 *                      `415c3a98` (exact curves) and `11234d7b` (synthesised
 *                      silhouettes, seam suppressed).
 *   3. SECTION       — SECTION A-A rendered as disconnected hatched
 *                      rectangles, correct topology, unusable. Fixed by
 *                      `cbef4dda`/`81b14736`.
 *   4. LEGIBILITY    — (not a shipped defect yet, but the same class of
 *                      risk) two annotation labels drawn on top of each
 *                      other are as unusable as missing geometry.
 *
 * # Why the oracle parses raw artifact TEXT, not a JS object
 *
 * `run` fetches the live SVG/DXF bytes and hands them to `oracle` UNPARSED.
 * The parsing (view-group extraction, polyline/circle/text regexes) lives
 * INSIDE the pure oracle, not in `run` — if the extractor lived in `run` it
 * would never be exercised by `test/oracle-19.mjs`'s offline mutation proof,
 * and a parser bug (e.g. counting a centerline as silhouette ink) could
 * produce a false green on exactly the check this scenario exists to hold
 * red. Feeding raw bytes means the proof covers parse+judge together, the
 * same discipline 15's `oracle-15.mjs` uses for its DrawingAnswer JSON.
 *
 * The honest fixture in `test/oracle-19.mjs` is hand-built in the markup shape
 * `geometry-engine/src/drawing/svg.rs`'s `render_view` / `render_view_labels`
 * and `dxf.rs`'s DXF writer emit — this is the same "reference source"
 * discipline scenario 18 uses for its closed-form volumes.
 *
 * That claim used to say "verified by reading both files directly, not
 * guessed", and READING THE SOURCE WAS NOT ENOUGH. The section fixture it
 * produced modelled hatch as closed rectangles and the outline as one chained
 * polyline; the renderer emits 45-degree hatch LINES and one 2-point polyline
 * per outline edge. That mismatch is what let an unsatisfiable check look
 * satisfiable for as long as it did. The section half of the fixture is now
 * derived from RENDERED OUTPUT — `target/drawing-visual-harness/harness_
 * {flange,ring_plate}.svg` — and the re-stated section property is confirmed
 * by running this oracle against those sheets, not only against the fixture.
 *
 * FRONT still carries one chained polyline where the renderer would emit
 * per-edge segments. Left as is deliberately: `frontInkSegments` flattens
 * polylines to segments before judging, so the two shapes are equivalent for
 * every check that reads it, and re-authoring S1/S2's vertex-drop mutations
 * would buy nothing.
 *
 * # Part under test
 *
 * `buildHubFlange` (lib/builders.mjs): revolve profile
 * `[[6,0],[30,0],[30,6],[12,6],[12,20],[6,20]]` around Z. That gives:
 *   - BORE_R = 6   — the axial through-bore (profile r=6 wall), a true
 *     cylindrical surface whose rim is a circle in any view along Z (TOP).
 *   - OD_R   = 30  — the base flange plate's outer radius, silhouette
 *     extreme in FRONT/RIGHT (a solid of revolution around Z).
 *   - PLATE_T = 6  — the base flange plate thickness (z 0..6): the vertical
 *     span the OD silhouette segment must carry at BOTH x=+30 and x=-30.
 *
 * # No known-red — and the history of that flag is the point
 *
 * This scenario carried a known-red twice, and BOTH reasons turned out to be
 * false while the flag stayed true:
 *
 *   1. "the -OD silhouette check fails pending `feat/silhouette-edges`" —
 *      that branch merged and both extremes pass.
 *   2. "non-hatch geometry extends beyond the hatched cut faces' bounding
 *      box" — measured, and NO real sheet in this repo has ever satisfied it.
 *      A cut through the axis of a solid of revolution spans the part's whole
 *      silhouette, so the outline lands ON the hatch boundary by construction.
 *      The only thing that ever passed it was this suite's own hand-built
 *      fixture, which asserted the "beyond" case because it was authored that
 *      way and never had to agree with a rendered sheet.
 *
 * (2) is now repaired rather than softened: the property is re-stated as "the
 * SECTION's outline forms one connected run whose extent BOUNDS the hatched
 * cut faces", which is what the confetti defect actually violated, and the
 * fixture is rebuilt from measured markup. See the export block at the foot
 * of this file for the measurements and what is still owed.
 */
import { buildHubFlange } from "../lib/builders.mjs";

// ── Geometry ground truth, tied to buildHubFlange's revolve profile ───────
const BORE_R = 6; // profile point r=6 — the axial through-bore
const OD_R = 30; // profile point r=30 — the base flange plate's OD
const PLATE_T = 6; // z-span of the OD wall (0..6) — the vertical run each
// silhouette segment at x=+/-OD_R must carry.

const RADIUS_TOL = 0.05; // mm — kernel geometry is exact; a real circle's
// radius should match the analytic bore to float precision, not "close".
const SIL_TOL = 0.75; // mm — HLR/projection numerical slack for silhouette
// coordinate matching (looser than RADIUS_TOL: this is edge-endpoint
// geometry from an occlusion pipeline, not an analytic primitive).
const SIL_MIN_RUN = 1.0; // mm — a "vertical run" must span at least this
// much in Y to count as ink, not a stray near-degenerate fragment.

const GLYPH_WIDTH_RATIO = 0.6; // APPROXIMATION: average glyph advance width
// as a fraction of font-size for the sans-serif families svg.rs uses. This
// is NOT font metrics — it is a documented, deliberately coarse estimate.
// It is good enough to catch a full glyph-through-glyph overlap (the
// failure mode this check exists for) and not good enough to certify tight
// kerning-level spacing. Height uses a single alphabetic-baseline
// approximation (ascent 0.8*font above the anchor, descent 0.2*font below)
// applied uniformly — real per-class `dominant-baseline` values differ
// slightly (svg.rs sets some classes to `middle`), which this does not
// model. Text carrying a `rotate(...)` transform is SKIPPED — an
// axis-aligned bbox on rotated text would misjudge worse than it helps.
const ANNOTATION_FONT_MM = {
  // class -> { font: mm, anchor: 'start' | 'middle' }, read directly off
  // svg.rs's `<style>` block (`push_svg_style`). Scoped to per-part
  // ANNOTATION classes only (dimension/GD&T/hole callouts) — deliberately
  // excludes title-block/zone/logo furniture, which is fixed sheet
  // decoration this scenario cannot calibrate against without a live
  // server, and would be a pure false-positive surface if included blind.
  "dim-text": { font: 3.1, anchor: "middle" }, // always paired with dim-text-c
  label: { font: 3.6, anchor: "start" },
  "hole-tag": { font: 2.6, anchor: "middle" },
  "cutting-plane-label": { font: 3.6, anchor: "middle" },
  "gdt-datum-label": { font: 2.6, anchor: "middle" },
  "gdt-fcf-text": { font: 2.6, anchor: "start" },
  "datum-marker-label": { font: 2.6, anchor: "start" },
};

// ── Tiny attribute/markup parsing (mirrors svg.rs's emitted shapes) ───────

function parseAttrs(attrStr) {
  const attrs = {};
  const re = /([\w:-]+)="([^"]*)"/g;
  let m;
  while ((m = re.exec(attrStr))) attrs[m[1]] = m[2];
  return attrs;
}

function unescapeXml(s) {
  return String(s ?? "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&amp;/g, "&");
}

/** Split the whole SVG document into its `<g class="view" ...>...</g>` blocks. */
function parseViews(svg) {
  const re = /<g class="view" data-view-id="([^"]*)" data-projection="([^"]*)"[^>]*>([\s\S]*?)<\/g>/g;
  const views = [];
  let m;
  while ((m = re.exec(svg))) {
    views.push({ id: m[1], projection: m[2], raw: m[3] });
  }
  return views;
}

/** Extract polyline/circle/ellipse/line primitives from one view block. */
function parsePrimitives(raw) {
  const polylines = [];
  const circles = [];
  const ellipses = [];
  const lines = [];
  const re = /<(polyline|circle|ellipse|line)\s+([^>]*)\/>/g;
  let m;
  while ((m = re.exec(raw))) {
    const [, tag, attrStr] = m;
    const a = parseAttrs(attrStr);
    const cls = a.class ?? "";
    if (tag === "polyline") {
      const points = (a.points ?? "")
        .trim()
        .split(/\s+/)
        .filter(Boolean)
        .map((pair) => pair.split(",").map(Number));
      polylines.push({ cls, points });
    } else if (tag === "circle") {
      circles.push({ cls, cx: Number(a.cx), cy: Number(a.cy), r: Number(a.r) });
    } else if (tag === "ellipse") {
      ellipses.push({ cls, cx: Number(a.cx), cy: Number(a.cy), rx: Number(a.rx), ry: Number(a.ry) });
    } else if (tag === "line") {
      lines.push({ cls, x1: Number(a.x1), y1: Number(a.y1), x2: Number(a.x2), y2: Number(a.y2) });
    }
  }
  return { polylines, circles, ellipses, lines };
}

/** Every `<text ...>content</text>` in the WHOLE document (sheet-space). */
function parseTexts(svg) {
  const out = [];
  const re = /<text\s+([^>]*)>([^<]*)<\/text>/g;
  let m;
  while ((m = re.exec(svg))) {
    const a = parseAttrs(m[1]);
    out.push({
      cls: a.class ?? "",
      x: Number(a.x),
      y: Number(a.y),
      rotated: "transform" in a,
      content: unescapeXml(m[2]),
    });
  }
  return out;
}

// ── Property 1: EXACTNESS — the bore is a real conic, not a facet chain ───

function scoreExactness(t, svg, dxf) {
  const views = parseViews(svg);
  let matchedCircle = null;
  let matchedEllipse = null;
  let facetVertexCount = null;

  for (const v of views) {
    const p = parsePrimitives(v.raw);
    for (const c of p.circles) {
      if (Math.abs(c.r - BORE_R) <= RADIUS_TOL) matchedCircle = { view: v.projection, ...c };
    }
    for (const e of p.ellipses) {
      const rAvg = (e.rx + e.ry) / 2;
      if (Math.abs(rAvg - BORE_R) <= 0.5) matchedEllipse = { view: v.projection, ...e };
    }
    // Faceted-stand-in detector: a closed-ish polyline whose vertices sit at
    // a near-constant radius from their own centroid, close to BORE_R — the
    // exact shape a 96-gon approximation of the bore takes. Reported ONLY
    // for diagnostic legibility (the vertex count), never used to pass.
    for (const pl of p.polylines) {
      if (pl.cls.includes("hatch") || pl.points.length < 8) continue;
      const cx = pl.points.reduce((s, pt) => s + pt[0], 0) / pl.points.length;
      const cy = pl.points.reduce((s, pt) => s + pt[1], 0) / pl.points.length;
      const radii = pl.points.map((pt) => Math.hypot(pt[0] - cx, pt[1] - cy));
      const meanR = radii.reduce((s, r) => s + r, 0) / radii.length;
      const variance = radii.reduce((s, r) => s + (r - meanR) ** 2, 0) / radii.length;
      if (Math.abs(meanR - BORE_R) <= 1.0 && Math.sqrt(variance) < 0.1 * meanR) {
        facetVertexCount = pl.points.length;
      }
    }
  }

  const svgOk = matchedCircle !== null || matchedEllipse !== null;
  t.ok("the bore rim appears as a real SVG <circle>/<ellipse>, not a faceted polyline", svgOk, {
    dim: "correctness",
    detail: svgOk
      ? `matched ${JSON.stringify(matchedCircle ?? matchedEllipse)}`
      : `no conic entity found near r=${BORE_R}` +
        (facetVertexCount != null ? ` — found a ${facetVertexCount}-vertex polyline circumscribing that radius instead` : " (and no facet stand-in found either)"),
  });

  const circleLines = (dxf.match(/\r\nCIRCLE\r\n/g) ?? []).length + (dxf.match(/\nCIRCLE\n/g) ?? []).length;
  t.ok("the DXF export carries a native CIRCLE entity (the CAM-facing artifact)", circleLines >= 1, {
    dim: "correctness",
    detail: `CIRCLE entity count in DXF = ${circleLines}`,
  });
}

// ── Property 2: SILHOUETTE completeness — ink at BOTH extremes, not extent ─

/** Every FRONT-view segment that could plausibly be silhouette ink: plain
 *  and hidden polylines, and raw `<line>` primitives — EXCLUDING hatch
 *  (irrelevant here) and centerline (chain-line through the axis, drawn as
 *  a `<line class="centerline">` and NOT part of the solid's outline).
 *  Deliberately permissive about class names beyond that: the silhouette
 *  fix landing on `feat/silhouette-edges` may add a new polyline class or a
 *  bare `<line>` for a synthesized limb, and this must still see it. */
function frontInkSegments(front) {
  const p = parsePrimitives(front.raw);
  const segs = [];
  for (const pl of p.polylines) {
    if (pl.cls.includes("hatch") || pl.cls.includes("centerline")) continue;
    for (let i = 0; i + 1 < pl.points.length; i++) {
      const [x1, y1] = pl.points[i];
      const [x2, y2] = pl.points[i + 1];
      segs.push({ x1, y1, x2, y2 });
    }
  }
  for (const ln of p.lines) {
    if (ln.cls.includes("centerline")) continue;
    segs.push({ x1: ln.x1, y1: ln.y1, x2: ln.x2, y2: ln.y2 });
  }
  return segs;
}

function hasVerticalInkAt(segs, xTarget, tolX, minRun) {
  return segs.some((s) => Math.abs(s.x1 - xTarget) <= tolX && Math.abs(s.x2 - xTarget) <= tolX && Math.abs(s.y1 - s.y2) >= minRun);
}

function scoreSilhouette(t, svg) {
  const views = parseViews(svg);
  const front = views.find((v) => v.projection === "Front");
  const segs = front ? frontInkSegments(front) : [];

  // Assert on ink AT THE EXTREMES, never on view extent — the extent was
  // already correct while one side carried zero ink, which is the entire
  // reason this defect survived every existing eval. Vertices can reach an
  // x-extreme with no LINE drawn there; only a segment (two points sharing
  // that x, spanning real Y) counts as ink.
  const posInk = hasVerticalInkAt(segs, OD_R, SIL_TOL, SIL_MIN_RUN);
  const negInk = hasVerticalInkAt(segs, -OD_R, SIL_TOL, SIL_MIN_RUN);

  t.ok(`FRONT view carries silhouette ink at the +OD extreme (x=+${OD_R})`, posInk, {
    dim: "correctness",
    detail: front ? `${segs.length} candidate segments in FRONT` : "no FRONT view found in the export",
  });
  t.ok(`FRONT view carries silhouette ink at the -OD extreme (x=-${OD_R}) — the shipped silhouette defect`, negInk, {
    dim: "correctness",
    detail: front ? `${segs.length} candidate segments in FRONT` : "no FRONT view found in the export",
  });
}

// ── Property 3: SECTION substance — a connected outline spanning the cut ──

function bboxOf(points) {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const [x, y] of points) {
    if (x < minX) minX = x;
    if (x > maxX) maxX = x;
    if (y < minY) minY = y;
    if (y > maxY) maxY = y;
  }
  return { minX, minY, maxX, maxY };
}

function boundsBbox(outer, inner, margin) {
  return (
    inner.minX >= outer.minX - margin &&
    inner.maxX <= outer.maxX + margin &&
    inner.minY >= outer.minY - margin &&
    inner.maxY <= outer.maxY + margin
  );
}

/** mm — endpoint distance at which two outline edges count as joined.
 *
 *  MEASURED, not chosen: `svg.rs` emits the section outline as one 2-point
 *  `<polyline>` PER EDGE (flange SECTION A-A = 46 of them, ring plate = 35),
 *  so "connected" can only ever mean shared ENDPOINTS — there is no single
 *  polyline to test. Where an arc meets a line the two endpoints disagree by
 *  ~3e-3 mm. Swept against both rendered sheets: at 1e-6 the outline shatters
 *  into 14 (flange) and 2 (ring plate) components; at >= 5e-3 both collapse to
 *  exactly ONE. 0.01 sits above the measured gap and orders of magnitude below
 *  any gap that would matter on a sheet this size. */
const OUTLINE_JOIN_TOL = 0.01;

/** Group outline polylines into connected components by shared endpoints,
 *  returning each component's points. Pairwise rather than bucketed: a bucket
 *  boundary can fall between two points that are within tolerance and split a
 *  component that is physically joined, and a section view carries a few
 *  hundred points at most. */
function outlineComponents(polylines) {
  const parent = polylines.map((_, i) => i);
  const find = (a) => (parent[a] === a ? a : (parent[a] = find(parent[a])));
  const joined = (a, b) =>
    a.points.some(([ax, ay]) => b.points.some(([bx, by]) => Math.hypot(ax - bx, ay - by) <= OUTLINE_JOIN_TOL));

  for (let i = 0; i < polylines.length; i++) {
    for (let j = i + 1; j < polylines.length; j++) {
      if (find(i) === find(j)) continue;
      if (joined(polylines[i], polylines[j])) parent[find(i)] = find(j);
    }
  }

  const groups = new Map();
  polylines.forEach((pl, i) => {
    const r = find(i);
    if (!groups.has(r)) groups.set(r, []);
    groups.get(r).push(...pl.points);
  });
  return [...groups.values()];
}

function scoreSection(t, svg) {
  // The SECTION A-A view is identified by its HATCH content, not by
  // `data-projection` — the section is built with `ProjectionType::Custom`
  // (see dimensioning.rs::attach_section_view), so the projection label is
  // "Custom" like any other custom-angle view would be. `hatch_polylines`
  // is documented as populated ONLY for a SECTION view (types.rs), so "the
  // view group containing hatch ink" is the reliable signal.
  const views = parseViews(svg);
  const sectionViews = views
    .map((v) => ({ v, p: parsePrimitives(v.raw) }))
    .filter(({ p }) => p.polylines.some((pl) => pl.cls.includes("hatch")));

  t.ok("the export contains a SECTION view (hatch ink present somewhere)", sectionViews.length > 0, {
    dim: "correctness",
    detail: `${sectionViews.length} view group(s) with hatch ink out of ${views.length} total views`,
  });
  if (sectionViews.length === 0) return;

  const { p } = sectionViews[0];
  const hatchPolylines = p.polylines.filter((pl) => pl.cls.includes("hatch"));
  const nonHatchPolylines = p.polylines.filter((pl) => !pl.cls.includes("hatch") && !pl.cls.includes("centerline"));

  t.ok("SECTION view carries geometry beyond the hatch (non-hatch ink present, not confetti)", nonHatchPolylines.length > 0, {
    dim: "correctness",
    detail: `hatch polylines=${hatchPolylines.length}, non-hatch polylines=${nonHatchPolylines.length}`,
  });

  const hatchPts = hatchPolylines.flatMap((pl) => pl.points);
  const hatchBbox = bboxOf(hatchPts);
  const margin = 0.1;

  // Rank components by half-perimeter, NOT area. Measured: a section carries
  // flat polylines — circles seen edge-on collapse to a horizontal run at the
  // face they sit on (14 of them on the ring plate) — so bbox area is 0 for
  // real geometry and an area ranking picks an arbitrary winner.
  const components = outlineComponents(nonHatchPolylines)
    .map((pts) => ({ n: pts.length, b: bboxOf(pts) }))
    .sort((x, y) => y.b.maxX - y.b.minX + (y.b.maxY - y.b.minY) - (x.b.maxX - x.b.minX + (x.b.maxY - x.b.minY)));

  const largest = components[0] ?? null;
  const spansTheCut = largest !== null && hatchPts.length > 0 && boundsBbox(largest.b, hatchBbox, margin);
  t.ok("the SECTION's outline forms one connected run whose extent bounds the hatched cut faces", spansTheCut, {
    dim: "correctness",
    detail:
      largest === null
        ? "the SECTION view carries no non-hatch outline geometry at all"
        : `hatch bbox=${JSON.stringify(hatchBbox)}, ${components.length} outline component(s), ` +
          `largest=${largest.n} points bbox=${JSON.stringify(largest.b)}`,
  });
}

// ── Property 4: LEGIBILITY — no two annotation labels collide ─────────────

function textBbox(txt) {
  const style = ANNOTATION_FONT_MM[txt.cls.split(/\s+/).find((c) => c in ANNOTATION_FONT_MM)];
  if (!style) return null;
  const width = txt.content.length * style.font * GLYPH_WIDTH_RATIO;
  const xMin = style.anchor === "middle" ? txt.x - width / 2 : txt.x;
  const xMax = xMin + width;
  const yMin = txt.y - style.font * 0.8;
  const yMax = txt.y + style.font * 0.2;
  return { xMin, xMax, yMin, yMax, cls: txt.cls, content: txt.content };
}

function boxesOverlap(a, b) {
  return a.xMin < b.xMax && a.xMax > b.xMin && a.yMin < b.yMax && a.yMax > b.yMin;
}

function scoreLegibility(t, svg) {
  const texts = parseTexts(svg).filter((tx) => !tx.rotated);
  const boxes = texts.map(textBbox).filter(Boolean);

  let collision = null;
  for (let i = 0; i < boxes.length && !collision; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      if (boxesOverlap(boxes[i], boxes[j])) {
        collision = [boxes[i], boxes[j]];
        break;
      }
    }
  }

  t.ok("no two annotation text labels' approximate bounding boxes collide", collision === null, {
    dim: "correctness",
    detail: collision
      ? `"${collision[0].content}" (${collision[0].cls}) overlaps "${collision[1].content}" (${collision[1].cls})`
      : `${boxes.length} annotation label(s) checked, no overlap`,
  });
}

/**
 * The PURE scoring oracle. No I/O, no client — `svg`/`dxf` are the RAW
 * exported artifact bytes as text, exactly what `run` fetched from the live
 * server. All parsing happens here so `test/oracle-19.mjs` proves the
 * parse-and-judge pipeline together, not just the judge.
 *
 * @param t  the harness `Checks` collector
 * @param d  { svg: string, dxf: string }
 */
export function oracle(t, d) {
  const svg = d.svg ?? "";
  const dxf = d.dxf ?? "";
  scoreExactness(t, svg, dxf);
  scoreSilhouette(t, svg);
  scoreSection(t, svg);
  scoreLegibility(t, svg);
}

export default {
  id: "19-drawing-export-truth",
  title: "Drawing export truth — exactness, silhouette, section substance, legibility",
  dims: ["correctness", "soundness"],
  budgetMs: 120000,
  // T3 RE-STATED 2026-08-18, against measured sheets rather than reasoning.
  //
  // The old property — "non-hatch geometry extends BEYOND the hatched cut
  // faces' bounding box" — was unsatisfiable, not unsatisfied. Every sheet
  // measured puts the outline extent EQUAL to the hatch extent, because a cut
  // through the axis of a solid of revolution spans the whole silhouette:
  //
  //   drilled flange (live, `9976896a`)  hatch {-30,0}..{30,20}  outline same
  //   harness_flange.svg   (HEAD)        hatch {-60,0}..{60,14}  outline same
  //   harness_ring_plate.svg             hatch {-30,0}..{30,12}  outline same
  //
  // The replacement asserts the thing the CONFETTI defect actually violated:
  // the outline is ONE CONNECTED RUN whose extent BOUNDS the hatch. Strictly
  // more general than the old form — a sheet where "beyond" is genuinely true
  // (an offset section through a rib on a wider body) still satisfies it.
  //
  // Two premises were measured before being asserted, and BOTH would have
  // shipped a second unsatisfiable check if they had been taken on reasoning:
  //
  //   - "connected" cannot mean one polyline. svg.rs emits the outline one
  //     2-point polyline PER EDGE — 46 on the flange sheet, 35 on the ring
  //     plate — so no single-polyline form of this check can ever pass, and
  //     neither can "some single polyline bounds the hatch".
  //   - connectivity is tolerance-dependent: at 1e-6 the flange outline is 14
  //     components and NONE bounds the hatch; at >= 5e-3 it is exactly one.
  //     Adjacent edges disagree by ~3e-3 mm where an arc meets a line.
  //
  // Proven to discriminate: oracle-19 catches 9 of 9 lies, including a NEW
  // T4 — outline confetti parked on the four corners of the hatch extent —
  // which an aggregate bounding box passes and only the connected-run form
  // catches. The honest fixture was rebuilt to the markup shape svg.rs really
  // emits; the old one asserted closed rectangles nothing renders.
  //
  // STILL OWED, and stated rather than implied: no live backend was running
  // when this landed, so the re-stated check has NOT been run end-to-end
  // against the export routes. It is verified against the SAME renderer
  // (`render_drawing_svg`) via two rendered sheets, and the knownRed flag is
  // removed rather than left standing on an unmeasured reason — a flag whose
  // reason has gone stale is worse than no flag, which is the mistake this
  // scenario has now made twice.
  async run(ctx, t) {
    const { c } = ctx;

    const { id } = await ctx.time("build hub flange", () => buildHubFlange(c, { boltHoles: 0 }));

    const dr = await ctx.time("make drawing", () => c.raw("POST", `/api/parts/${id}/drawing?name=hub_flange_export_truth`, undefined, 90000));
    t.eq("drawing endpoint returns 200", dr.status, 200, { dim: "soundness" });
    const did = dr.data?.id;

    const svgResp = await c.raw("GET", `/api/drawings/${did}/svg`);
    t.eq("SVG export returns 200", svgResp.status, 200, { dim: "soundness" });
    const dxfResp = await c.raw("GET", `/api/drawings/${did}/dxf`);
    t.eq("DXF export returns 200", dxfResp.status, 200, { dim: "soundness" });

    const svg = typeof svgResp.data === "string" ? svgResp.data : "";
    const dxf = typeof dxfResp.data === "string" ? dxfResp.data : "";

    oracle(t, { svg, dxf });
  },
};
