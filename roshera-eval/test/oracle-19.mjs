#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 19 (drawing export truth).
 *
 * Runs NO backend. Feeds the pure oracle a hand-built SVG/DXF fixture in the
 * EXACT markup shape `geometry-engine/src/drawing/svg.rs`'s `render_view` /
 * `render_view_labels` and `dxf.rs`'s DXF writer emit (verified against that
 * source directly — see scenario 19's header docblock), then a set of
 * single-mutation lies, one per scored check:
 *
 *   E1  the bore's SVG circle is replaced by a 96-vertex facet polyline
 *   E2  the DXF export's CIRCLE entity is renamed away (CAM can't see it)
 *   S1  the FRONT view's +OD silhouette vertex is dropped
 *   S2  the FRONT view's -OD silhouette vertex is dropped (the shipped defect)
 *   T1  no view group in the export carries hatch ink at all
 *   T2  the SECTION view's hatch survives but the outline is stripped
 *   T3  the SECTION view's outline survives but never leaves the hatch bbox
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
const DEFAULT_HATCH = [
  [[-30, 0], [-20, 0], [-20, 6], [-30, 6]],
  [[-8, 0], [8, 0], [8, 6], [-8, 6]],
  [[20, 0], [30, 0], [30, 6], [20, 6]],
];
const DEFAULT_NONHATCH = [[[-12, 6], [-12, 20], [12, 20], [12, 6]]];
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
    name: "T3 the SECTION view's outline survives but never leaves the hatch bbox",
    build: () => ({ svg: buildSvg({ nonHatchPolylines: [[[-8, 0], [8, 0], [8, 6], [-8, 6]]] }), dxf: buildDxf() }),
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
