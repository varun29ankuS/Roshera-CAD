// The sketch workbench's drawing logic (Varun 2026-11-14: "the sketching is
// severely underworked ... nothing compared to how state of art works").
// Every tool builds REAL sketch geometry -- shared corner points, lines
// between them, the constraints a CAD sketcher adds as you draw -- so the
// sketch can be dimensioned and edited afterwards, not a frozen outline.
import { test } from 'node:test'
import assert from 'node:assert/strict'

const lib = () => import('../src/lib/sketch-bench.ts')
const near = (a, b, eps = 1e-9) => Math.abs(a - b) < eps
const count = (plan, op) => plan.filter((o) => o.op === op).length
const kinds = (plan) => plan.filter((o) => o.op === 'constrain').map((o) => o.kind)

test('a rectangle is four shared corners, four lines, held square', async () => {
  const { planShape } = await lib()
  const plan = planShape('rectangle', [{ at: [0, 0] }, { at: [10, 5] }])
  assert.equal(count(plan, 'point'), 4)
  assert.equal(count(plan, 'line'), 4)
  assert.deepEqual(kinds(plan).sort(), ['Horizontal', 'Horizontal', 'Vertical', 'Vertical'])
  // Each line runs between two of the rectangle's own corners.
  const corners = new Set(plan.filter((o) => o.op === 'point').map((o) => o.key))
  for (const l of plan.filter((o) => o.op === 'line')) assert.ok(corners.has(l.a) && corners.has(l.b))
})

test('a rectangle with no width draws nothing', async () => {
  const { planShape } = await lib()
  assert.deepEqual(planShape('rectangle', [{ at: [0, 0] }, { at: [0, 5] }]), [])
})

test('a centre rectangle is symmetric about its first click', async () => {
  const { planShape } = await lib()
  const pts = planShape('center-rectangle', [{ at: [5, 5] }, { at: [8, 7] }]).filter((o) => o.op === 'point').map((o) => o.at)
  const xs = pts.map((p) => p[0]).sort((a, b) => a - b)
  const ys = pts.map((p) => p[1]).sort((a, b) => a - b)
  assert.deepEqual([xs[0], xs[3], ys[0], ys[3]], [2, 8, 3, 7])
})

test('a nearly level line is drawn level and constrained Horizontal', async () => {
  const { planShape } = await lib()
  const plan = planShape('line', [{ at: [0, 0] }, { at: [10, 0.1] }])
  const b = plan.filter((o) => o.op === 'point')[1]
  assert.equal(b.at[1], 0)
  assert.deepEqual(kinds(plan), ['Horizontal'])
})

test('a line from an existing point reuses it -- the chain stays joined', async () => {
  const { planShape } = await lib()
  const plan = planShape('line', [{ at: [0, 0], pointId: 'p-old' }, { at: [3, 4] }])
  assert.equal(count(plan, 'point'), 1)
  const line = plan.find((o) => o.op === 'line')
  assert.equal(line.a, 'id:p-old')
})

test('a centre line is construction geometry', async () => {
  const { planShape } = await lib()
  const line = planShape('centerline', [{ at: [0, 0] }, { at: [3, 4] }]).find((o) => o.op === 'line')
  assert.equal(line.construction, true)
})

test('a polygon has equal sides on its circle', async () => {
  const { planShape } = await lib()
  const plan = planShape('polygon', [{ at: [0, 0] }, { at: [10, 0] }], { sides: 6 })
  assert.equal(count(plan, 'point'), 6)
  assert.equal(count(plan, 'line'), 6)
  assert.equal(kinds(plan).filter((k) => k === 'Equal').length, 5)
  for (const p of plan.filter((o) => o.op === 'point')) assert.ok(near(Math.hypot(p.at[0], p.at[1]), 10))
})

test('an arc through three points has the circle they lie on, and runs start -> end through the middle', async () => {
  const { arcThrough } = await lib()
  const a = arcThrough([1, 0], [0, 1], [-1, 0])
  assert.ok(near(a.c[0], 0) && near(a.c[1], 0) && near(a.r, 1))
  assert.ok(near(a.start, 0) && near(a.end, Math.PI))
  // The same ends, but passing below: the sweep is the other way round.
  const b = arcThrough([1, 0], [0, -1], [-1, 0])
  assert.ok(near(b.start, Math.PI) && near(b.end, 2 * Math.PI))
  assert.equal(arcThrough([0, 0], [1, 1], [2, 2]), null, 'collinear points have no arc')
})

test('a slot is two lines and two half-circle ends', async () => {
  const { planShape } = await lib()
  const plan = planShape('slot', [{ at: [0, 0] }, { at: [10, 0] }, { at: [5, 2] }])
  assert.equal(count(plan, 'line'), 2)
  const arcs = plan.filter((o) => o.op === 'arc')
  assert.equal(arcs.length, 2)
  for (const a of arcs) {
    assert.ok(near(a.r, 2))
    assert.ok(near(((a.end - a.start) + 2 * Math.PI) % (2 * Math.PI), Math.PI))
  }
})

test('a spline is built on its own control points', async () => {
  const { planShape } = await lib()
  const plan = planShape('spline', [{ at: [0, 0] }, { at: [5, 5] }, { at: [10, 0] }, { at: [15, 5] }])
  assert.equal(count(plan, 'point'), 4)
  const s = plan.find((o) => o.op === 'spline')
  assert.equal(s.points.length, 4)
  assert.equal(s.degree, 3)
})

test('snapping prefers an end point over the grid, within reach', async () => {
  const { snap } = await lib()
  const targets = [{ at: [10, 10], kind: 'endpoint', pointId: 'p1' }]
  const s = snap([10.3, 9.8], targets, { reach: 0.5, grid: 1 })
  assert.equal(s.kind, 'endpoint')
  assert.equal(s.pointId, 'p1')
  assert.deepEqual(snap([3.4, 7.6], targets, { reach: 0.5, grid: 1 }).at, [3, 8])
  assert.deepEqual(snap([3.4, 7.6], targets, { reach: 0.5, grid: 0 }).at, [3.4, 7.6])
})

test('every entity has a drawn path; a click picks the nearest within reach', async () => {
  const { entityPaths, pickEntity } = await lib()
  const summary = {
    id: 's', constraint_count: 0, splines: [], ellipses: [], polylines: [], rectangles: [],
    points: [{ id: 'a', x: 0, y: 0, is_fixed: false, is_construction: false }],
    lines: [{ id: 'l', is_construction: false, geometry: { Segment: { start: { x: 0, y: 0 }, end: { x: 10, y: 0 } } } }],
    circles: [{ id: 'c', cx: 20, cy: 0, radius: 2, is_construction: true }],
    arcs: [{ id: 'r', cx: 0, cy: 20, radius: 3, start_angle: 0, end_angle: Math.PI, ccw: true, is_construction: false }],
  }
  const paths = entityPaths(summary)
  assert.deepEqual(paths.map((p) => Object.keys(p.ref)[0]).sort(), ['Arc', 'Circle', 'Line'])
  const circle = paths.find((p) => 'Circle' in p.ref)
  assert.equal(circle.construction, true)
  assert.ok(near(Math.hypot(circle.pts[0][0] - 20, circle.pts[0][1]), 2, 1e-6))
  assert.deepEqual(pickEntity([5, 0.2], paths, summary, 0.5), { Line: 'l' })
  assert.deepEqual(pickEntity([0.1, 0.1], paths, summary, 0.5), { Point: 'a' }, 'a point wins over the line it ends')
  assert.equal(pickEntity([5, 4], paths, summary, 0.5), null)
})

test('a plan draws as paths -- what the rubber band and the not-yet-confirmed shape show', async () => {
  const { planShape, planPaths } = await lib()
  const rect = planPaths(planShape('rectangle', [{ at: [0, 0] }, { at: [4, 2] }]), new Map())
  assert.equal(rect.length, 4, 'four sides')
  // A line from an existing point draws from where that point is.
  const known = new Map([['p-old', [1, 1]]])
  const line = planPaths(planShape('line', [{ at: [1, 1], pointId: 'p-old' }, { at: [5, 1] }]), known)
  assert.deepEqual(line[0], [[1, 1], [5, 1]])
  const circle = planPaths(planShape('circle', [{ at: [0, 0] }, { at: [3, 0] }]), new Map())
  assert.ok(circle[0].length > 16)
})

test('the freedom chip reads the solver verdict, not the raw count', async () => {
  const { freedomLabel } = await lib()
  assert.deepEqual(freedomLabel({ total_free_dofs: 11, status: { kind: 'under_constrained', dofs: 7 } }, false), { text: '7 free', tone: 'free' })
  assert.deepEqual(freedomLabel({ total_free_dofs: 4, status: { kind: 'fully_constrained' } }, false), { text: 'Fully defined', tone: 'done' })
  assert.deepEqual(freedomLabel({ total_free_dofs: 4, status: { kind: 'over_constrained', conflicting_constraints: 2 } }, false), { text: '2 conflicting', tone: 'conflict' })
  assert.equal(freedomLabel({ total_free_dofs: 0, status: { kind: 'fully_constrained' } }, true), null, 'an empty sketch says nothing')
})

// ── Slice 2: constraints, dimensions, crossings, modify ─────────────────
const seg = (id, x0, y0, x1, y1, construction = false) => ({ id, is_construction: construction, geometry: { Segment: { start: { x: x0, y: y0 }, end: { x: x1, y: y1 } } } })
const sk = (o) => ({ id: 's', constraint_count: 0, points: [], lines: [], circles: [], arcs: [], splines: [], ellipses: [], polylines: [], rectangles: [], ...o })

test('the constraints offered fit what is selected', async () => {
  const { constraintsFor } = await lib()
  const kinds = (sel) => constraintsFor(sel).map((c) => c.kind)
  assert.deepEqual(kinds([{ Line: 'a' }]), ['Horizontal', 'Vertical'])
  assert.deepEqual(kinds([{ Line: 'a' }, { Line: 'b' }]), ['Parallel', 'Perpendicular', 'Equal'])
  assert.deepEqual(kinds([{ Point: 'p' }, { Point: 'q' }]), ['Coincident'])
  assert.deepEqual(kinds([{ Point: 'p' }, { Line: 'a' }]), ['PointOnCurve', 'Midpoint'])
  assert.deepEqual(kinds([{ Line: 'a' }, { Circle: 'c' }]), ['Tangent'])
  assert.deepEqual(kinds([{ Circle: 'c' }, { Arc: 'r' }]), ['Concentric', 'Equal', 'Tangent'])
  assert.deepEqual(kinds([{ Point: 'p' }, { Point: 'q' }, { Line: 'a' }]), ['Symmetric'])
  assert.deepEqual(kinds([]), [])
  // The point always goes first where the solver expects it.
  assert.deepEqual(constraintsFor([{ Line: 'a' }, { Point: 'p' }])[0].entities, [{ Point: 'p' }, { Line: 'a' }])
})

test('smart dimension measures what is selected, at its current size', async () => {
  const { dimensionFor } = await lib()
  const s = sk({
    points: [
      { id: 'p', x: 0, y: 0, is_fixed: false, is_construction: false },
      { id: 'q', x: 3, y: 4, is_fixed: false, is_construction: false },
    ],
    lines: [{ ...seg('a', 0, 0, 3, 4), endpoints: undefined }, seg('b', 0, 0, 10, 0)],
    circles: [{ id: 'c', cx: 0, cy: 0, radius: 2.5, is_construction: false }],
    arcs: [{ id: 'r', cx: 0, cy: 0, radius: 4, start_angle: 0, end_angle: 1, ccw: true, is_construction: false }],
  })
  const d1 = dimensionFor([{ Line: 'b' }], s)
  assert.equal(d1.kind, 'Length')
  assert.equal(d1.value, 10)
  assert.deepEqual(dimensionFor([{ Circle: 'c' }], s), { kind: 'Diameter', entities: [{ Circle: 'c' }], value: 5, unit: 'mm' })
  assert.equal(dimensionFor([{ Arc: 'r' }], s).kind, 'Radius')
  assert.equal(dimensionFor([{ Point: 'p' }, { Point: 'q' }], s).value, 5)
  const ang = dimensionFor([{ Line: 'a' }, { Line: 'b' }], s)
  assert.equal(ang.kind, 'Angle')
  assert.equal(ang.unit, 'deg')
  assert.ok(Math.abs(ang.value - 53.130102) < 1e-4)
  assert.equal(dimensionFor([{ Circle: 'c' }, { Line: 'b' }, { Point: 'p' }], s), null)
})

test('crossings are found; touching at a shared end is not a crossing; overlaps are flagged', async () => {
  const { crossings, overlaps } = await lib()
  const s = sk({
    lines: [seg('a', 0, 0, 10, 0), seg('b', 5, -5, 5, 5), seg('c', 10, 0, 10, 8), seg('d', 2, 0, 6, 0)],
    circles: [{ id: 'k', cx: 20, cy: 0, radius: 3, is_construction: false }],
  })
  const xs = crossings(s).map((p) => p.at.map((v) => Math.round(v * 1000) / 1000))
  assert.ok(xs.some(([x, y]) => x === 5 && y === 0), JSON.stringify(xs))
  assert.ok(!xs.some(([x, y]) => x === 10 && y === 0), 'a corner is not a crossing')
  const ov = overlaps(s)
  assert.equal(ov.length, 1)
  assert.deepEqual(ov[0].map((r) => Object.values(r)[0]).sort(), ['a', 'd'])
})

test('extend runs a line end forward to the nearest curve it meets', async () => {
  const { extendBoundary } = await lib()
  const s = sk({ lines: [seg('a', 0, 0, 4, 0), seg('w1', 6, -5, 6, 5), seg('w2', 9, -5, 9, 5), seg('back', -3, -5, -3, 5)] })
  assert.deepEqual(extendBoundary('a', 'end', s), { Line: 'w1' })
  assert.deepEqual(extendBoundary('a', 'start', s), { Line: 'back' })
  assert.equal(extendBoundary('w1', 'end', s), null)
})

test('a corner is a point where exactly two lines meet', async () => {
  const { isCorner } = await lib()
  const s = sk({
    points: [{ id: 'p', x: 0, y: 0, is_fixed: false, is_construction: false }, { id: 'q', x: 9, y: 9, is_fixed: false, is_construction: false }],
    lines: [{ ...seg('a', 0, 0, 5, 0), endpoints: ['p', 'x'] }, { ...seg('b', 0, 0, 0, 5), endpoints: ['y', 'p'] }],
  })
  assert.equal(isCorner('p', s), true)
  assert.equal(isCorner('q', s), false)
})

test('arcs are built on their end points, so they join what shares them', async () => {
  const { planShape } = await lib()
  const arc = planShape('arc3', [{ at: [1, 0] }, { at: [-1, 0] }, { at: [0, 1] }])
  const a = arc.find((o) => o.op === 'arc')
  const pts = new Map(arc.filter((o) => o.op === 'point').map((o) => [o.key, o.at]))
  assert.ok(pts.has(a.a) && pts.has(a.b), 'both ends are points of the plan')
  // A slot's ends ARE its sides' end points: 4 points in all.
  const slot = planShape('slot', [{ at: [0, 0] }, { at: [10, 0] }, { at: [5, 2] }])
  assert.equal(slot.filter((o) => o.op === 'point').length, 4)
  const lineEnds = new Set(slot.filter((o) => o.op === 'line').flatMap((o) => [o.a, o.b]))
  for (const ar of slot.filter((o) => o.op === 'arc')) assert.ok(lineEnds.has(ar.a) && lineEnds.has(ar.b))
})

test('construction geometry is never an overlap or a crossing (it forms no profile)', async () => {
  const { crossings, overlaps } = await lib()
  const s = sk({ lines: [seg('a', 0, 0, 10, 0), seg('g', 2, 0, 6, 0, true), seg('v', 5, -5, 5, 5, true)] })
  assert.equal(overlaps(s).length, 0)
  assert.equal(crossings(s).length, 0)
})

test('a constraint that only touches construction geometry is not marked on the sketch', async () => {
  const { showsMark } = await lib()
  const s = sk({ lines: [seg('a', 0, 0, 10, 0), seg('g', 0, 0, 0, 10, true)], points: [{ id: 'p', x: 0, y: 0, is_fixed: false, is_construction: false }] })
  assert.equal(showsMark([{ Line: 'g' }], s), false)
  assert.equal(showsMark([{ Line: 'a' }, { Line: 'g' }], s), true)
  assert.equal(showsMark([{ Point: 'p' }], s), true)
})
