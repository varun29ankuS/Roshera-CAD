// Run: node --experimental-strip-types --test test/object-upsert.test.mjs
//
// An `ObjectCreated` frame for an id the scene already has is an in-place
// kernel change (a transform, a face-extrude): only the geometry and the pose
// are new. It must not fly the camera to the part, must not post a "Created"
// line, and must not reset the colour the part already has.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mergeObjectCreated } from '../src/lib/object-upsert.ts'

function obj(overrides) {
  return {
    id: 'a',
    name: 'Transformed',
    objectType: 'transform',
    mesh: { vertices: new Float32Array([1]), indices: new Uint32Array([0]), normals: new Float32Array([0]) },
    material: { color: '#b3b3bf', metalness: 0.1, roughness: 0.8, opacity: 1 },
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    scale: [1, 1, 1],
    visible: true,
    locked: false,
    ...overrides,
  }
}

const existing = obj({
  name: 'Bracket A',
  objectType: 'box',
  mesh: { vertices: new Float32Array([7]), indices: new Uint32Array([0]), normals: new Float32Array([0]) },
  material: { color: '#ff3300', metalness: 0.4, roughness: 0.2, opacity: 1 },
  position: [5, 0, 0],
  visible: false,
})

test('an in-place change is not announced as new: the camera is not re-framed', () => {
  const r = mergeObjectCreated(obj({}), existing, () => 'Created **Transformed**')
  assert.equal(r.announceAsNew, false)
})

test('an in-place change posts no "Created" line', () => {
  let asked = 0
  const r = mergeObjectCreated(obj({}), existing, () => {
    asked += 1
    return 'Created **Transformed**'
  })
  assert.equal(r.echoMessage, null)
  assert.equal(asked, 0)
})

test('an in-place change keeps the part colour; only geometry and pose update', () => {
  const incoming = obj({})
  const r = mergeObjectCreated(incoming, existing, () => null)
  assert.deepEqual(r.obj.material, existing.material)
  assert.equal(r.obj.name, 'Bracket A')
  assert.equal(r.obj.objectType, 'box')
  assert.equal(r.obj.visible, false)
  assert.equal(r.obj.mesh, incoming.mesh)
  assert.deepEqual(r.obj.position, [0, 0, 0])
})

test('a genuinely new part is framed, echoed, and takes the server material', () => {
  const incoming = obj({})
  const r = mergeObjectCreated(incoming, undefined, () => 'Created **Transformed**')
  assert.equal(r.announceAsNew, true)
  assert.equal(r.echoMessage, 'Created **Transformed**')
  assert.equal(r.obj, incoming)
})
