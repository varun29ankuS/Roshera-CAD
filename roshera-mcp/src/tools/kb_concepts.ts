/**
 * The `concepts` playbook's worked example (concept board, 2026-11-27): a
 * motor-to-frame bracket, three ways, as the `roshera:concept` card the agent
 * posts -- and concept A built through the kernel, its program GENERATED from
 * the recipe by `recipeToProgram` (the mapping the playbook states, in code),
 * so the example cannot drift from the mapping.
 *
 * The concept set below is plain JSON between two markers: roshera-app's
 * test/concept-recipe.test.mjs reads it out of this file and validates it
 * against the card's own schema (roshera-app/src/lib/concept-recipe.ts), so
 * the agent is taught exactly what the board accepts. Keep it JSON (quoted
 * keys, no trailing commas, no expressions).
 *
 * Every hole size in it is a cited reference value, re-checked by
 * test/kb_playbooks.test.mjs: M5 and M3 clearance at the ISO 273 medium
 * class (the house default -- the playbook says to state that).
 */

import { referenceLookup } from "./kb_reference.js";
import type { WorkedExample } from "./kb_examples.js";

type V3 = [number, number, number];

interface ConceptShape {
  kind: "box" | "plate" | "cylinder" | "cone" | "rib" | "slot";
  [field: string]: unknown;
}
interface ConceptStep {
  op: "union" | "subtract" | "intersect";
  shape: ConceptShape;
  blend?: number;
}
export interface ConceptRecipe {
  steps: ConceptStep[];
}
interface ConceptPayload {
  id: string;
  title: string;
  recipe: ConceptRecipe;
  [field: string]: unknown;
}
export interface ConceptSetPayload {
  id?: string;
  title?: string;
  material?: { name: string; density: number };
  concepts: ConceptPayload[];
}

export const BRACKET_CONCEPT_SET: ConceptSetPayload = /* concept-example:begin */ {
  "id": "motor-bracket",
  "title": "NEMA 17 motor to 20 mm frame",
  "material": { "name": "6082-T6", "density": 2700 },
  "concepts": [
    {
      "id": "A",
      "title": "L bracket, two gussets",
      "recipe": {
        "steps": [
          { "op": "union", "shape": { "kind": "box", "center": [0, 0, 0], "width": 60, "depth": 40, "height": 5 } },
          { "op": "union", "shape": { "kind": "box", "center": [0, -17.5, 5], "width": 60, "depth": 5, "height": 50 } },
          { "op": "union", "shape": { "kind": "rib", "origin": [25, -15, 5], "leg_a": [0, 30, 0], "leg_b": [0, 0, 30], "thickness": 5 } },
          { "op": "union", "shape": { "kind": "rib", "origin": [-25, -15, 5], "leg_a": [0, 30, 0], "leg_b": [0, 0, 30], "thickness": 5 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [0, -22, 30], "axis": [0, 1, 0], "radius": 11.5, "height": 9 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [15.5, -22, 14.5], "axis": [0, 1, 0], "radius": 1.7, "height": 9 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [-15.5, -22, 14.5], "axis": [0, 1, 0], "radius": 1.7, "height": 9 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [15.5, -22, 45.5], "axis": [0, 1, 0], "radius": 1.7, "height": 9 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [-15.5, -22, 45.5], "axis": [0, 1, 0], "radius": 1.7, "height": 9 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [15, 10, -1], "radius": 2.75, "height": 7 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [-15, 10, -1], "radius": 2.75, "height": 7 } }
        ]
      },
      "numbers": [
        { "label": "bore deflection, gussets ignored", "value": 0.0024, "unit": "mm", "basis": "cantilever F L^3 / 3 E I, 20 N, E 70 GPa" }
      ],
      "note": "stiffest; motor bores need a side setup",
      "supports": [{ "at": [0, 10, 0], "label": "frame bolts" }],
      "loads": [{ "at": [0, -17.5, 30], "dir": [0, 0, -1], "magnitude_n": 20, "label": "motor weight" }]
    },
    {
      "id": "B",
      "title": "Flat plate, motor face-mounted",
      "recipe": {
        "steps": [
          { "op": "union", "shape": { "kind": "plate", "center": [0, 0, 0], "width": 60, "depth": 60, "height": 6, "corner_radius": 6 } },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [0, 0, -1], "radius": 11.5, "height": 8 } },
          { "op": "subtract", "shape": { "kind": "slot", "center": [0, 24, -1], "dir": [1, 0, 0], "normal": [0, 0, 1], "length": 30, "width": 5.5, "depth": 8 } }
        ]
      },
      "note": "one setup; frame bolt slides in the slot",
      "supports": [{ "at": [0, 24, 0], "label": "frame bolt" }],
      "loads": [{ "at": [0, 0, 6], "dir": [0, 0, -1], "magnitude_n": 20, "label": "motor weight" }]
    },
    {
      "id": "C",
      "title": "Round saddle",
      "recipe": {
        "steps": [
          { "op": "union", "shape": { "kind": "box", "center": [0, 0, 0], "width": 56, "depth": 50, "height": 8 } },
          { "op": "union", "shape": { "kind": "cone", "center": [0, 0, 8], "base_radius": 18, "top_radius": 13, "height": 20 }, "blend": 3 },
          { "op": "subtract", "shape": { "kind": "cylinder", "center": [0, 0, -1], "radius": 11.5, "height": 30 } }
        ]
      },
      "note": "round, so turned or printed; heaviest"
    }
  ]
} /* concept-example:end */;

// ─── recipe -> kernel program (the playbook's mapping, executable) ──────────

const r9 = (v: number) => Math.round(v * 1e9) / 1e9;
const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const add = (a: V3, b: V3): V3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const scale = (a: V3, k: number): V3 => [a[0] * k, a[1] * k, a[2] * k];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a: V3) => Math.sqrt(dot(a, a));
const unit = (a: V3): V3 => scale(a, 1 / norm(a));
const tidy = (a: V3): V3 => [r9(a[0]), r9(a[1]), r9(a[2])];
const isDefault = (a: unknown, d: V3) => a === undefined || (Array.isArray(a) && a.every((x, i) => x === d[i]));

export interface ProgramOp {
  tool: string;
  args: Record<string, unknown>;
}

/** The ops that make one shape, and the index (within them) of the op whose
 *  result is the shape's solid. */
function shapeOps(s: ConceptShape, at: number): { ops: ProgramOp[]; solid: number } {
  const v = (k: string) => s[k] as V3;
  const n = (k: string) => s[k] as number;
  switch (s.kind) {
    case "box":
    case "plate": {
      const u = (s.u_axis as V3 | undefined) ?? [1, 0, 0];
      const w = (s.v_axis as V3 | undefined) ?? [0, 1, 0];
      const frame = isDefault(s.u_axis, [1, 0, 0]) && isDefault(s.v_axis, [0, 1, 0]) ? {} : { plane: { origin: [0, 0, 0], u_axis: u, v_axis: w } };
      const box: ProgramOp = { tool: "create_box", args: { ...frame, center: v("center"), width: n("width"), depth: n("depth"), height: n("height") } };
      if (s.kind === "box") return { ops: [box], solid: 0 };
      // Plate: round the four edges parallel to the height direction, each the
      // edge furthest toward its corner (the mounting_plate playbook's recipe).
      const normal = tidy(unit(cross(u, w)));
      const corners: Array<[number, number]> = [[1, 1], [-1, 1], [1, -1], [-1, -1]];
      const selects: ProgramOp[] = corners.map(([su, sv]) => ({
        tool: "select_edge",
        args: { part_id: `$${at}.part_id`, curve_kind: "line", direction: normal, extremal: "most_along", along: tidy(add(scale(u, su), scale(w, sv))) },
      }));
      const fillet: ProgramOp = {
        tool: "fillet_edges",
        args: { part_id: `$${at}.part_id`, radius: n("corner_radius"), edge_ids: corners.map((_, i) => `$${at + 1 + i}.edge_id`) },
      };
      return { ops: [box, ...selects, fillet], solid: 5 };
    }
    case "cylinder":
      return { ops: [{ tool: "create_cylinder", args: { center: v("center"), axis: (s.axis as V3 | undefined) ?? [0, 0, 1], radius: n("radius"), height: n("height") } }], solid: 0 };
    case "cone":
      return {
        ops: [{ tool: "create_cone", args: { center: v("center"), axis: (s.axis as V3 | undefined) ?? [0, 0, 1], base_radius: n("base_radius"), top_radius: n("top_radius"), height: n("height") } }],
        solid: 0,
      };
    case "rib": {
      const a = v("leg_a");
      const b = v("leg_b");
      const u = unit(a);
      const w = unit(sub(b, scale(u, dot(b, u))));
      const nn = cross(u, w);
      const ht = n("thickness") / 2;
      const points = [[0, 0], [r9(norm(a)), 0], [r9(dot(b, u)), r9(dot(b, w))]];
      return {
        ops: [
          { tool: "psketch_begin", args: {} },
          { tool: "psketch_add_entity", args: { csketch_id: `$${at}.csketch_id`, kind: "polyline", params: { points, closed: true } } },
          {
            tool: "psketch_extrude",
            args: { csketch_id: `$${at}.csketch_id`, distance: n("thickness"), plane: { origin: tidy(sub(v("origin"), scale(nn, ht))), u_axis: tidy(u), v_axis: tidy(w) } },
          },
        ],
        solid: 2,
      };
    }
    case "slot": {
      const dir = v("dir");
      const normal = v("normal");
      const across = tidy(cross(normal, dir));
      const hl = (n("length") - n("width")) / 2;
      const r = n("width") / 2;
      const c = v("center");
      return {
        ops: [
          { tool: "create_box", args: { plane: { origin: [0, 0, 0], u_axis: dir, v_axis: across }, center: c, width: r9(2 * hl), depth: n("width"), height: n("depth") } },
          { tool: "create_cylinder", args: { center: tidy(add(c, scale(dir, hl))), axis: normal, radius: r9(r), height: n("depth") } },
          { tool: "create_cylinder", args: { center: tidy(add(c, scale(dir, -hl))), axis: normal, radius: r9(r), height: n("depth") } },
          { tool: "boolean_many", args: { op: "union", base: `$${at}.object_uuid`, tools: [`$${at + 1}.object_uuid`, `$${at + 2}.object_uuid`] } },
        ],
        solid: 3,
      };
    }
  }
}

const BOOLEAN_OP = { union: "union", subtract: "difference", intersect: "intersection" } as const;

/**
 * The cad_program ops that build `recipe` one step per kernel verb: the first
 * shape is the body, each later shape is made and then combined with the
 * body by one `boolean`. A blend cannot be planned statically -- its junction
 * edges exist only after the union -- so a recipe with one is refused here
 * with the procedure to follow instead.
 */
export function recipeToProgram(recipe: ConceptRecipe): { ops: ProgramOp[] } | { refused: true; reason: string } {
  const blended = recipe.steps.findIndex((s) => s.blend !== undefined);
  if (blended >= 0) {
    return {
      refused: true,
      reason:
        `step ${blended} carries a blend: run the program up to that union, name every junction edge it made ` +
        "(select_edge, convexity concave, once per edge), fillet_edges them at the blend radius, then continue -- " +
        "and if any edge cannot be named or the fillet refuses, refuse the build with that reason",
    };
  }
  const ops: ProgramOp[] = [];
  let body = -1;
  recipe.steps.forEach((step, i) => {
    const made = shapeOps(step.shape, ops.length);
    const solid = ops.length + made.solid;
    ops.push(...made.ops);
    if (i === 0) {
      body = solid;
      return;
    }
    ops.push({ tool: "boolean", args: { op: BOOLEAN_OP[step.op], object_a: `$${body}.object_uuid`, object_b: `$${solid}.object_uuid` } });
    body = ops.length - 1;
  });
  return { ops };
}

// ─── the worked example ─────────────────────────────────────────────────────

function clearance(fastener: string): number {
  const r = referenceLookup("clearance_hole", { fastener });
  if ("refused" in r) throw new Error(`clearance_hole ${fastener} refused: ${r.reason}`);
  return (r.value as { diameter_mm: number }).diameter_mm;
}

/** Concept A's exact volume, by hand: plates + gussets - bores. */
export function bracketAVolume(): number {
  const plates = 60 * 40 * 5 + 60 * 5 * 50;
  const gussets = 2 * 0.5 * 30 * 30 * 5;
  const bores = Math.PI * 5 * (11.5 * 11.5 + 4 * 1.7 * 1.7 + 2 * 2.75 * 2.75);
  return Math.round((plates + gussets - bores) * 1000) / 1000;
}

export function conceptsExample(): WorkedExample {
  const m5 = clearance("M5");
  const m3 = clearance("M3");
  const a = BRACKET_CONCEPT_SET.concepts[0];
  const program = recipeToProgram(a.recipe);
  if ("refused" in program) throw new Error(`concept A must build statically: ${program.reason}`);
  const volume = bracketAVolume();
  const built = `the part_id of step 5's last op (ledger op ${program.ops.length - 1})`;
  const fence = "```roshera:concept\n" + JSON.stringify(BRACKET_CONCEPT_SET) + "\n```";
  return {
    spec:
      "Engineer: \"a bracket to hold this NEMA 17 motor to the 20 mm frame\" -- the job is clear, the form and the sizes are not, " +
      "so the reply is three concepts, not a part. The motor face (42.3 mm square, 22 mm pilot, M3 on a 31 mm square) and the " +
      `frame slot (M5) are the facts; holes are ISO 273 medium clearance, the house default (M5 D${m5}, M3 D${m3}) -- say so. ` +
      "The engineer picks A; A is built exactly as drawn and certified.",
    steps: [
      { step: 1, tool: "kb_lookup", args: { kind: "reference", key: "clearance_hole", args: { fastener: "M5" } }, why: `frame bolt clearance D${m5} (house default class)` },
      { step: 2, tool: "kb_lookup", args: { kind: "reference", key: "clearance_hole", args: { fastener: "M3" } }, why: `motor screw clearance D${m3} (house default class)` },
      {
        step: 3,
        tool: "blackboard_add_entry",
        args: { text: fence },
        why: "ONE concept card, three genuinely different ways: an L with gussets (stiffest, but its motor bores need a side setup), a flat plate (one setup), a round saddle (turned or printed). Each note is its trade-off; the one number carries its basis",
      },
      {
        step: 4,
        tool: "timeline_checkpoint",
        args: {
          name: "Motor bracket concept A: L 60 x 40 x 5 base, 50 mm upright, two 30 mm gussets, D23 pilot, 4x D3.4 on 31 sq, 2x D5.5 base",
          description: "Built exactly as concept A's recipe: each shape one kernel verb, combined in order by boolean.",
        },
        why: "the engineer answered \"Build concept A\"; declare that intent",
      },
      {
        step: 5,
        tool: "cad_program",
        args: { name: "motor bracket concept A", ops: program.ops },
        why: "recipeToProgram(concept A): box -> create_box, rib -> triangle sketch extruded on its own plane, cylinder -> create_cylinder, each later shape combined by one boolean (subtract = difference)",
      },
      { step: 6, tool: "verify_part", args: {}, bind: { part_id: built }, why: "the certificate -- only the built part has one" },
      {
        step: 7,
        tool: "verify_claim",
        args: { expr: "v", bindings: [{ var: "v", measure: { kind: "volume" } }], expected: volume, tolerance: Math.round(volume * 1e-4 * 1000) / 1000 },
        bind: { "bindings.0.measure.part": `the object_uuid of step 5's last op (ledger op ${program.ops.length - 1})` },
        why: `plates 27000 + gussets 4500 - pilot, motor and frame bores = ${volume} mm³: the kernel built the recipe that was shown`,
      },
    ],
    expect: [
      "step 3: the board shows three rotatable sketches, each marked CONCEPT, with the board's own volume and mass estimate under each",
      `step 5: every op sound; ${program.ops.length} ops`,
      "step 6: sound",
      `step 7: verified -- ${volume} mm³`,
    ],
    state_as_uncertified: [
      "Every number on the concept card is an estimate (the deflection by cantilever theory, the board's volume by sampling the sketch); only the built part's measurements are the kernel's.",
      "The load path and maker views are heuristics (straight load lines, a straight tool along Z, a 45 degree overhang rule), not analyses.",
      `The clearance holes are the house default class (ISO 273 medium: M5 D${m5}, M3 D${m3}), not the engineer's choice.`,
      "Concept C's blend is drawn as a round union; the kernel builds it as a fillet on the junction edges or refuses -- never a stand-in.",
    ],
    how_to_read:
      "Send each step as written. `bind` is NOT an argument: fill each named arg (a dotted path into args) from the earlier result it names, then drop the bind key. '$N.key' inside a cad_program is that program's own chaining and is sent as is.",
    static_check:
      "validated statically against the MCP tool schemas (names, argument names and types) and the concept card's own schema; not executed against a live kernel by the knowledge base -- read every ledger and verdict as it comes back",
  };
}
