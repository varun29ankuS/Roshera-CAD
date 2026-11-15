// Run: node --experimental-strip-types --test test/sketch-owners.test.mjs
//
// A feature made from a sketch holds that sketch in the tree
// (Extrude1 > Sketch1), as every CAD tree shows it (Varun 2026-11-15: "a part
// that is extruded from sketch should show that ... thats not the case with
// our tree"). The tree listed every sketch at the top level instead.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { sketchOwners } from '../src/lib/chapters-api.ts'

const ch = (over) => ({
  kind: 'extrude',
  name: null,
  events: [1],
  anchor: 1,
  sound: true,
  refusals: [],
  author: 'user',
  author_detail: { kind: 'user' },
  intent: null,
  instances: [],
  part: 7,
  sketch: null,
  ...over,
})

test('a feature that names its sketch holds it; other sketches stay loose', () => {
  const { owner, loose } = sketchOwners(
    [ch({ anchor: 3, sketch: 's1' }), ch({ kind: 'box', anchor: 1, part: 2 })],
    ['s1', 's2'],
    [],
  )
  assert.equal(owner.get(3), 's1')
  assert.equal(owner.get(1), undefined, 'a box holds no sketch')
  assert.deepEqual(loose, ['s2'])
})

test('older history links through the body the sketch made', () => {
  // Recorded before features named their sketch: the extruded body carries
  // the sketch id, and its solid is the part the extrude ended up in.
  const { owner, loose } = sketchOwners(
    [ch({ kind: 'cylinder', anchor: 0, part: 7 }), ch({ anchor: 5, part: 7 }), ch({ anchor: 9, part: 7 })],
    ['old'],
    [{ sketchId: 'old', solidId: 7 }],
  )
  assert.equal(owner.get(5), 'old', 'the first sketch feature of that part takes it')
  assert.equal(owner.get(0), undefined, 'a cylinder is not a sketch feature')
  assert.equal(owner.get(9), undefined, 'a sketch is held once')
  assert.deepEqual(loose, [])
})

test('a sketch named by a feature is not given to another by the fallback', () => {
  const { owner } = sketchOwners(
    [ch({ anchor: 2, part: 7, sketch: 's1' }), ch({ anchor: 4, part: 7 })],
    ['s1'],
    [{ sketchId: 's1', solidId: 7 }],
  )
  assert.equal(owner.get(2), 's1')
  assert.equal(owner.get(4), undefined)
})

test('a sketch the document no longer has is never shown', () => {
  const { owner, loose } = sketchOwners([ch({ anchor: 2, sketch: 'gone' })], [], [])
  assert.equal(owner.size, 0)
  assert.deepEqual(loose, [])
})

test('a pattern instance made from a sketch holds it too', () => {
  const inst = ch({ anchor: 6, sketch: 's3' })
  const { owner } = sketchOwners([ch({ kind: 'pattern', anchor: 8, instances: [inst] })], ['s3'], [])
  assert.equal(owner.get(6), 's3')
})
