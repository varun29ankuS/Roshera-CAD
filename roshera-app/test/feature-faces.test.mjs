import { test } from 'node:test'
import assert from 'node:assert/strict'

const { locateFeatures } = await import('../src/lib/feature-faces.ts')

// The flange as the kernel reports it (2026-11-14, solid 12):
// GET /api/agent/parts/12/features and /api/timeline/parameters + chapters.
const cyl = (face_id, radius, origin) => ({ face_id, surface_kind: 'cylinder', radius, axis: [0, 0, 1], origin, normal: null })
const plane = (face_id, z, nz) => ({ face_id, surface_kind: 'plane', radius: null, axis: null, origin: [0, 0, z], normal: [0, 0, nz] })
const faces = [
  plane(56, 0, -1), plane(57, 12, 1), cyl(58, 4, [0, -30, -4]), cyl(59, 4, [-30, 0, -4]),
  cyl(60, 4, [0, 30, -4]), cyl(61, 4, [30, 0, -4]), plane(62, 32, 1), cyl(63, 15, [0, 0, -4]),
  cyl(64, 40, [0, 0, 0]), cyl(65, 22.5, [0, 0, 12]),
]
const P = (sequence, key, value) => ({ sequence, event_id: `e${sequence}`, command: 'create_cylinder_3d', key, value, edited: false })
const cylinderParams = (seq, r, h, x, y, z) => [P(seq, 'radius', r), P(seq, 'height', h), P(seq, 'base_x', x), P(seq, 'base_y', y), P(seq, 'base_z', z)]
const params = [
  ...cylinderParams(0, 40, 12, 0, 0, 0),
  ...cylinderParams(2, 22.5, 20, 0, 0, 12),
  ...cylinderParams(5, 15, 40, 0, 0, -4),
  ...cylinderParams(8, 4, 20, 30, 0, -4),
  ...cylinderParams(11, 4, 20, 0, 30, -4),
  ...cylinderParams(14, 4, 20, -30, 0, -4),
  ...cylinderParams(17, 4, 20, 0, -30, -4),
]
const chapters = [
  { anchor: 0, events: [0, 1], instances: [] },
  { anchor: 4, events: [2, 3, 4], instances: [] },
  { anchor: 7, events: [5, 6, 7], instances: [] },
  { anchor: 19, events: [8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19], instances: [{ events: [8, 9, 10] }, { events: [11, 12, 13] }, { events: [14, 15, 16] }, { events: [17, 18, 19] }] },
]
const sorted = (a) => [...a].sort((x, y) => x - y)

test('each feature owns the faces lying on the cylinder it recorded', () => {
  const at = locateFeatures(chapters, params, faces)
  assert.deepEqual(sorted(at.get(7)), [63]) // the bore: its wall only (a through cut has no ends)
  assert.deepEqual(sorted(at.get(19)), [58, 59, 60, 61]) // all four holes
})

test('an end face shared by two features goes to the earlier one, which made it', () => {
  const at = locateFeatures(chapters, params, faces)
  assert.deepEqual(sorted(at.get(0)), [56, 57, 64]) // flange: bottom, top, rim
  assert.deepEqual(sorted(at.get(4)), [62, 65]) // hub: its top and wall -- not the flange's top it sits on
})

test('a feature with no recorded geometry to match is not placed (no guess)', () => {
  const at = locateFeatures([{ anchor: 30, events: [30], instances: [] }], [P(30, 'radius', 3)].map((p) => ({ ...p, command: 'fillet_edges' })), faces)
  assert.equal(at.has(30), false)
})

test('a cylinder of the right radius on another axis is not the feature', () => {
  const offAxis = [cyl(99, 15, [5, 0, -4])]
  const at = locateFeatures([chapters[2]], params, offAxis)
  assert.equal(at.has(7), false)
})
