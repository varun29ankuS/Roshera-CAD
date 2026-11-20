// A part that has just been built is shown properly: fitted AND turned to
// the isometric view, Z up (Varun 2026-11-20: "once the part is build, it
// should be zoomed in and rotated to be looked at properly"). Before, a new
// part was fitted from whatever direction the camera already had -- after a
// sketch on the top plane, a fresh extrusion was framed looking straight
// down at it. The Fit button still keeps the user's direction.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const { frameView, ISO_DIRECTION } = await import('../src/lib/camera-fit.ts')

const len = (v) => Math.hypot(v[0], v[1], v[2])
const near = (a, b) => a.every((x, i) => Math.abs(x - b[i]) < 1e-9)

test('a new part is turned to the iso direction, Z up, whatever the camera was doing', () => {
  const topDown = [0, 0, 1]
  const v = frameView('new', topDown, [0, 1, 0], false)
  assert.ok(near(v.direction, ISO_DIRECTION), `direction ${v.direction}`)
  assert.ok(near(v.up, [0, 0, 1]), `up ${v.up}`)
  assert.ok(Math.abs(len(v.direction) - 1) < 1e-9, 'unit direction')
  assert.ok(ISO_DIRECTION[2] > 0, 'looks down onto the part, not from below')
})

test('Fit keeps the direction and up the user chose', () => {
  const front = [0, -1, 0]
  const v = frameView('fit', front, [0, 0, 1], false)
  assert.ok(near(v.direction, front))
  assert.ok(near(v.up, [0, 0, 1]))
})

test('a new part while sketching keeps the sketch-normal view', () => {
  const v = frameView('new', [0, 0, 1], [0, 1, 0], true)
  assert.ok(near(v.direction, [0, 0, 1]))
  assert.ok(near(v.up, [0, 1, 0]))
})

test('opening a document is the iso view too', () => {
  const v = frameView('all', [0.3, 0.2, -0.9], [1, 0, 0], false)
  assert.ok(near(v.direction, ISO_DIRECTION))
  assert.ok(near(v.up, [0, 0, 1]))
})

test('a degenerate camera direction falls back to iso', () => {
  const v = frameView('fit', [0, 0, 0], [0, 0, 1], false)
  assert.ok(near(v.direction, ISO_DIRECTION))
})

test('the scene bridge frames a newly announced part as a new part, not a plain fit', () => {
  const src = readFileSync(new URL('../src/lib/ws-bridge.ts', import.meta.url), 'utf8')
  const announce = src.slice(src.indexOf('if (!op.announceAsNew) continue'))
  assert.match(announce.slice(0, 200), /setPendingFrameObject\(FRAME_NEW\)/)
  const cam = readFileSync(new URL('../src/components/viewport/CameraController.tsx', import.meta.url), 'utf8')
  assert.match(cam, /frameView\(/, 'the camera takes its framing direction from frameView')
})
