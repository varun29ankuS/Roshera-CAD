// Run: node --experimental-strip-types --test test/concept-sdf.test.mjs
//
// The concept sketch's geometry: the signed distance field a recipe defines
// (lib/concept-sdf.ts), its bounds, and the volume / mass estimate the card
// prints under each concept. The volume is an ESTIMATE by sampling the field
// on a grid; these tests pin how good it is against closed forms, on shapes
// placed OFF the grid (a grid-aligned box is exact by accident).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { BRACKET_SET } from './concept-fixtures.mjs'

const { parseCard } = await import('../src/lib/blackboard-cards.ts')
const { compileSdf, recipeBounds, estimateVolume, massGrams } = await import('../src/lib/concept-sdf.ts')

/** A recipe through the real schema, so defaults are filled the same way. */
function recipe(steps) {
  const r = parseCard('concept', JSON.stringify({ concepts: [{ id: 'T', title: 't', recipe: { steps } }] }))
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  return r.card.card.concepts[0].recipe
}
const near = (a, b, tol, what) => assert.ok(Math.abs(a - b) <= tol, `${what}: ${a} vs ${b} (tol ${tol})`)
const relNear = (a, b, rel, what) => near(a, b, Math.abs(b) * rel, what)

const S = Math.SQRT1_2
// A 40 x 30 x 10 box, rotated 45 degrees about Z, base centre off the grid.
const ROTATED_BOX = [
  { op: 'union', shape: { kind: 'box', center: [3.3, -1.7, 2.9], u_axis: [S, S, 0], v_axis: [-S, S, 0], width: 40, depth: 30, height: 10 } },
]

test('a box field: inside negative, outside positive, zero on the faces', () => {
  const f = compileSdf(recipe([{ op: 'union', shape: { kind: 'box', center: [0, 0, 0], width: 40, depth: 30, height: 10 } }]))
  near(f([0, 0, 5]), -5, 1e-9, 'centre is 5 mm deep')
  near(f([0, 0, 10]), 0, 1e-9, 'top face')
  near(f([0, 0, 0]), 0, 1e-9, 'base face (the base-centre convention)')
  near(f([25, 0, 5]), 5, 1e-9, '5 mm off the +X face')
  near(f([23, 18, 5]), Math.hypot(3, 3), 1e-9, 'edge distance is Euclidean')
})

test('rotated box: its frame is honoured', () => {
  const f = compileSdf(recipe(ROTATED_BOX))
  // 19 mm along u from the middle is inside; 21 mm is outside.
  const mid = [3.3, -1.7, 2.9 + 5]
  assert.ok(f([mid[0] + 19 * S, mid[1] + 19 * S, mid[2]]) < 0)
  assert.ok(f([mid[0] + 21 * S, mid[1] + 21 * S, mid[2]]) > 0)
})

test('cylinder, cone and rib fields match their closed forms at known points', () => {
  const cyl = compileSdf(recipe([{ op: 'union', shape: { kind: 'cylinder', center: [1, 2, 3], axis: [1, 0, 0], radius: 5, height: 20 } }]))
  near(cyl([11, 2, 3]), -5, 1e-9, 'cylinder axis point mid-length')
  near(cyl([11, 2, 10]), 2, 1e-9, '2 mm outside the lateral face')
  near(cyl([-2, 2, 3]), 3, 1e-9, '3 mm beyond the base cap')
  const cone = compileSdf(recipe([{ op: 'union', shape: { kind: 'cone', center: [0, 0, 0], base_radius: 10, top_radius: 0, height: 10 } }]))
  near(cone([0, 0, 5]), -5 / Math.SQRT2, 1e-9, 'a 45 degree cone: axis point at mid-height')
  near(cone([0, 0, -1]), 1, 1e-9, 'below the base')
  const rib = compileSdf(recipe([{ op: 'union', shape: { kind: 'rib', origin: [0, 0, 0], leg_a: [10, 0, 0], leg_b: [0, 0, 10], thickness: 4 } }]))
  near(rib([2, 0, 2]), -2, 1e-9, 'inside the triangle, half the thickness to the side face')
  near(rib([2, 5, 2]), 3, 1e-9, '3 mm off the side face')
})

test('booleans: union is the min, subtract cuts, intersect keeps the overlap', () => {
  const base = { op: 'union', shape: { kind: 'box', center: [0, 0, 0], width: 20, depth: 20, height: 10 } }
  const bore = { kind: 'cylinder', center: [0, 0, -1], radius: 4, height: 12 }
  const cut = compileSdf(recipe([base, { op: 'subtract', shape: bore }]))
  assert.ok(cut([0, 0, 5]) > 0, 'the bore axis is empty')
  assert.ok(cut([7, 0, 5]) < 0, 'beside the bore is metal')
  const both = compileSdf(recipe([base, { op: 'intersect', shape: bore }]))
  assert.ok(both([0, 0, 5]) < 0 && both([7, 0, 5]) > 0, 'intersect keeps only the core')
})

test('a union blend adds material in the inside corner, and only there', () => {
  const plateA = { op: 'union', shape: { kind: 'box', center: [0, 0, 0], width: 40, depth: 40, height: 5 } }
  const wall = { kind: 'box', center: [0, -17.5, 5], width: 40, depth: 5, height: 30 }
  const sharp = compileSdf(recipe([plateA, { op: 'union', shape: wall }]))
  const round = compileSdf(recipe([plateA, { op: 'union', shape: wall, blend: 4 }]))
  const corner = [0, -14, 6] // 1 mm off both faces of the inside corner
  assert.ok(sharp(corner) > 0, 'empty without a blend')
  assert.ok(round(corner) < 0, 'filled by a 4 mm blend')
  near(round([0, 10, 10]), sharp([0, 10, 10]), 1e-9, 'far from the junction the blend changes nothing')
})

test('bounds enclose the union and shrink for an intersect', () => {
  const b = recipeBounds(recipe(ROTATED_BOX))
  // Corners at +-20 along u = (S, S, 0) and +-15 along v = (-S, S, 0): x reaches 35 S.
  assert.ok(b.max[0] >= 3.3 + 35 * S - 1e-9 && b.min[0] <= 3.3 - 35 * S + 1e-9)
  assert.ok(b.min[2] <= 2.9 + 1e-9 && b.max[2] >= 12.9 - 1e-9)
})

test('volume estimate: a rotated, off-grid box within 1%', () => {
  const v = estimateVolume(recipe(ROTATED_BOX))
  relNear(v.volume_mm3, 40 * 30 * 10, 0.01, 'rotated box volume')
})

test('volume estimate: a tilted cylinder within 1%', () => {
  const r = recipe([{ op: 'union', shape: { kind: 'cylinder', center: [2.1, 0.4, -3.3], axis: [0.6, 0, 0.8], radius: 6, height: 25 } }])
  relNear(estimateVolume(r).volume_mm3, Math.PI * 36 * 25, 0.01, 'cylinder volume')
})

test('volume estimate: a box minus a through bore within 1%', () => {
  const r = recipe([
    { op: 'union', shape: { kind: 'box', center: [0.37, 0.21, 0.13], width: 50, depth: 40, height: 12 } },
    { op: 'subtract', shape: { kind: 'cylinder', center: [0.37, 0.21, -2], radius: 8, height: 16 } },
  ])
  relNear(estimateVolume(r).volume_mm3, 50 * 40 * 12 - Math.PI * 64 * 12, 0.01, 'box minus bore')
})

test('mutation control: a wrong half-extent is caught by the same 1% check', () => {
  const wrong = recipe([{ op: 'union', shape: { kind: 'box', center: [3.3, -1.7, 2.9], u_axis: [S, S, 0], v_axis: [-S, S, 0], width: 41, depth: 30, height: 10 } }])
  const v = estimateVolume(wrong).volume_mm3
  assert.ok(Math.abs(v - 12000) > 0.01 * 12000, `a 41 mm box must not pass as 40 mm: ${v}`)
})

test('mass from density: grams = mm^3 x kg/m^3 x 1e-6', () => {
  near(massGrams(12000, 2700), 32.4, 1e-9, '12 cm^3 of aluminium')
})

// A correctness test does not assert wall-clock time: it failed at 400 ms
// while a kernel build loaded the machine (2026-11-27), as the kernel's own
// shell-creation timing test did. The time is reported, not judged.
test('every concept in the bracket set has material, and its estimate time is reported', () => {
  const r = parseCard('concept', JSON.stringify(BRACKET_SET))
  assert.equal(r.ok, true)
  for (const c of r.card.card.concepts) {
    const t0 = performance.now()
    const v = estimateVolume(c.recipe)
    const ms = performance.now() - t0
    assert.ok(v.volume_mm3 > 0, `${c.id} has material`)
    console.log(`concept ${c.id}: volume estimate in ${ms.toFixed(0)} ms`)
  }
})
