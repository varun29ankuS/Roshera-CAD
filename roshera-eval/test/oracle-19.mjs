#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 19 (drawing export truth).
 *
 * Runs NO backend. Feeds the pure oracle a hand-built SVG/DXF fixture in the
 * EXACT markup shape `geometry-engine/src/drawing/svg.rs`'s `render_view` /
 * `render_view_labels` and `dxf.rs`'s DXF writer emit (verified against that
 * source directly — see scenario 19's header docblock), then a set of
 * single-mutation lies, at least one per scored check — the section's
 * connected-run check carries two, because T4 exists precisely to show that
 * the weaker aggregate-bbox form of that same check would pass a lie:
 *
 *   E1  the bore's SVG circle is replaced by a 96-vertex facet polyline
 *   E2  the DXF export's CIRCLE entity is renamed away (CAM can't see it)
 *   S1  the FRONT view's +OD silhouette vertex is dropped
 *   S2  the FRONT view's -OD silhouette vertex is dropped (the shipped defect)
 *   T1  no view group in the export carries hatch ink at all
 *   T2  the SECTION view's hatch survives but the outline is stripped
 *   T3  the outline survives but covers only the bore, not the cut extent
 *   T4  the outline is confetti on the corners of that extent (bulk bbox
 *       spans it perfectly; only the CONNECTED-run form catches this)
 *   L1  two annotation labels are moved to the same anchor
 *
 * Usage: node test/oracle-19.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/19-drawing-export-truth.mjs";

// ── Fixture geometry, matching buildHubFlange's revolve profile ───────────
const DEFAULT_FRONT_POINTS = [
  [-30, 0], [30, 0], [30, 6], [12, 6], [12, 20], [-12, 20], [-12, 6], [-30, 6], [-30, 0],
];
// The SECTION fixture is built to AGREE WITH A RENDERED SHEET, which the
// previous hand-built one never had to. Two structural facts, both measured
// off `target/drawing-visual-harness/harness_{flange,ring_plate}.svg`:
//
//   - hatch is 45-degree LINE segments, one 2-point polyline each (not the
//     closed rectangles this fixture used to assert)
//   - the outline is one 2-point polyline PER EDGE (46 on the flange sheet,
//     35 on the ring plate), never a single chained polyline
//
// The geometry is buildHubFlange's own revolve profile cut through its axis:
// bore r=6, plate r=30 x z 0..6, hub r=12 to z=20, mirrored about the axis.
// Hatch extent and outline extent therefore COINCIDE at {-30,0}..{30,20} —
// which is exactly what the live drilled flange measured at `9976896a`, and
// why the old "outline extends BEYOND the hatch" premise was unsatisfiable.
const DEFAULT_HATCH = [
  // right cut face: the plate's L-section, then the hub column
  [[24, 0], [30, 6]], [[18, 0], [24, 6]], [[12, 0], [18, 6]], [[6, 0], [12, 6]],
  [[6, 8], [12, 14]], [[6, 14], [12, 20]],
  // left cut face, mirrored
  [[-24, 0], [-30, 6]], [[-18, 0], [-24, 6]], [[-12, 0], [-18, 6]], [[-6, 0], [-12, 6]],
  [[-6, 8], [-12, 14]], [[-6, 14], [-12, 20]],
];
const DEFAULT_NONHATCH = [
  // right half of the cut profile, edge by edge
  [[6, 0], [30, 0]], [[30, 0], [30, 6]], [[30, 6], [12, 6]],
  [[12, 6], [12, 20]], [[12, 20], [6, 20]], [[6, 20], [6, 0]],
  // left half, mirrored
  [[-6, 0], [-30, 0]], [[-30, 0], [-30, 6]], [[-30, 6], [-12, 6]],
  [[-12, 6], [-12, 20]], [[-12, 20], [-6, 20]], [[-6, 20], [-6, 0]],
  // the bore's far wall, seen through the opening — back-of-plane geometry,
  // which lands INSIDE the hatch extent and is what joins the two halves into
  // a single connected run. The ring-plate sheet carries the same edge.
  [[-6, 0], [6, 0]], [[-6, 20], [6, 20]],
];
const DEFAULT_TEXTS = [
  { cls: "dim-text dim-text-c", x: 60.0, y: 40.0, content: "Ø12.00" },
  { cls: "label", x: 10.0, y: 15.0, content: "FRONT" },
  { cls: "label", x: 90.0, y: 15.0, content: "TOP" },
  { cls: "hole-tag", x: 140.0, y: 60.0, content: "A1" },
  { cls: "cutting-plane-label", x: 55.0, y: 95.0, content: "A" },
  { cls: "gdt-fcf-text", x: 130.0, y: 130.0, content: "FCF" },
  { cls: "datum-marker-label", x: 150.0, y: 160.0, content: "A" },
];

function removeVertex(points, [mx, my]) {
  return points.filter(([x, y]) => !(x === mx && y === my));
}

function circlePoints(n, r) {
  const pts = [];
  for (let i = 0; i < n; i++) {
    const th = (2 * Math.PI * i) / n;
    pts.push([Number((r * Math.cos(th)).toFixed(4)), Number((r * Math.sin(th)).toFixed(4))]);
  }
  return pts;
}

function ptsAttr(points) {
  return points.map((p) => p.join(",")).join(" ");
}

/** Build the SVG fixture. Every option defaults to the honest shape. */
function buildSvg(opts = {}) {
  const frontPoints = opts.frontPoints ?? DEFAULT_FRONT_POINTS;
  const boreCircle = opts.boreCircle !== false;
  const facetPolyline = opts.facetPolyline ?? null;
  const hatch = opts.hatchPolylines ?? DEFAULT_HATCH;
  const nonHatch = opts.nonHatchPolylines ?? DEFAULT_NONHATCH;
  const texts = opts.texts ?? DEFAULT_TEXTS;

  const frontInner = `    <polyline points="${ptsAttr(frontPoints)}" />\n`;

  let topInner = "";
  if (boreCircle) {
    topInner += `    <circle cx="0.0000" cy="0.0000" r="6.0000" />\n`;
    topInner += `    <circle class="hidden" cx="0.0000" cy="0.0000" r="30.0000" />\n`;
  }
  if (facetPolyline) {
    topInner += `    <polyline points="${ptsAttr(facetPolyline)}" />\n`;
  }

  const sectionInner =
    hatch.map((h) => `    <polyline class="hatch" points="${ptsAttr(h)}" />\n`).join("") +
    nonHatch.map((h) => `    <polyline points="${ptsAttr(h)}" />\n`).join("");

  const textsMarkup = texts.map((t) => `  <text class="${t.cls}" x="${t.x.toFixed(3)}" y="${t.y.toFixed(3)}">${t.content}</text>\n`).join("");

  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="210mm" height="297mm" viewBox="0 0 210 297">\n` +
    `  <g class="view" data-view-id="v-front" data-projection="Front" transform="translate(20.000 250.000) scale(1 -1) ">\n${frontInner}  </g>\n` +
    `  <g class="view" data-view-id="v-top" data-projection="Top" transform="translate(120.000 250.000) scale(1 -1) ">\n${topInner}  </g>\n` +
    `  <g class="view" data-view-id="v-sec" data-projection="Custom" transform="translate(20.000 150.000) scale(1 -1) ">\n${sectionInner}  </g>\n` +
    textsMarkup +
    `</svg>\n`
  );
}

/** Build the DXF fixture (minimal, group-code line format the real writer uses). */
function buildDxf(opts = {}) {
  const entity = opts.hasCircle === false ? "LWPOLYLINE" : "CIRCLE";
  return ["0", "SECTION", "2", "ENTITIES", "0", entity, "8", "0", "10", "0.0", "20", "0.0", "40", "6.0", "0", "ENDSEC", "0", "EOF"].join("\n") + "\n";
}

function honest() {
  return { svg: buildSvg(), dxf: buildDxf() };
}

const LIES = [
  {
    name: "E1 the bore's SVG circle is replaced by a 96-vertex facet polyline",
    build: () => ({ svg: buildSvg({ boreCircle: false, facetPolyline: circlePoints(96, 6) }), dxf: buildDxf() }),
  },
  {
    name: "E2 the DXF export's CIRCLE entity is renamed away (CAM can't see it)",
    build: () => ({ svg: buildSvg(), dxf: buildDxf({ hasCircle: false }) }),
  },
  {
    name: "S1 the FRONT view's +OD silhouette vertex is dropped",
    build: () => ({ svg: buildSvg({ frontPoints: removeVertex(DEFAULT_FRONT_POINTS, [30, 6]) }), dxf: buildDxf() }),
  },
  {
    name: "S2 the FRONT view's -OD silhouette vertex is dropped (the shipped defect, reproduced)",
    build: () => ({ svg: buildSvg({ frontPoints: removeVertex(DEFAULT_FRONT_POINTS, [-30, 6]) }), dxf: buildDxf() }),
  },
  {
    name: "T1 no view group in the export carries hatch ink at all",
    build: () => ({ svg: buildSvg({ hatchPolylines: [] }), dxf: buildDxf() }),
  },
  {
    name: "T2 the SECTION view's hatch survives but the outline is stripped (confetti regression)",
    build: () => ({ svg: buildSvg({ nonHatchPolylines: [] }), dxf: buildDxf() }),
  },
  {
    name: "T3 the SECTION view's outline survives but covers only the bore, not the cut faces' extent",
    build: () => ({
      svg: buildSvg({
        nonHatchPolylines: [[[-6, 0], [6, 0]], [[6, 0], [6, 20]], [[6, 20], [-6, 20]], [[-6, 20], [-6, 0]]],
      }),
      dxf: buildDxf(),
    }),
  },
  {
    // The lie an AGGREGATE bounding box cannot catch: four disconnected scraps
    // parked on the corners of the hatch extent span it perfectly in bulk.
    // Only the CONNECTED-run form of the check sees that the largest actual
    // run is a 2 mm square. This mutation is what earns the connectivity
    // clause its place in the property.
    name: "T4 the SECTION view's outline is confetti on the corners of the cut faces' extent",
    build: () => ({
      svg: buildSvg({
        nonHatchPolylines: [
          [[-30, 0], [-28, 0]], [[-28, 0], [-28, 2]], [[-28, 2], [-30, 2]], [[-30, 2], [-30, 0]],
          [[30, 0], [28, 0]], [[28, 0], [28, 2]], [[28, 2], [30, 2]], [[30, 2], [30, 0]],
          [[-30, 20], [-28, 20]], [[-28, 20], [-28, 18]], [[-28, 18], [-30, 18]], [[-30, 18], [-30, 20]],
          [[30, 20], [28, 20]], [[28, 20], [28, 18]], [[28, 18], [30, 18]], [[30, 18], [30, 20]],
        ],
      }),
      dxf: buildDxf(),
    }),
  },
  {
    name: "L1 two annotation labels are moved to the same anchor",
    build: () => ({
      svg: buildSvg({ texts: DEFAULT_TEXTS.map((t) => (t.cls === "hole-tag" ? { ...t, x: 60.0, y: 40.0 } : t)) }),
      dxf: buildDxf(),
    }),
  },
];

function main() {
  let failures = 0;

  const t = new Checks(scenario.id);
  oracle(t, honest());
  const failed = t.items.filter((i) => !i.passed);
  if (failed.length > 0) {
    failures += 1;
    console.log("FAIL  honest fixture did not pass cleanly:");
    for (const f of failed) console.log(`        [${f.dim}] ${f.name} — ${f.detail}`);
  } else {
    console.log(`ok    honest fixture passes all ${t.items.length} checks`);
  }

  for (const lie of LIES) {
    const tc = new Checks(scenario.id);
    oracle(tc, lie.build());
    const caught = tc.items.filter((i) => !i.passed);
    if (caught.length === 0) {
      failures += 1;
      console.log(`FAIL  lie SURVIVED the oracle: ${lie.name}`);
    } else {
      console.log(`ok    caught: ${lie.name}  (${caught.map((c) => `[${c.dim}] ${c.name}`).join("; ")})`);
    }
  }

  if (!scenario.dims.includes("correctness")) {
    failures += 1;
    console.log("FAIL  scenario 19 must declare the correctness dimension");
  } else {
    console.log("ok    scenario declares the correctness dimension");
  }

  console.log(
    failures === 0
      ? `\nORACLE VALIDATED — honest fixture passes, all ${LIES.length} lies caught.`
      : `\n${failures} ORACLE DEFECT(S).`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main();
