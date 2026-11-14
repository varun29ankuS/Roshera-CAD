import { test } from 'node:test'
import assert from 'node:assert/strict'

const { poseOf } = await import('../src/lib/assembly-pose.ts')

const close = (a, b) => a.every((x, i) => Math.abs(x - b[i]) < 1e-9)

test('a row-major instance transform reads as position, rotation and scale', () => {
  // 90° about Z, then moved to (10, 20, 30): translation is the LAST column of a row-major matrix.
  const m = [
    [0, -1, 0, 10],
    [1, 0, 0, 20],
    [0, 0, 1, 30],
    [0, 0, 0, 1],
  ]
  const p = poseOf(m)
  assert.ok(close(p.position, [10, 20, 30]), JSON.stringify(p.position))
  assert.ok(close(p.rotation, [0, 0, Math.PI / 2]), JSON.stringify(p.rotation))
  assert.ok(close(p.scale, [1, 1, 1]), JSON.stringify(p.scale))
})
