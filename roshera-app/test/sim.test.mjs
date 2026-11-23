import { test } from 'node:test'
import assert from 'node:assert/strict'

const { forceOf, setupGaps, studyOf, simRunOf, verdictView, stressColor, fieldBuffers, deformScale, fieldRange } =
  await import('../src/lib/sim.ts')

const material = { name: 'Al 6061-T6', youngs_modulus_mpa: '68900', poisson_ratio: '0.33', yield_strength_mpa: '276', source: 'customer datasheet' }
const setup = (over = {}) => ({
  supports: [{ kind: 'fixed', faces: [3] }],
  loads: [{ faces: [7], direction: '-z', newtons: 100, spread: 'even', normal: [0, 0, 1] }],
  material,
  minSafetyFactor: '2',
  meshSizeMm: '',
  ...over,
})

test('a load pushing into its face acts against the outward normal, at its full size', () => {
  const f = forceOf({ faces: [1], direction: 'into', newtons: 50, spread: 'even', normal: [0, 2, 0] })
  assert.deepEqual(f, [-0, -50, -0])
  assert.deepEqual(forceOf({ faces: [1], direction: '+x', newtons: 5, spread: 'even', normal: null }), [5, 0, 0])
  assert.equal(forceOf({ faces: [1], direction: 'into', newtons: 5, spread: 'even', normal: null }), null)
})

test('a complete set-up becomes the kernel study verbatim; optional values are left out, not zeroed', () => {
  const s = studyOf(setup())
  assert.deepEqual(s, {
    supports: [{ kind: 'fixed', faces: [3] }],
    loads: [{ faces: [7], force_n: [0, 0, -100], spread: 'even' }],
    material: { name: 'Al 6061-T6', youngs_modulus_mpa: 68900, poisson_ratio: 0.33, yield_strength_mpa: 276, source: 'customer datasheet' },
    goal: { min_safety_factor: 2 },
    mesh: {},
  })
  const noYield = studyOf(setup({ material: { ...material, yield_strength_mpa: '', source: '' } }))
  assert.equal('yield_strength_mpa' in noYield.material, false, 'a blank yield is absent, so the kernel refuses a safety factor')
  assert.equal('source' in noYield.material, false)
})

test('an incomplete set-up names what is missing and is not run', () => {
  const s = setup({ supports: [], loads: [], minSafetyFactor: '' })
  const gaps = setupGaps(s)
  assert.ok(gaps.some((g) => g.includes('held')), gaps.join(' | '))
  assert.ok(gaps.some((g) => g.includes('load')), gaps.join(' | '))
  assert.ok(gaps.some((g) => g.includes('safety factor')), gaps.join(' | '))
  assert.equal(studyOf(s), null)
  assert.ok(setupGaps(setup({ material: { ...material, poisson_ratio: '0.5' } })).some((g) => g.includes('Poisson')))
})

test("the route's refusals become distinct outcomes, the free motions kept in words", () => {
  assert.equal(simRunOf(422, { error: 'sim_stale_solid', reason: 'r' }).kind, 'unverified')
  const moving = simRunOf(422, { error: 'sim_part_can_move', reason: 'r', free_motions: ['can slide along x', 7] })
  assert.deepEqual(moving, { kind: 'can_move', reason: 'r', motions: ['can slide along x'] })
  assert.equal(simRunOf(503, { error: 'sim_engine_missing', reason: 'set ROSHERA_GMSH' }).kind, 'engine_missing')
  assert.deepEqual(simRunOf(422, { error: 'sim_refused', reason: 'face 9 is not on solid 1' }), {
    kind: 'refused',
    reason: 'face 9 is not on solid 1',
  })
  assert.equal(simRunOf(200, { verdict: { verdict: 'holds' } }).kind, 'report')
})

test('an undecided study reads as no verdict with its reason -- never as a pass', () => {
  const v = verdictView({ verdict: 'undecided', reason: 'peak stress did not converge' })
  assert.equal(v.state, 'unknown')
  assert.match(v.detail, /converge/)
  assert.equal(verdictView({ verdict: 'holds', safety_factor: 9.21, required: 2 }).state, 'pass')
  assert.equal(verdictView({ verdict: 'fails', safety_factor: 1.4, required: 2 }).state, 'fix')
})

test('the colour scale runs blue to red and clamps outside its range', () => {
  const lo = stressColor(0)
  const hi = stressColor(1)
  assert.ok(lo[2] > lo[0], 'low end is blue')
  assert.ok(hi[0] > hi[2], 'high end is red')
  assert.deepEqual(stressColor(-3), lo)
  assert.deepEqual(stressColor(7), hi)
  assert.deepEqual(stressColor(NaN), lo)
})

const field = {
  positions: [[0, 0, 0], [100, 0, 0], [0, 10, 0]],
  displacement: [[0, 0, 0], [0, 0, -0.2], [0, 0, -0.1]],
  von_mises: [30, 0, 15],
  triangles: [[0, 1, 2]],
  faces: [4],
  min_von_mises: 0,
  max_von_mises: 30,
}

test('result buffers move each vertex by the scaled displacement and colour it by its own value', () => {
  const b = fieldBuffers(field, 'stress', 10)
  assert.deepEqual([...b.index], [0, 1, 2])
  assert.ok(Math.abs(b.positions[1 * 3 + 2] - -2) < 1e-6, 'vertex 1 drawn 10x its 0.2 mm drop')
  assert.deepEqual([...b.colors.slice(0, 3)].map((x) => +x.toFixed(4)), stressColor(1).map((x) => +x.toFixed(4)))
  assert.deepEqual([...b.colors.slice(3, 6)].map((x) => +x.toFixed(4)), stressColor(0).map((x) => +x.toFixed(4)))
  const d = fieldBuffers(field, 'displacement', 1)
  assert.deepEqual([...d.colors.slice(3, 6)].map((x) => +x.toFixed(4)), stressColor(1).map((x) => +x.toFixed(4)))
  assert.deepEqual(fieldRange(field, 'displacement'), { min: 0, max: 0.2, unit: 'mm' })
})

test('the deformation is exaggerated to a round factor showing the largest movement near 5% of the part', () => {
  // 100 mm part, 0.2 mm peak: 5 mm wanted -> 25x -> the round factor below is 20.
  assert.equal(deformScale(field), 20)
  assert.equal(deformScale({ ...field, displacement: field.displacement.map(() => [0, 0, 0]) }), 1)
  assert.equal(deformScale({ ...field, displacement: [[0, 0, 0], [0, 0, -50], [0, 0, 0]] }), 1)
})
