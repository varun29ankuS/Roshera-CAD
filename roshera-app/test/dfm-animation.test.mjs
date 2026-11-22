import { test } from 'node:test'
import assert from 'node:assert/strict'

const { sampleActor, sampleFaceTrack, animationTime, animationFor } = await import('../src/lib/dfm-animation.ts')
const { dfmRows, DFM_PROCESSES } = await import('../src/lib/dfm.ts')

// Shaped like the kernel's `cnc.internal_corner_radius` descriptor on the
// sharp-pocket fixture (tests/dfm_processes.rs): a Ø3 end mill descending
// −Z to the pocket floor at z = 4.
const corner = {
  rule: 'cnc.internal_corner_radius',
  kind: 'tool_approach',
  duration_s: 4,
  caption: 'Ø3.00 end mill leaves R1.50 — this inside corner is R0.00',
  measured: 0,
  limit: 1.5,
  faces: [12, 13, 14, 15],
  actors: [
    {
      id: 'end_mill',
      role: 'tool',
      shape: { type: 'end_mill', diameter: 3, flute_length: 6, length: 12 },
      keys: [
        { t: 0, position: [15, 10, 16], axis: [0, 0, -1], opacity: 0, extent: 0 },
        { t: 0.15, position: [15, 10, 16], axis: [0, 0, -1], opacity: 1, extent: 0 },
        { t: 0.6, position: [15, 10, 4], axis: [0, 0, -1], opacity: 1, extent: 0 },
        { t: 1, position: [15, 10, 4], axis: [0, 0, -1], opacity: 1, extent: 0 },
      ],
    },
  ],
  face_tracks: [
    {
      faces: [12, 13, 14, 15],
      keys: [
        { t: 0, offset: [0, 0, 0], rotation_deg: 0, state: 'context' },
        { t: 0.55, offset: [0, 0, 0], rotation_deg: 0, state: 'fail' },
        { t: 1, offset: [0, 0, 0], rotation_deg: 0, state: 'fail' },
      ],
    },
  ],
}

test('an actor hits its keyframes exactly — the player never invents a position', () => {
  const tool = corner.actors[0]
  assert.deepEqual(sampleActor(tool, 0.15).position, [15, 10, 16])
  assert.deepEqual(sampleActor(tool, 0.6).position, [15, 10, 4])
  assert.deepEqual(sampleActor(tool, 1).position, [15, 10, 4])
  assert.equal(sampleActor(tool, 0).opacity, 0)
})

test('between keys the tool moves monotonically along the kernel path', () => {
  const tool = corner.actors[0]
  const zs = [0.2, 0.3, 0.4, 0.5].map((t) => sampleActor(tool, t).position[2])
  for (let i = 1; i < zs.length; i++) assert.ok(zs[i] < zs[i - 1], `${zs}`)
  assert.ok(zs.every((z) => z < 16 && z > 4))
})

test('a face turns red when its key says so, not before', () => {
  const tr = corner.face_tracks[0]
  assert.equal(sampleFaceTrack(tr, 0.5).state, 'context')
  assert.equal(sampleFaceTrack(tr, 0.55).state, 'fail')
  assert.equal(sampleFaceTrack(tr, 0.9).state, 'fail')
})

test('a caliper opening interpolates the kernel extent to the measured wall', () => {
  const caliper = {
    id: 'caliper',
    role: 'tool',
    shape: { type: 'caliper', opening: 10.5 },
    keys: [
      { t: 0.15, position: [0, 0, 0], axis: [0, 0, 1], opacity: 1, extent: 10.5 },
      { t: 0.7, position: [0, 0, 0], axis: [0, 0, 1], opacity: 1, extent: 0.5 },
    ],
  }
  assert.equal(sampleActor(caliper, 1).extent, 0.5)
  assert.equal(sampleActor(caliper, 0.15).extent, 10.5)
})

test('playback loops after a hold on the last frame', () => {
  assert.equal(animationTime(corner, 0), 0)
  assert.equal(animationTime(corner, 2), 0.5)
  assert.equal(animationTime(corner, 4.5), 1, 'holding the last frame')
  assert.equal(animationTime(corner, 4 + 1.2), 0, 'looped')
})

test('the panel finds a finding’s animation by rule id, or none', () => {
  assert.equal(animationFor([corner], 'cnc.internal_corner_radius'), corner)
  assert.equal(animationFor([corner], 'cnc.hole_access'), null)
  assert.equal(animationFor(undefined, 'cnc.hole_access'), null)
})

test('the process packs are offered and their findings read in an engineer’s words', () => {
  const packs = DFM_PROCESSES.map((p) => p.pack)
  for (const p of ['cnc_3_axis', 'cnc_turning', 'sheet_metal', 'casting']) assert.ok(packs.includes(p), p)
  const report = {
    pack: 'cnc_3_axis',
    params: { pack: 'cnc_3_axis' },
    verdicts: [
      {
        rule: 'cnc.internal_corner_radius',
        verdict: { kind: 'violation', witnesses: [12, 13], measured: { value: 0 }, limit: { value: 1.5 } },
        provenance: { kind: 'shop_practice', note: 'tool radius' },
      },
      {
        rule: 'cnc.hole_depth_ratio',
        verdict: { kind: 'pass', margin: { value: 1e300 } },
        provenance: { kind: 'shop_practice', note: 'twist drill' },
      },
    ],
    summary: { kind: 'violations', count: 1 },
    animations: [corner],
  }
  const rows = dfmRows(report)
  assert.deepEqual([rows[0].title, rows[0].measured, rows[0].limit], ['Inside corner radius', '0 mm', '≥ 1.5 mm'])
  assert.equal(rows[1].measured, 'none on this part', 'the vacuous sentinel is never shown as a margin')
})
