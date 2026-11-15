import { test } from 'node:test'
import assert from 'node:assert/strict'

const { lensFaceTints, lensSummary, lensStoryRows } = await import('../src/lib/lens.ts')

// The shape `GET /api/lens/{solid}` returns (Kernel Lens slice 1, Task 4).
const part = {
  overview: [
    { kind: 'loop_reversed', count: 3 },
    { kind: 'edge_tolerance_raised', count: 2 },
    { kind: 'blocking', count: 1 },
  ],
  faces: [
    { face_id: 4, pid: 'a1', status: 'as_written' },
    { face_id: 7, pid: 'b2', status: 'repaired' },
    { face_id: 9, pid: 'c3', status: 'blocking' },
    { face_id: 11, pid: 'd4', status: 'repaired' },
  ],
  solid_events: [],
}

test('repaired faces tint amber, blocking faces red, the rest are left as written', () => {
  assert.deepEqual(lensFaceTints(part), { repaired: [7, 11], blocking: [9] })
})

test('a part the kernel repaired but which certifies reads repaired, with the count', () => {
  const sound = { ...part, faces: part.faces.filter((f) => f.status !== 'blocking'), overview: part.overview.slice(0, 2) }
  assert.deepEqual(lensSummary(sound), { state: 'repaired', text: '2 faces repaired · sound' })
})

test('a blocking face makes the part blocked, whatever else was repaired', () => {
  assert.deepEqual(lensSummary(part), { state: 'blocking', text: '1 face blocks the certificate' })
})

test('a part with an empty journal is as written and sound, not an empty card', () => {
  const clean = { overview: [], faces: [{ face_id: 1, pid: 'x', status: 'as_written' }], solid_events: [] }
  assert.deepEqual(lensSummary(clean), { state: 'as_written', text: 'Imported as written · sound' })
})

test('a solid-level blocker (no face to point at) still blocks the part', () => {
  const euler = {
    overview: [{ kind: 'blocking', count: 1 }],
    faces: [{ face_id: 1, pid: 'x', status: 'as_written' }],
    solid_events: [
      { entity: { solid: 'p' }, stage: 'certify', kind: { blocking: { conjunct: 'euler' } }, step_instance: null, why: 'V-E+F-R is odd', measured: [['euler', -61]] },
    ],
  }
  assert.equal(lensSummary(euler).state, 'blocking')
})

test("a face's story: repairs amber, blockers red, each with its why and numbers", () => {
  const events = [
    { entity: { face: 'b2' }, stage: 'import', kind: 'loop_reversed', step_instance: 9942, why: 'FACE_BOUND .F. — walked reversed (ISO 10303-42)', measured: [] },
    { entity: { face: 'b2' }, stage: 'repair', kind: 'edge_tolerance_raised', step_instance: 4417, why: 'edge gap over the declared accuracy', measured: [['gap_mm', 0.0032]] },
    { entity: { face: 'b2' }, stage: 'certify', kind: { blocking: { conjunct: 'watertight' } }, step_instance: null, why: 'open mesh edges on this face', measured: [['open_edges', 8]] },
  ]
  assert.deepEqual(lensStoryRows(events), [
    { mark: 'cross', title: 'Blocks: watertight', why: 'open mesh edges on this face', numbers: 'open_edges 8', source: null },
    { mark: 'amber', title: 'Loop reversed', why: 'FACE_BOUND .F. — walked reversed (ISO 10303-42)', numbers: '', source: '#9942' },
    { mark: 'amber', title: 'Edge tolerance raised', why: 'edge gap over the declared accuracy', numbers: 'gap_mm 0.0032', source: '#4417' },
  ])
})

test('a part the certificate fails reads unsound even when no face is named', () => {
  const s = lensSummary({ sound: false, overview: [], faces: [{ face_id: 1, pid: null, status: 'as_written' }], solid_events: [] })
  assert.equal(s.state, 'blocking')
})
