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
