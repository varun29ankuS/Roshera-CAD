// Run: node --experimental-strip-types --test test/concept-pov.test.mjs
//
// The concept card's points of view (lib/concept-pov.ts), each a labelled
// ESTIMATE, never a verdict:
//  - Load path: material near the straight line from each load to each
//    support is emphasised, the rest faded.
//  - Maker, 3-axis milling from +-Z: a surface point is reachable when a
//    straight tool coming down (or up) the Z axis meets no material on the
//    way. The face resting at the lowest Z is the stock face.
//  - Maker, FDM: a surface whose normal points more than 45 degrees past
//    vertical downward is an overhang; the face on the bed is not.
// The shader draws the same rules (concept-glsl.ts); these pin them.
import { test } from 'node:test'
import assert from 'node:assert/strict'

const { parseCard } = await import('../src/lib/blackboard-cards.ts')
const { compileSdf, recipeBounds } = await import('../src/lib/concept-sdf.ts')
const {
  loadPathWeight,
  loadPathSigma,
  loadPathStats,
  surfaceSamples,
  reachableAlongZ,
  isOverhang,
  makerStats,
} = await import('../src/lib/concept-pov.ts')

function recipe(steps) {
  const r = parseCard('concept', JSON.stringify({ concepts: [{ id: 'T', title: 't', recipe: { steps } }] }))
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  return r.card.card.concepts[0].recipe
}

const PLATE = { op: 'union', shape: { kind: 'box', center: [0.3, 0.2, 0.1], width: 60, depth: 40, height: 10 } }
// A horizontal through-bore along X, at mid-height: its wall cannot be
// reached by a straight tool along Z from either side.
const CROSS_BORE = { op: 'subtract', shape: { kind: 'cylinder', center: [-31, 0.2, 5.1], axis: [1, 0, 0], radius: 3, height: 62 } }
// A vertical through-bore: reachable from +Z.
const VERTICAL_BORE = { op: 'subtract', shape: { kind: 'cylinder', center: [0.3, 0.2, -1], radius: 4, height: 12 } }

test('load path weight: 1 on the line, falls off with distance, 0 far away', () => {
  const loads = [{ at: [0, 0, 0], dir: [0, 0, -1] }]
  const supports = [{ at: [100, 0, 0] }]
  assert.equal(loadPathWeight([50, 0, 0], loads, supports, 10), 1)
  const w1 = loadPathWeight([50, 5, 0], loads, supports, 10)
  const w2 = loadPathWeight([50, 15, 0], loads, supports, 10)
  assert.ok(w1 > 0.5 && w2 < 0.2 && w1 > w2, `${w1} ${w2}`)
  assert.ok(loadPathWeight([50, 60, 0], loads, supports, 10) < 1e-6)
  // Beyond the segment's end the distance is to the end point, not the infinite line.
  assert.ok(loadPathWeight([160, 0, 0], loads, supports, 10) < 1e-6)
})

test('load path sigma scales with the part', () => {
  const b = recipeBounds(recipe([PLATE]))
  const s = loadPathSigma(b)
  assert.ok(s > 3 && s < 15, `sigma ${s}`)
})

test('load path stats: a bar between its load and support is nearly all on the path; a wide plate is not', () => {
  const bar = recipe([{ op: 'union', shape: { kind: 'box', center: [50, 0, -2], width: 100, depth: 4, height: 4 } }])
  const along = loadPathStats(bar, [{ at: [0, 0, 0], dir: [0, 0, -1] }], [{ at: [100, 0, 0] }])
  assert.ok(along.on_path_fraction > 0.9, `bar on path ${along.on_path_fraction}`)
  const wide = recipe([{ op: 'union', shape: { kind: 'box', center: [50, 0, -2], width: 100, depth: 100, height: 4 } }])
  const plate = loadPathStats(wide, [{ at: [0, 0, 0], dir: [0, 0, -1] }], [{ at: [100, 0, 0] }])
  assert.ok(plate.on_path_fraction < 0.35, `plate on path ${plate.on_path_fraction}`)
})

test('surface samples lie on the surface, normals point out, areas add up', () => {
  const r = recipe([PLATE])
  const f = compileSdf(r)
  const s = surfaceSamples(r)
  assert.ok(s.samples.length > 500)
  for (const q of s.samples.slice(0, 200)) assert.ok(Math.abs(f(q.p)) < 0.05 * s.cell_mm, 'projected onto the surface')
  const area = s.samples.reduce((a, q) => a + q.area, 0)
  // The faces' area plus the documented convex-edge over-count, (pi/4) h per mm of edge.
  const exact = 2 * (60 * 40 + 60 * 10 + 40 * 10)
  const edges = 4 * (60 + 40 + 10)
  const model = exact + (Math.PI / 4) * s.cell_mm * edges
  assert.ok(Math.abs(area - model) / model < 0.03, `area ${area} vs ${model} (faces ${exact})`)
  const top = s.samples.find((q) => q.p[2] > 10.09 && Math.abs(q.p[0]) < 20 && Math.abs(q.p[1]) < 10)
  assert.ok(top && top.n[2] > 0.99, 'a top-face normal is +Z')
})

test('reach along Z: the top face from +Z, the bottom face from -Z, a cross-bore wall from neither', () => {
  const r = recipe([PLATE, CROSS_BORE])
  const f = compileSdf(r)
  const b = recipeBounds(r)
  const top = { p: [20, 15, 10.1], n: [0, 0, 1] }
  assert.equal(reachableAlongZ(f, b, top.p, top.n, +1), true)
  assert.equal(reachableAlongZ(f, b, top.p, top.n, -1), false)
  const bottom = { p: [20, 15, 0.1], n: [0, 0, -1] }
  assert.equal(reachableAlongZ(f, b, bottom.p, bottom.n, -1), true)
  assert.equal(reachableAlongZ(f, b, bottom.p, bottom.n, +1), false)
  // The bore's upper wall faces down (-Z), its lower wall up: both are roofed.
  const boreTop = { p: [10, 0.2, 8.1], n: [0, 0, -1] }
  const boreBottom = { p: [10, 0.2, 2.1], n: [0, 0, 1] }
  for (const q of [boreTop, boreBottom]) {
    assert.equal(reachableAlongZ(f, b, q.p, q.n, +1), false)
    assert.equal(reachableAlongZ(f, b, q.p, q.n, -1), false)
  }
  // A side wall is reachable from +Z (a straight tool runs down it).
  assert.equal(reachableAlongZ(f, b, [30.3, 10, 5], [1, 0, 0], +1), true)
})

test('maker stats: a plate with a vertical bore is one setup; a cross bore is not reachable from Z', () => {
  const one = makerStats(recipe([PLATE, VERTICAL_BORE]))
  assert.equal(one.setups, 1, JSON.stringify(one))
  assert.ok(one.mill.neither < 0.005, `neither ${one.mill.neither}`)
  const cross = makerStats(recipe([PLATE, CROSS_BORE]))
  assert.equal(cross.setups, null, 'unreachable from +-Z: no straight-tool setup count')
  const boreArea = (2 * Math.PI * 3 * 60) / (2 * (60 * 40 + 60 * 10 + 40 * 10) - 2 * Math.PI * 9 + 2 * Math.PI * 3 * 60)
  assert.ok(cross.mill.neither > 0.4 * boreArea, `neither ${cross.mill.neither} vs bore share ${boreArea}`)
})

test('maker stats: an underside step needs the -Z setup', () => {
  // A T: a wide flange on a narrow stem -- the flange underside faces down, above the bed.
  const r = recipe([
    { op: 'union', shape: { kind: 'box', center: [0.2, 0.1, 0], width: 10, depth: 10, height: 20 } },
    { op: 'union', shape: { kind: 'box', center: [0.2, 0.1, 20], width: 40, depth: 40, height: 4 } },
  ])
  const m = makerStats(r)
  assert.equal(m.setups, 2, JSON.stringify(m))
  assert.ok(m.fdm.overhang > 0.2, `the flange underside overhangs: ${m.fdm.overhang}`)
})

test('FDM overhang: more than 45 degrees past vertical, downward; the bed face is not one', () => {
  const c = Math.cos(Math.PI / 4)
  assert.equal(isOverhang([0, 0, -1], 5, 0, 0.5), true, 'a ceiling above the bed overhangs')
  assert.equal(isOverhang([0, 0, -1], 0.1, 0, 0.5), false, 'the face on the bed does not')
  assert.equal(isOverhang([1, 0, 0], 5, 0, 0.5), false, 'a wall does not')
  assert.equal(isOverhang([c, 0, -c], 5, 0, 0.5), false, 'exactly 45 degrees is printable')
  const steep = [Math.sin(0.6), 0, -Math.cos(0.6)] // 34 degrees from straight down
  assert.equal(isOverhang(steep, 5, 0, 0.5), true)
  const plain = makerStats(recipe([PLATE]))
  assert.ok(plain.fdm.overhang < 0.005, `a plain box prints without support: ${plain.fdm.overhang}`)
})

test('a cutting tool that overshoots into the air leaves no phantom surface behind', () => {
  // The bore's tool runs 1 mm past the top of the boss. Inside it, the
  // subtract field is a bound whose gradient points at the tool's cap in the
  // air; a sample must never settle there (it read as a downward face, an
  // FDM overhang that does not exist).
  const r = recipe([
    { op: 'union', shape: { kind: 'box', center: [0.2, 0.1, 0], width: 56, depth: 50, height: 8 } },
    { op: 'union', shape: { kind: 'cone', center: [0.2, 0.1, 8], base_radius: 18, top_radius: 13, height: 20 } },
    { op: 'subtract', shape: { kind: 'cylinder', center: [0.2, 0.1, -1], radius: 11.5, height: 30 } },
  ])
  const f = compileSdf(r)
  const s = surfaceSamples(r, 36)
  const off = s.samples.filter((q) => Math.abs(f(q.p)) > 0.05 * s.cell_mm)
  assert.equal(off.length, 0, `${off.length} samples off the surface`)
  const m = makerStats(r, 36)
  assert.ok(m.fdm.overhang < 0.005, `a cone narrowing upward on a box has no overhang: ${m.fdm.overhang}`)
  assert.equal(m.setups, 1, JSON.stringify(m))
})
