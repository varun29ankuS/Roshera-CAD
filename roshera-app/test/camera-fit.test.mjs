import { test } from 'node:test'
import assert from 'node:assert/strict'

const { fitDistance, FIT_MARGIN } = await import('../src/lib/camera-fit.ts')

// What fraction of the view's height / width a sphere of radius r fills at distance d.
const filled = (r, d, vfovDeg, aspect) => {
  const v = Math.tan((vfovDeg * Math.PI) / 360)
  return { h: r / (d * v), w: r / (d * v * aspect) }
}

test('fit leaves a margin: the part fills about half the view, not all of it', () => {
  const d = fitDistance(40, 50, 1.6)
  const f = filled(40, d, 50, 1.6)
  assert.ok(f.h <= 0.5 + 1e-9 && f.h > 0.4, `fills ${f.h} of the height`)
  assert.ok(FIT_MARGIN >= 2)
})

test('a narrow view is fitted by its width, so the part is never cut at the sides', () => {
  const d = fitDistance(40, 50, 0.6)
  const f = filled(40, d, 50, 0.6)
  assert.ok(f.w <= 0.5 + 1e-9, `fills ${f.w} of the width`)
})

const { openingPose, advanceFly, FLY_SECONDS, ISO_DIRECTION } = await import('../src/lib/camera-fit.ts')

test('a camera that opens on a loaded model stands outside it, fitted, in the iso view', () => {
  // A 100 mm part centred 20 mm up; the old start pose was 30 mm from the origin -- inside it.
  const pose = openingPose([0, 0, 20], 60, 50, 1.6, 800)
  const d = Math.hypot(pose.position[0] - 0, pose.position[1] - 0, pose.position[2] - 20)
  assert.ok(Math.abs(d - fitDistance(60, 50, 1.6)) < 1e-9, `stands ${d} away`)
  assert.ok(d > 60 * 2, 'well outside the part')
  assert.deepEqual(pose.target, [0, 0, 20])
  assert.deepEqual(pose.up, [0, 0, 1])
  for (let i = 0; i < 3; i++) {
    assert.ok(Math.abs((pose.position[i] - pose.target[i]) / d - ISO_DIRECTION[i]) < 1e-9)
  }
  // The orthographic camera shows the same footprint: zoom = px / (2 d tan(fov/2)).
  const zoom = 800 / (2 * d * Math.tan((50 * Math.PI) / 360))
  assert.ok(Math.abs(pose.zoom - zoom) < 1e-9)
})

test('a view change takes about a third of a second, like the CAD tools engineers know', () => {
  assert.ok(FLY_SECONDS >= 0.25 && FLY_SECONDS <= 0.35, `${FLY_SECONDS}s`)
})

test('the first frame after an idle viewport does not swallow the whole fly', () => {
  // The canvas renders on demand, so the first frame of a fly reports the
  // seconds since the LAST frame -- which can be minutes.
  const p = advanceFly(0, 42, false)
  assert.ok(p > 0 && p < 0.25, `one frame moved the fly to ${p}`)
})

test('a fly runs to its end in FLY_SECONDS of ordinary frames', () => {
  let p = 0
  let frames = 0
  while (p < 1 && frames < 1000) {
    p = advanceFly(p, 1 / 60, false)
    frames++
  }
  assert.ok(Math.abs(frames / 60 - FLY_SECONDS) < 0.05, `${frames} frames`)
})

test('with reduced motion the view arrives at once', () => {
  assert.equal(advanceFly(0, 1 / 60, true), 1)
})
