import { test } from 'node:test'
import assert from 'node:assert/strict'

const { parseExtrudeDistance, faceExtrudeBody } = await import('../src/lib/face-extrude.ts')

test('a typed distance reads as millimetres, signed by the direction', () => {
  assert.equal(parseExtrudeDistance('5', 'out'), 5)
  assert.equal(parseExtrudeDistance(' 2.5 mm ', 'out'), 2.5)
  assert.equal(parseExtrudeDistance('4', 'in'), -4)
  assert.equal(parseExtrudeDistance('-3', 'out'), -3, 'an explicit minus is kept as typed')
})

test('nothing, zero or text is not a distance', () => {
  for (const t of ['', '0', '0.0000001', 'abc', '5x', 'NaN', 'Infinity']) {
    assert.equal(parseExtrudeDistance(t, 'out'), null, JSON.stringify(t))
  }
})

test('the request names the part, the face, the signed distance and the direction', () => {
  assert.deepEqual(faceExtrudeBody('509242fb-75bd-4b08-8bcc-9e1ab2375791', 62, -4, [0, 0, 1]), {
    object_uuid: '509242fb-75bd-4b08-8bcc-9e1ab2375791',
    face_id: 62,
    distance: -4,
    direction: [0, 0, 1],
  })
})

test('a request the kernel would refuse is not built', () => {
  assert.throws(() => faceExtrudeBody('', 62, 4, [0, 0, 1]), /part/)
  assert.throws(() => faceExtrudeBody('x', 6.5, 4, [0, 0, 1]), /face/)
  assert.throws(() => faceExtrudeBody('x', 62, 0, [0, 0, 1]), /distance/)
  assert.throws(() => faceExtrudeBody('x', 62, 4, [0, 0, 0]), /direction/)
  assert.throws(() => faceExtrudeBody('x', 62, 4, [NaN, 0, 1]), /direction/)
})
