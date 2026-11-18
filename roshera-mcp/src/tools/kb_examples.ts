/**
 * Worked examples for the part-family playbooks (2026-11-18) — ONE complete,
 * real-numbered build per family that an agent opens before it starts. Served
 * as `worked_example` on kb_lookup(kind:"playbook") for the families that have
 * one; the playbook `text` stays the token-pinned prose, the example rides
 * beside it.
 *
 * Shape: `steps` are the calls in order, exactly as the agent sends them.
 * Inside a cad_program, '$N.key' is the program's own chaining placeholder
 * (cad_program.ts). Across separate calls a value cannot be chained, so a step
 * names it in `bind` ({arg path: where the value comes from}) and leaves the
 * arg out — the agent fills it from the earlier result. test/kb_playbooks
 * validates every call against its tool's own zod schema with those fields
 * given typed dummies.
 *
 * Every number below is either the engineer's (stated in `spec`) or computed
 * here from a cited reference (the NACA generator, ISO 273, EN 1993-1-8,
 * the bend-allowance relation) — never typed in from memory. The aerofoil's
 * polyline is the generator's own output, so the example cannot drift from it.
 *
 * Honesty of the examples themselves: they are validated STATICALLY (tool
 * names, argument names and types). They have not been executed against a live
 * kernel by this module; `static_check` says so in every payload.
 */

import { referenceLookup } from "./kb_reference.js";

export interface ExampleStep {
  step: number;
  tool: string;
  args: Record<string, unknown>;
  /** arg path → the earlier result this value is read from (cross-call). */
  bind?: Record<string, string>;
  why: string;
}

export interface WorkedExample {
  spec: string;
  how_to_read: string;
  steps: ExampleStep[];
  expect: string[];
  state_as_uncertified: string[];
  static_check: string;
}

const HOW_TO_READ =
  "Send each step as written. `bind` is NOT an argument: fill each named arg (a dotted path into args) from the earlier result it names, then drop the bind key — a cad_program op carrying `bind` is refused. '$N.key' inside a cad_program is that program's own chaining and is sent as is.";

const STATIC_CHECK =
  "validated statically against the MCP tool schemas (names, argument names and types); not executed against a live kernel by the knowledge base — read every ledger and verdict as it comes back";

/** Throws at module load if a cited reference stops answering: an example
 *  built on a refused lookup must not ship (the test suite loads this). */
function ref(key: string, args: Record<string, unknown>): Record<string, any> {
  const r = referenceLookup(key, args);
  if ("refused" in r) throw new Error(`worked example reference ${key} refused: ${r.reason}`);
  return r.value as Record<string, any>;
}

const r3 = (v: number) => Math.round(v * 1000) / 1000;

// ─── aerofoil_section ───────────────────────────────────────────────────────

function aerofoilExample(): WorkedExample {
  const SPAN = 300;
  const MID = SPAN / 2;
  const MAIN = { x: 50, y: 3.5, r: 6 };
  const REAR = { x: 120, y: 3.5, r: 4 };
  /** 1 mm inside the trailing edge: where a ray across the section measures
   *  the as-built TE region (the wall rule does not reach a tapering wedge). */
  const TE_X = 199;
  const lookupArgs = {
    designation: "2412",
    chord_mm: 200,
    trailing_edge: "blunt",
    te_thickness_mm: 1.0,
    n_per_surface: 80,
    stations_x_mm: [MAIN.x, REAR.x, TE_X],
  };
  const naca = ref("naca_profile", lookupArgs);
  const holeArea = Math.PI * (MAIN.r ** 2 + REAR.r ** 2);
  const volume = r3((naca.polygon_area_mm2 - holeArea) * SPAN);
  const probes: Array<{ point: [number, number]; polygon_distance_mm: number }> = naca.probes;
  const maxProbe = Math.max(...probes.map((p) => p.polygon_distance_mm));
  const st = (x: number) => (naca.stations as Array<Record<string, number>>).find((s) => s.x_mm === x)!;
  const wall = (h: { x: number; y: number; r: number }) => ({
    above: r3(st(h.x).upper_y_mm - (h.y + h.r)),
    below: r3(h.y - h.r - st(h.x).lower_y_mm),
  });
  const wMain = wall(MAIN);
  const wRear = wall(REAR);
  const PART = { part_id: "the part_id of step 3's psketch_extrude (ledger op 4)" };

  return {
    spec:
      "Engineer's spec, every SPEC FIRST answer given: NACA 2412 strut section, chord 200 mm, span 300 mm, " +
      "trailing edge blunt 1.0 mm (their choice: 2.5x a 0.4 mm nozzle), main spar bore D12 at 25% chord, " +
      "rear spar bore D8 at 60% chord, FDM PETG printed standing on the root, root face bolts to a fuselage plate. " +
      `The lookup's stations put the mean line at y = ${r3(st(MAIN.x).camber_y_mm)} mm (x = 50) and ` +
      `${r3(st(REAR.x).camber_y_mm)} mm (x = 120); both bores sit at y = 3.5 (within 0.06 mm of the mean line, ` +
      "one chordwise line so the position basic has no y component).",
    steps: [
      {
        step: 1,
        tool: "kb_lookup",
        args: { kind: "reference", key: "naca_profile", args: lookupArgs },
        why: "the profile, its faceting bound, probe points and the spar stations — every profile number comes from here",
      },
      {
        step: 2,
        tool: "timeline_checkpoint",
        args: {
          name: "NACA 2412 strut c200 x span 300, blunt TE 1.0, spar bores D12 at x50 and D8 at x120",
          description:
            "Profile generated from the designation (closed-TE yt plus a linear ramp to a 1.0 mm TE), 161-vertex cosine-spaced polygon; spars on one chordwise line y = 3.5.",
        },
        why: "declare the intent; mutating calls are refused without it",
      },
      {
        step: 3,
        tool: "cad_program",
        args: {
          name: "naca 2412 strut section",
          ops: [
            { tool: "psketch_begin", args: {} },
            {
              tool: "psketch_add_entity",
              args: { csketch_id: "$0.csketch_id", kind: "polyline", params: { points: naca.points, closed: true } },
            },
            { tool: "psketch_add_entity", args: { csketch_id: "$0.csketch_id", kind: "circle", params: { cx: MAIN.x, cy: MAIN.y, radius: MAIN.r } } },
            { tool: "psketch_add_entity", args: { csketch_id: "$0.csketch_id", kind: "circle", params: { cx: REAR.x, cy: REAR.y, radius: REAR.r } } },
            { tool: "psketch_extrude", args: { csketch_id: "$0.csketch_id", distance: SPAN, plane: "xy", name: "naca2412_strut" } },
          ],
        },
        why: "one program: outline + two bore circles + extrude; the circles become true cylindrical bores, the root face lands at z = 0",
      },
      {
        step: 4,
        tool: "verify_part",
        args: {},
        bind: PART,
        why: "the full certificate before any number is trusted",
      },
      {
        step: 5,
        tool: "verify_claim",
        args: {
          expr: "v",
          bindings: [{ var: "v", measure: { kind: "volume" } }],
          expected: volume,
          tolerance: 1.0,
        },
        bind: { "bindings.0.measure.part": "the object_uuid of step 3's psketch_extrude (ledger op 4)" },
        why: `kernel volume must equal (polygon area ${r3(naca.polygon_area_mm2)} mm² − bores ${r3(holeArea)} mm²) x 300 = ${volume} mm³: proves the kernel built the polygon that was sent`,
      },
      {
        step: 6,
        tool: "cad_program",
        args: {
          name: "profile probes + spar walls (read-only)",
          ops: [
            ...probes.map((p) => ({
              tool: "point_query",
              args: { point: [p.point[0], p.point[1], MID] },
              bind: PART,
            })),
            ...[MAIN, REAR].flatMap((h) => [
              { tool: "ray_query", args: { origin: [h.x, h.y, MID], direction: [0, 1, 0] }, bind: PART },
              { tool: "ray_query", args: { origin: [h.x, h.y, MID], direction: [0, -1, 0] }, bind: PART },
            ]),
            { tool: "ray_query", args: { origin: [TE_X, 20, MID], direction: [0, -1, 0] }, bind: PART },
          ],
        },
        why:
          `point_query at the ${probes.length} analytic probe points: each |signed distance| is the kernel's measurement of the as-built section's distance from NACA 2412 there (expected up to ${maxProbe.toExponential(2)} mm). ` +
          "ray_query from each bore centre along +Y and −Y: the second crossing minus the first is the wall. " +
          `The last ray crosses the section 1 mm inside the trailing edge: its two crossings ${r3(st(TE_X).thickness_mm)} mm apart (analytic) are the kernel's read of the TE region`,
      },
      {
        step: 7,
        tool: "cad_program",
        args: {
          name: "datums A root face, B main spar bore; flatness, perpendicularity, position",
          ops: [
            { tool: "gdt_datum", args: { label: "A", selector: '{"kind":"planar","normal_dir":[0,0,-1]}' }, bind: PART },
            { tool: "gdt_datum", args: { label: "B", selector: '{"kind":"cylindrical","extremal":"largest_area"}' }, bind: PART },
            {
              tool: "gdt_fcf",
              args: { characteristic: "flatness", tolerance_mm: 0.05, datum_refs: [], target_selector: '{"kind":"planar","normal_dir":[0,0,-1]}' },
              bind: PART,
            },
            {
              tool: "gdt_fcf",
              args: { characteristic: "perpendicularity", tolerance_mm: 0.05, datum_refs: ["A"], target_selector: '{"kind":"cylindrical","extremal":"largest_area"}' },
              bind: PART,
            },
            {
              tool: "gdt_fcf",
              args: {
                characteristic: "position",
                tolerance_mm: 0.1,
                datum_refs: ["A", "B"],
                target_selector: '{"kind":"cylindrical","extremal":"smallest_area"}',
                basic: [REAR.x - MAIN.x, 0],
              },
              bind: PART,
            },
          ],
        },
        why:
          "datum A = root face (seats on the fuselage plate, 3 DOF), datum B = main spar bore axis (locates the section in plane); the rear bore is positioned |A|B| at basic [70, 0] — DRF origin where B pierces A, X′ = world X. " +
          "If select_face refuses a selector as ambiguous, read part_features and pass target_face / face_id instead",
      },
      {
        step: 8,
        tool: "dfm_check",
        args: { pack: "fdm", nozzle_diameter: 0.4, build_direction: [0, 0, 1] },
        bind: PART,
        why:
          "the stated process, summary reported verbatim. Its wall rule pairs only parallel planes and coaxial cylinders, so it does NOT measure the tapering trailing edge — expect that rule to say so (unverifiable or no pair), and carry the TE thickness from the lookup and the step-6 ray instead",
      },
    ],
    expect: [
      "step 3 ledger: 5/5 ops ok, the extrude reports one region (the outline with its two bores)",
      "step 4: sound, watertight, manifold; Euler characteristic −2 (genus 2: two through-bores)",
      `step 5: verified — volume ${volume} mm³ within 1.0 mm³`,
      `step 6: every |point_query distance| within 1e-4 mm of that probe's polygon_distance_mm; max ≈ ${maxProbe.toExponential(2)} mm`,
      `step 6: walls ≈ ${wMain.above} / ${wMain.below} mm above/below the D12 bore and ${wRear.above} / ${wRear.below} mm around the D8 bore (faceted surface, so within the faceting bound of these analytic figures)`,
      "step 7: flatness, perpendicularity and position IN SPEC at ~0.00 on a perfect build; each verdict discloses its DRF",
      `step 6: the trailing-edge ray's crossings ≈ ${r3(st(TE_X).thickness_mm)} mm apart`,
      "step 8: the DfmSummary verbatim, every unverifiable count included — never reported as a pass for the trailing edge",
    ],
    state_as_uncertified: [
      `"The section is a 161-vertex polygon: measured by the kernel at ${probes.length} probe points it lies at most <max |d|> mm from NACA 2412; between the probes the generator's dense-sample bound is ${Number(naca.faceting.max_deviation_mm).toExponential(2)} mm — a computed figure, not a kernel measurement."`,
      "The trailing-edge thickness is the stated 1.0 mm and a measured cross-section 1 mm inside it; the printability wall check does not reach a tapering edge.",
      "The 1.0 mm trailing edge is a stated departure from NACA 2412: closed-TE thickness plus a linear ramp, not part of the designation.",
      "Aerodynamic performance (lift, drag, pitching moment, stall) is not computed and not claimed.",
      "No structural check of the spars or the root joint was run.",
    ],
    how_to_read: HOW_TO_READ,
    static_check: STATIC_CHECK,
  };
}

// ─── sheet_metal_bracket ────────────────────────────────────────────────────

function bracketExample(): WorkedExample {
  const T = 2;
  const R = 2;
  const K = 0.4;
  const BASE = 50;
  const UP = 40;
  const WIDTH = 30;
  const m5 = ref("clearance_hole", { fastener: "M5", class: "medium" });
  const m6 = ref("clearance_hole", { fastener: "M6", class: "medium" });
  const ba = ref("bend_allowance", { thickness_mm: T, angle_deg: 90, radius_mm: R, k_factor: K, material: "5052-H32" });
  const r5 = m5.diameter_mm / 2;
  const r6 = m6.diameter_mm / 2;
  const flat = r3(BASE - (R + T) + (UP - (R + T)) + ba.bend_allowance_mm);
  const sectionArea = (BASE - (R + T)) * T + (UP - (R + T)) * T + (Math.PI / 4) * ((R + T) ** 2 - R ** 2);
  const volume = r3(sectionArea * WIDTH - 2 * Math.PI * r5 * r5 * T - Math.PI * r6 * r6 * T);
  const BUILT = "the part_id of step 5's boolean_many (ledger op 14)";

  return {
    spec:
      `Engineer's spec: L bracket, 5052-H32 aluminium, t = ${T} mm (their stock), inside bend radius R = ${R} mm (1t, their press brake), ` +
      `K = ${K} (their shop's figure for this tooling), base leg ${BASE} mm and upright ${UP} mm OUTSIDE dimensions, width ${WIDTH} mm. ` +
      `Base: 2x M5 clearance (ISO 273 medium, D${m5.diameter_mm}) at 30 mm from the upright's outside face, 7.5 mm in from each side. ` +
      `Upright: 1x M6 clearance (ISO 273 medium, D${m6.diameter_mm}) 25 mm above the base underside, centred. ` +
      "Modelled with the section in XY (base along +X, upright along +Y) and the width along +Z.",
    steps: [
      { step: 1, tool: "kb_lookup", args: { kind: "reference", key: "clearance_hole", args: { fastener: "M5", class: "medium" } }, why: `D${m5.diameter_mm}, ISO 273 medium` },
      { step: 2, tool: "kb_lookup", args: { kind: "reference", key: "clearance_hole", args: { fastener: "M6", class: "medium" } }, why: `D${m6.diameter_mm}, ISO 273 medium` },
      {
        step: 3,
        tool: "kb_lookup",
        args: { kind: "reference", key: "bend_allowance", args: { thickness_mm: T, angle_deg: 90, radius_mm: R, k_factor: K, material: "5052-H32" } },
        why: `BA = ${r3(ba.bend_allowance_mm)} mm with the engineer's K; flat length = (${BASE} − ${R + T}) + (${UP} − ${R + T}) + BA = ${flat} mm`,
      },
      {
        step: 4,
        tool: "timeline_checkpoint",
        args: {
          name: `L bracket 5052-H32 t${T} R${R}, legs ${BASE} x ${UP} outside x ${WIDTH} wide, 2x D${m5.diameter_mm} base + D${m6.diameter_mm} upright`,
          description: "Bent sheet modelled as a constant-thickness solid: inside fillet R, concentric outside fillet R + t.",
        },
        why: "declare the intent",
      },
      {
        step: 5,
        tool: "cad_program",
        args: {
          name: "L bracket body, bend, holes",
          ops: [
            { tool: "psketch_begin", args: {} },
            {
              tool: "psketch_add_entity",
              args: { csketch_id: "$0.csketch_id", kind: "polyline", params: { points: [[0, 0], [BASE, 0], [BASE, T], [T, T], [T, UP], [0, UP]], closed: true } },
            },
            { tool: "psketch_extrude", args: { csketch_id: "$0.csketch_id", distance: WIDTH, plane: "xy", name: "l_bracket" } },
            { tool: "select_edge", args: { part_id: "$2.part_id", curve_kind: "line", convexity: "concave", direction: [0, 0, 1] } },
            { tool: "fillet_edges", args: { part_id: "$2.part_id", radius: R, edge_ids: ["$3.edge_id"] } },
            {
              tool: "select_edge",
              args: { part_id: "$4.part_id", curve_kind: "line", blend: "unblended", convexity: "convex", direction: [0, 0, 1], extremal: "most_along", along: [-1, -1, 0] },
            },
            { tool: "fillet_edges", args: { part_id: "$4.part_id", radius: R + T, edge_ids: ["$5.edge_id"] } },
            { tool: "ray_query", args: { part_id: "$6.part_id", origin: [R + T, R + T, WIDTH / 2], direction: [-1, -1, 0] } },
            { tool: "select_face", args: { part_id: "$6.part_id", kind: "planar", normal_dir: [0, 1, 0], extremal: "largest_area" } },
            { tool: "select_face", args: { part_id: "$6.part_id", kind: "planar", normal_dir: [1, 0, 0], extremal: "largest_area" } },
            { tool: "measure_faces", args: { part_a: "$6.part_id", face_a: "$8.face_id", part_b: "$6.part_id", face_b: "$9.face_id" } },
            { tool: "create_cylinder", args: { center: [30, -1, 7.5], axis: [0, 1, 0], radius: r5, height: T + 2, name: "m5_clear_a" } },
            { tool: "create_cylinder", args: { center: [30, -1, 22.5], axis: [0, 1, 0], radius: r5, height: T + 2, name: "m5_clear_b" } },
            { tool: "create_cylinder", args: { center: [-1, 25, WIDTH / 2], axis: [1, 0, 0], radius: r6, height: T + 2, name: "m6_clear" } },
            { tool: "boolean_many", args: { op: "difference", base: "$6.object_uuid", tools: ["$11.object_uuid", "$12.object_uuid", "$13.object_uuid"] } },
          ],
        },
        why:
          "sharp-cornered L extruded by the width; the concave inside corner filleted R, then the convex outside corner (the edge furthest toward −X−Y) filleted R + t — concentric, so the bend keeps thickness t. " +
          `The bend is measured before the holes go in: a ray from the bend axis (${R + T}, ${R + T}) along −X−Y crosses the inside bend at ${R} mm and the outside at ${R + T} mm (concentric radii, thickness ${T}); ` +
          "measure_faces between the two inner flange faces reads the bend angle (blend faces are not plain cylinders to measure_faces, so the radii are read by the ray). " +
          "Three clearance cylinders overshoot each flange and are subtracted in one batch",
      },
      { step: 6, tool: "verify_part", args: {}, bind: { part_id: BUILT }, why: "full certificate" },
      {
        step: 7,
        tool: "verify_claim",
        args: { expr: "v", bindings: [{ var: "v", measure: { kind: "volume" } }], expected: volume, tolerance: r3(volume * 1e-4) },
        bind: { "bindings.0.measure.part": "the object_uuid returned by step 5's boolean_many (ledger op 14)" },
        why:
          `section area ${r3(sectionArea)} mm² (two straight legs + a quarter annulus) x ${WIDTH} − three bores = ${volume} mm³, tolerance 1e-4 relative: ` +
          "the volume over blend faces is integrated by quadrature and may carry its own error bound — if the claim fails inside that bound, report both numbers; never widen the tolerance silently",
      },
      { step: 8, tool: "part_features", args: {}, bind: { part_id: BUILT }, why: "hole diameters and the face ids of the three bores (select_face cannot tell the two equal M5 holes apart)" },
      {
        step: 9,
        tool: "cad_program",
        args: {
          name: "datums A base underside, B upright back; hole position and perpendicularity",
          ops: [
            { tool: "gdt_datum", args: { label: "A", selector: '{"kind":"planar","normal_dir":[0,-1,0]}' }, bind: { part_id: BUILT } },
            { tool: "gdt_datum", args: { label: "B", selector: '{"kind":"planar","normal_dir":[-1,0,0]}' }, bind: { part_id: BUILT } },
            {
              tool: "gdt_fcf",
              args: { characteristic: "position", tolerance_mm: 0.2, datum_refs: ["A", "B"], basic: [30, 7.5] },
              bind: { part_id: BUILT, target_face: "step 8: the M5 bore face whose axis passes (30, ·, 7.5)" },
            },
            {
              tool: "gdt_fcf",
              args: { characteristic: "position", tolerance_mm: 0.2, datum_refs: ["A", "B"], basic: [30, 22.5] },
              bind: { part_id: BUILT, target_face: "step 8: the M5 bore face whose axis passes (30, ·, 22.5)" },
            },
            {
              tool: "gdt_fcf",
              args: { characteristic: "perpendicularity", tolerance_mm: 0.1, datum_refs: ["A"] },
              bind: { part_id: BUILT, target_face: "step 8: either M5 bore face" },
            },
            {
              tool: "gdt_fcf",
              args: { characteristic: "position", tolerance_mm: 0.2, datum_refs: ["B", "A"], basic: [25, WIDTH / 2] },
              bind: { part_id: BUILT, target_face: `step 8: the D${m6.diameter_mm} bore face (axis along X)` },
            },
          ],
        },
        why:
          "datum A = the base underside (bolts down, 3 DOF), datum B = the upright's back face (orients the bracket about A's normal). " +
          "|A|B|: Z′ = −Y (A outward), X′ = +X (B inward), Y′ = Z′ × X′ = +Z, y₀ = part corner z = 0 — basics read [30, 7.5] like the drawing. " +
          "The upright hole is located |B|A| (B primary, its own seating face): X′ = +Y, Y′ = −Z from the far side z = 30, so the centred hole reads [25, 15] either way. " +
          "Every hole target is a face id from step 8: the two M5 bores are equal, and no size ordering singles out the M6 among the bend cylinders",
      },
    ],
    expect: [
      `step 5: the bend ray's crossings at ${R} mm and ${R + T} mm from the bend axis (inside and outside radius, concentric), dihedral between the inner flange faces 90°`,
      "step 5: boolean_many completed 3 of 3, every step sound",
      "step 6: sound, watertight, manifold",
      `step 7: verified — ${volume} mm³ within ${r3(volume * 1e-4)} mm³`,
      `step 8: bore diameters ${m5.diameter_mm} (x2) and ${m6.diameter_mm} mm`,
      "step 9: every FCF IN SPEC at ~0.00 on a perfect build, each disclosing its DRF",
    ],
    state_as_uncertified: [
      `Flat length ${flat} mm comes from the bend-allowance relation with the engineer's K = ${K}; the kernel has no unfold, so it is a computation, not a measurement.`,
      "Springback, the achievable bend-angle tolerance and hole distortion near a bend are not modelled.",
      "The sheet-metal guidance (inside radius ≥ 1t, hole-to-bend distance, bend relief) is vendor practice; the kernel cannot check sheet-metal rules, so none of it is a kernel verdict.",
    ],
    how_to_read: HOW_TO_READ,
    static_check: STATIC_CHECK,
  };
}

// ─── mounting_plate ─────────────────────────────────────────────────────────

function plateExample(): WorkedExample {
  const L = 120;
  const W = 80;
  const T = 10;
  const RC = 5;
  const E = 12;
  const m8 = ref("clearance_hole", { fastener: "M8" });
  const edge = ref("bolt_edge_distance", { hole_diameter_mm: m8.diameter_mm });
  const tol = ref("general_tolerance", { nominal_mm: L, class: "m" });
  const r = m8.diameter_mm / 2;
  const holes: Array<[number, number]> = [[E, E], [L - E, E], [E, W - E], [L - E, W - E]];
  const volume = r3(L * W * T - 4 * (RC * RC - (Math.PI * RC * RC) / 4) * T - 4 * Math.PI * r * r * T);
  const BUILT = "the part_id of step 5's boolean_many (ledger op 10)";

  return {
    spec:
      `Engineer's spec: mounting plate ${L} x ${W} x ${T} mm, 6082-T6, CNC milled, corner radii R${RC}, ` +
      `4x M8 clearance holes ${E} mm in from both adjacent edges (no class stated, so the house default ISO 273 medium applies: D${m8.diameter_mm} — say so), ` +
      `general tolerance ISO 2768-m (their choice), hole position ${"0.2"} mm to datums A|B. ` +
      "The seating face is modelled toward +Z (z = 10) so the position basics read from the left and front edges like the drawing.",
    steps: [
      { step: 1, tool: "kb_lookup", args: { kind: "reference", key: "clearance_hole", args: { fastener: "M8" } }, why: `D${m8.diameter_mm} — house default class, attributed as such` },
      {
        step: 2,
        tool: "kb_lookup",
        args: { kind: "reference", key: "bolt_edge_distance", args: { hole_diameter_mm: m8.diameter_mm } },
        why: `EN 1993-1-8 minimums e1 = e2 = ${edge.e1_min_mm} mm, p2 = ${edge.p2_min_mm} mm: ${E} mm edge distance clears ${edge.e1_min_mm}, the ${L - 2 * E} / ${W - 2 * E} mm spacings clear ${edge.p2_min_mm} — a structural-steel rule applied as a floor, stated as such`,
      },
      {
        step: 3,
        tool: "kb_lookup",
        args: { kind: "reference", key: "general_tolerance", args: { nominal_mm: L, class: "m" } },
        why: `ISO 2768-m: ±${tol.plus_minus_mm} mm on the ${L} mm length (repeat per nominal for the drawing)`,
      },
      {
        step: 4,
        tool: "timeline_checkpoint",
        args: {
          name: `Mounting plate ${L} x ${W} x ${T}, R${RC} corners, 4x D${m8.diameter_mm} M8 clearance at ${E} from edges`,
          description: "Box at the origin corner, corner fillets, four overshooting clearance cylinders subtracted in one batch.",
        },
        why: "declare the intent",
      },
      {
        step: 5,
        tool: "cad_program",
        args: {
          name: "mounting plate body, corners, holes",
          ops: [
            { tool: "create_box", args: { center: [L / 2, W / 2, 0], width: L, depth: W, height: T, name: "mounting_plate" } },
            ...([[1, 1], [-1, 1], [1, -1], [-1, -1]] as Array<[number, number]>).map(([sx, sy]) => ({
              tool: "select_edge",
              args: { part_id: "$0.part_id", curve_kind: "line", direction: [0, 0, 1], extremal: "most_along", along: [sx, sy, 0] },
            })),
            { tool: "fillet_edges", args: { part_id: "$0.part_id", radius: RC, edge_ids: ["$1.edge_id", "$2.edge_id", "$3.edge_id", "$4.edge_id"] } },
            ...holes.map(([x, y], i) => ({
              tool: "create_cylinder",
              args: { center: [x, y, -1], axis: [0, 0, 1], radius: r, height: T + 2, name: `m8_clear_${i + 1}` },
            })),
            { tool: "boolean_many", args: { op: "difference", base: "$5.object_uuid", tools: ["$6.object_uuid", "$7.object_uuid", "$8.object_uuid", "$9.object_uuid"] } },
          ],
        },
        why: "box base at the corner (x 0..120, y 0..80, z 0..10); each vertical corner edge is the one furthest toward its corner; four cylinders overshoot both faces by 1 mm",
      },
      { step: 6, tool: "verify_part", args: {}, bind: { part_id: BUILT }, why: "full certificate" },
      { step: 7, tool: "part_features", args: {}, bind: { part_id: BUILT }, why: "four D9.0 bores, their axes, and their face ids for the FCFs" },
      {
        step: 8,
        tool: "verify_claim",
        args: { expr: "v", bindings: [{ var: "v", measure: { kind: "volume" } }], expected: volume, tolerance: r3(volume * 1e-4) },
        bind: { "bindings.0.measure.part": "the object_uuid returned by step 5's boolean_many (ledger op 10)" },
        why:
          `${L}·${W}·${T} − 4 corner losses (R² − πR²/4)·t − 4 bores = ${volume} mm³, tolerance 1e-4 relative: ` +
          "the corner blends are integrated by quadrature and may carry their own error bound — if the claim fails inside it, report both numbers; never widen the tolerance silently",
      },
      {
        step: 9,
        tool: "cad_program",
        args: {
          name: "datums A seating face, B left edge; flatness + position of the 4 holes",
          ops: [
            { tool: "gdt_datum", args: { label: "A", selector: '{"kind":"planar","normal_dir":[0,0,1]}' }, bind: { part_id: BUILT } },
            { tool: "gdt_datum", args: { label: "B", selector: '{"kind":"planar","normal_dir":[-1,0,0]}' }, bind: { part_id: BUILT } },
            {
              tool: "gdt_fcf",
              args: { characteristic: "flatness", tolerance_mm: 0.05, datum_refs: [], target_selector: '{"kind":"planar","normal_dir":[0,0,1]}' },
              bind: { part_id: BUILT },
            },
            ...holes.map(([x, y]) => ({
              tool: "gdt_fcf",
              args: { characteristic: "position", tolerance_mm: 0.2, datum_refs: ["A", "B"], basic: [x, y] },
              bind: { part_id: BUILT, target_face: `step 7: the bore face whose axis passes (${x}, ${y})` },
            })),
          ],
        },
        why:
          "datum A = the seating face (3 DOF), datum B = the left edge face (2 DOF). |A|B|: Z′ = +Z, X′ = +X (B inward), Y′ = +Y, y₀ = part corner y = 0 — so each basic is the hole's drawing coordinate. A third datum would not change these verdicts (the frame uses the first two)",
      },
    ],
    expect: [
      "step 5: fillet ok on 4 edges; boolean_many completed 4 of 4, every step sound",
      "step 6: sound, watertight, manifold, Euler characteristic −6 (genus 4)",
      `step 7: four cylinders Ø ${m8.diameter_mm} mm, axes +Z, at ${holes.map(([x, y]) => `(${x}, ${y})`).join(", ")}`,
      `step 8: verified — ${volume} mm³ within ${r3(volume * 1e-4)} mm³`,
      "step 9: flatness and all four positions IN SPEC at ~0.00 on a perfect build, each disclosing its DRF",
    ],
    state_as_uncertified: [
      `The D${m8.diameter_mm} clearance is the house default class (ISO 273 medium), not the engineer's choice — say so.`,
      `Edge distance and spacing were compared against EN 1993-1-8 Table 3.3 minimums (${edge.e1_min_mm} / ${edge.p2_min_mm} mm); that is a table comparison for structural steel, not a kernel check and not a load rating.`,
      "ISO 2768-m general tolerances are design intent on the drawing; the kernel does not verify them.",
      "No machining rules were checked: the kernel has no CNC manufacturability rules.",
    ],
    how_to_read: HOW_TO_READ,
    static_check: STATIC_CHECK,
  };
}

const BUILDERS: Record<string, () => WorkedExample> = {
  aerofoil_section: aerofoilExample,
  sheet_metal_bracket: bracketExample,
  mounting_plate: plateExample,
};

export const WORKED_EXAMPLE_KEYS = Object.keys(BUILDERS);

const cache = new Map<string, WorkedExample>();

/**
 * The worked example for a playbook key, built on first request from the
 * cited references and cached. Lazy on purpose: an example whose reference
 * stops answering must cost that ONE example (a typed `refused` the caller
 * reports), never the whole tool table at server start.
 * `null` = this playbook has no worked example.
 */
export function workedExample(key: string): WorkedExample | { refused: true; reason: string } | null {
  const build = BUILDERS[key];
  if (!build) return null;
  const hit = cache.get(key);
  if (hit) return hit;
  try {
    const ex = build();
    cache.set(key, ex);
    return ex;
  } catch (e) {
    return {
      refused: true,
      reason: `the worked example for '${key}' could not be built from its cited references (${e instanceof Error ? e.message : String(e)}) — follow the playbook text and the references directly`,
    };
  }
}
