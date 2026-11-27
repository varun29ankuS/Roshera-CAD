// Shared payloads for the concept-board tests (not a test file itself).
//
// A motor-to-frame bracket, three ways -- the same worked example the
// `concepts` playbook teaches (roshera-mcp/src/tools/kb_concepts.ts). Every
// shape uses the kernel verbs' own conventions: a box's `center` is the
// centre of its BASE face, a cylinder's and a cone's `center` the centre of
// their base, extruded along +axis.

export const L_BRACKET = {
  id: 'A',
  title: 'L bracket, two gussets',
  recipe: {
    steps: [
      { op: 'union', shape: { kind: 'box', center: [0, 0, 0], width: 60, depth: 40, height: 5 } },
      { op: 'union', shape: { kind: 'box', center: [0, -17.5, 5], width: 60, depth: 5, height: 50 } },
      { op: 'union', shape: { kind: 'rib', origin: [25, -15, 5], leg_a: [0, 30, 0], leg_b: [0, 0, 30], thickness: 5 } },
      { op: 'union', shape: { kind: 'rib', origin: [-25, -15, 5], leg_a: [0, 30, 0], leg_b: [0, 0, 30], thickness: 5 } },
      { op: 'subtract', shape: { kind: 'cylinder', center: [0, -22, 30], axis: [0, 1, 0], radius: 11.5, height: 9 } },
    ],
  },
  numbers: [{ label: 'tip deflection', value: 0.02, unit: 'mm', basis: 'beam theory, gusset as a cantilever' }],
  note: 'stiffest; motor bores need a side setup',
  supports: [{ at: [0, 10, 0], label: 'frame bolts' }],
  loads: [{ at: [0, -17.5, 30], dir: [0, 0, -1], magnitude_n: 20, label: 'motor weight' }],
}

export const FLAT_PLATE = {
  id: 'B',
  title: 'Flat plate on standoffs',
  recipe: {
    steps: [
      { op: 'union', shape: { kind: 'plate', center: [0, 0, 0], width: 60, depth: 60, height: 6, corner_radius: 6 } },
      { op: 'subtract', shape: { kind: 'cylinder', center: [0, 0, -1], radius: 11.5, height: 8 } },
      { op: 'subtract', shape: { kind: 'slot', center: [0, 24, -1], dir: [1, 0, 0], normal: [0, 0, 1], length: 30, width: 5.5, depth: 8 } },
    ],
  },
  note: 'one setup',
  supports: [{ at: [0, 24, 0] }],
  loads: [{ at: [0, 0, 6], dir: [0, 0, -1] }],
}

export const PRINTED_CLAMP = {
  id: 'C',
  title: 'Round saddle',
  recipe: {
    steps: [
      { op: 'union', shape: { kind: 'box', center: [0, 0, 0], width: 56, depth: 50, height: 8 } },
      { op: 'union', shape: { kind: 'cone', center: [0, 0, 8], base_radius: 18, top_radius: 13, height: 20 }, blend: 3 },
      { op: 'subtract', shape: { kind: 'cylinder', center: [0, 0, -1], radius: 11.5, height: 30 } },
    ],
  },
  note: 'round, so turned or printed; heaviest',
}

export const BRACKET_SET = {
  id: 'motor-bracket',
  title: 'Motor-to-frame bracket',
  material: { name: '6082-T6', density: 2700 },
  concepts: [L_BRACKET, FLAT_PLATE, PRINTED_CLAMP],
}

export const clone = (v) => JSON.parse(JSON.stringify(v))
