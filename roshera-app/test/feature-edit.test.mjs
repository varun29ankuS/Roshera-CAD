import { test } from 'node:test'
import assert from 'node:assert/strict'

const { featureFields, mouldBody, parseSize } = await import('../src/lib/feature-edit.ts')

const P = (sequence, event_id, command, key, value, edited = false) => ({ sequence, event_id, command, key, value, edited })

// The flange's real history (2026-11-13): step 0 the flange cylinder, steps
// 8-19 four holes (cylinder + subtract each), 26 the R3 hub fillet.
const params = [
  P(0, 'e0', 'create_cylinder_3d', 'radius', 40),
  P(0, 'e0', 'create_cylinder_3d', 'height', 12),
  P(0, 'e0', 'create_cylinder_3d', 'base_z', 0),
  P(8, 'e8', 'create_cylinder_3d', 'radius', 4),
  P(8, 'e8', 'create_cylinder_3d', 'height', 20),
  P(8, 'e8', 'create_cylinder_3d', 'base_x', 30),
  P(11, 'e11', 'create_cylinder_3d', 'radius', 4),
  P(11, 'e11', 'create_cylinder_3d', 'height', 20),
  P(11, 'e11', 'create_cylinder_3d', 'base_x', 0),
  P(26, 'e26', 'fillet_edges', 'radius', 3, true),
]

test('a feature shows the sizes of the steps it is made of, with labels and units', () => {
  const fields = featureFields({ events: [0, 1], instances: [] }, params)
  assert.deepEqual(
    fields.map((f) => [f.label, f.value, f.unit]),
    [
      ['Radius', 40, 'mm'],
      ['Height', 12, 'mm'],
      ['Position Z', 0, 'mm'],
    ],
  )
})

test('an edited size says so', () => {
  const [r] = featureFields({ events: [26], instances: [] }, params)
  assert.deepEqual([r.label, r.value, r.edited], ['Radius', 3, true])
})

test('a pattern shows the sizes its instances share, edited together', () => {
  const pattern = { events: [8, 9, 10, 11, 12, 13], instances: [{ events: [8, 9, 10] }, { events: [11, 12, 13] }] }
  const fields = featureFields(pattern, params)
  assert.deepEqual(fields.map((f) => [f.label, f.value]), [
    ['Radius', 4],
    ['Height', 20],
  ], 'positions differ per hole, so they are not one field')
  assert.deepEqual(fields[0].targets, [
    { event_id: 'e8', key: 'radius' },
    { event_id: 'e11', key: 'radius' },
  ])
})

test('the edit request targets the first step and links the rest', () => {
  const pattern = { events: [8, 11], instances: [{ events: [8] }, { events: [11] }] }
  const [radius] = featureFields(pattern, params)
  assert.deepEqual(mouldBody(radius, 4.5), {
    target_event_id: 'e8',
    parameter: 'radius',
    value: 4.5,
    linked: [{ target_event_id: 'e11', parameter: 'radius' }],
  })
})

test('a typed size is a finite number', () => {
  assert.equal(parseSize('4.5'), 4.5)
  assert.equal(parseSize(' 12 mm'), 12)
  assert.equal(parseSize('-2'), -2)
  for (const t of ['', 'x', '1e999', '3..2']) assert.equal(parseSize(t), null, t)
})
