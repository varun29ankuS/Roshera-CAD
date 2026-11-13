// Run: node --experimental-strip-types --test test/history-view.test.mjs
//
// Sliding through the timeline: the viewport shows the part as it stood
// after a chapter, read-only, from GET /api/timeline/scrub/{branch}/{seq}
// (frame cache). These pin the conversion of that answer into what the
// viewport draws, which frame a drag is over, and arrow-key stepping.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { scrubObjects, frameIndexAt, stepTo } from '../src/lib/history-view.ts'

test('a scrub answer becomes read-only scene objects, one per solid', () => {
  const objs = scrubObjects({
    at_sequence: 7,
    objects: [
      {
        id: 'scrub:2',
        name: 'solid 2 @ event 7',
        mesh: { vertices: [0, 0, 0, 1, 0, 0, 0, 1, 0], indices: [0, 1, 2], normals: [0, 0, 1, 0, 0, 1, 0, 0, 1], face_ids: [5] },
      },
    ],
  })
  assert.equal(objs.length, 1)
  const o = objs[0]
  assert.equal(o.id, 'scrub:2')
  assert.equal(o.locked, true, 'a past state cannot be edited in place')
  assert.equal(o.visible, true)
  assert.ok(o.mesh.vertices instanceof Float32Array)
  assert.deepEqual(Array.from(o.mesh.indices), [0, 1, 2])
  assert.deepEqual(Array.from(o.mesh.faceIds ?? []), [5])
  assert.deepEqual(o.position, [0, 0, 0])
})

test('a scrub answer with no solids draws nothing, honestly', () => {
  assert.deepEqual(scrubObjects({ at_sequence: 0, objects: [] }), [])
  assert.deepEqual(scrubObjects({}), [])
})

test('dragging along the strip lands on the frame under the pointer', () => {
  // 4 frames, each 80 px wide starting at x = 10.
  assert.equal(frameIndexAt(10, 10, 80, 4), 0)
  assert.equal(frameIndexAt(95, 10, 80, 4), 1)
  assert.equal(frameIndexAt(250, 10, 80, 4), 3)
  assert.equal(frameIndexAt(-50, 10, 80, 4), 0, 'left of the strip: the first frame')
  assert.equal(frameIndexAt(9999, 10, 80, 4), 3, 'right of the strip: the last frame')
})

test('arrow keys step one chapter; past the last frame is the live part', () => {
  const anchors = [3, 7, 19]
  assert.equal(stepTo(anchors, null, -1), 7, 'from live, back one: the frame before the last')
  assert.equal(stepTo(anchors, 7, -1), 3)
  assert.equal(stepTo(anchors, 3, -1), 3, 'stops at the start')
  assert.equal(stepTo(anchors, 7, +1), null, 'forward onto the last frame is the live part')
  assert.equal(stepTo(anchors, null, +1), null, 'already live')
})

test('a binary step (RSCB v1) is read in place into scene objects', async () => {
  const { decodeScrubBinary } = await import('../src/lib/history-view.ts')
  const header = new TextEncoder().encode(
    JSON.stringify({ at_sequence: 7, objects: [{ id: 'scrub:2', name: 'solid 2', nv: 9, ni: 3, nn: 9, nf: 1 }] }),
  )
  const pad = (4 - ((12 + header.length) % 4)) % 4
  const buf = new ArrayBuffer(12 + header.length + pad + 4 * (9 + 3 + 9 + 1))
  const u8 = new Uint8Array(buf)
  u8.set(new TextEncoder().encode('RSCB'), 0)
  const dv = new DataView(buf)
  dv.setUint32(4, 1, true)
  dv.setUint32(8, header.length, true)
  u8.set(header, 12)
  let at = 12 + header.length + pad
  for (const x of [0, 0, 0, 1, 0, 0, 0, 1, 0]) { dv.setFloat32(at, x, true); at += 4 }
  for (const x of [0, 1, 2]) { dv.setUint32(at, x, true); at += 4 }
  for (const x of [0, 0, 1, 0, 0, 1, 0, 0, 1]) { dv.setFloat32(at, x, true); at += 4 }
  dv.setUint32(at, 5, true)
  const objs = decodeScrubBinary(buf)
  assert.equal(objs.length, 1)
  assert.equal(objs[0].id, 'scrub:2')
  assert.equal(objs[0].locked, true)
  assert.deepEqual(Array.from(objs[0].mesh.vertices), [0, 0, 0, 1, 0, 0, 0, 1, 0])
  assert.deepEqual(Array.from(objs[0].mesh.indices), [0, 1, 2])
  assert.deepEqual(Array.from(objs[0].mesh.faceIds ?? []), [5])
})

test('a buffer that is not RSCB v1 is refused, not misread', async () => {
  const { decodeScrubBinary } = await import('../src/lib/history-view.ts')
  assert.throws(() => decodeScrubBinary(new TextEncoder().encode('{"objects":[]}').buffer), /RSCB/)
})
