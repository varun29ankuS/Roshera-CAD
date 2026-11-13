import { test } from 'node:test'
import assert from 'node:assert/strict'

const { cubePreset } = await import('../src/lib/view-cube.ts')

const v = (x, y, z) => ({ x, y, z })
const ZERO = v(0, 0, 0)

test('a face click goes to that side (Z-up: +Z top, -Y front)', () => {
  assert.equal(cubePreset(ZERO, v(0, 0, 1)), 'top')
  assert.equal(cubePreset(ZERO, v(0, -1, 0)), 'front')
  assert.equal(cubePreset(ZERO, v(1, 0, 0)), 'right')
  assert.equal(cubePreset(ZERO, v(0, 1, 0)), 'back')
})

test('a corner click goes to the iso view, whatever side of the corner box was hit', () => {
  assert.equal(cubePreset(v(0.38, -0.38, 0.38), v(1, 0, 0)), 'isometric')
  assert.equal(cubePreset(v(0.38, -0.38, 0.38), v(0, 0, 1)), 'isometric')
})

test('an edge click goes to one of its two faces, by where the edge is, not the side hit', () => {
  // The top-front edge: top, whichever side of the edge box was hit.
  assert.equal(cubePreset(v(0, -0.38, 0.38), v(0, -1, 0)), 'top')
  // A vertical edge (front-right): right.
  assert.equal(cubePreset(v(0.38, -0.38, 0), v(0, -1, 0)), 'right')
})
