import { test } from 'node:test'
import assert from 'node:assert/strict'

const { keepCameraPose, keptCameraPose } = await import('../src/lib/camera-memory.ts')

test('the view left behind is the view found on return', () => {
  keepCameraPose({ position: [120, -90, 80], target: [0, 0, 16], up: [0, 0, 1], zoom: 2.5 })
  assert.deepEqual(keptCameraPose(), { position: [120, -90, 80], target: [0, 0, 16], up: [0, 0, 1], zoom: 2.5 })
  assert.deepEqual(keptCameraPose(), keptCameraPose(), 'reading does not use it up')
})

test('a kept view cannot be changed through what was handed back', () => {
  keepCameraPose({ position: [1, 2, 3], target: [0, 0, 0], up: [0, 0, 1], zoom: 1 })
  const p = keptCameraPose()
  p.position[0] = 999
  assert.equal(keptCameraPose().position[0], 1)
})

test('a broken pose is not kept; the last good one stays', () => {
  keepCameraPose({ position: [5, 5, 5], target: [0, 0, 0], up: [0, 0, 1], zoom: 1 })
  keepCameraPose({ position: [NaN, 0, 0], target: [0, 0, 0], up: [0, 0, 1], zoom: 1 })
  keepCameraPose({ position: [1, 1, 1], target: [1, 1, 1], up: [0, 0, 1], zoom: 1 })
  keepCameraPose({ position: [1, 1, 1], target: [0, 0, 0], up: [0, 0, 1], zoom: 0 })
  assert.deepEqual(keptCameraPose().position, [5, 5, 5])
})
