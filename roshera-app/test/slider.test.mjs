// Run: node --experimental-strip-types --test test/slider.test.mjs
//
// The history slider (Varun 2026-11-13: "literally like a slider in a video
// file"): chapter marks spaced along one track, the right end the live
// part. These pin where a mark sits, which mark a pointer is nearest, and
// how play advances.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { markFraction, nearestMark, nextPlayIndex } from '../src/lib/slider.ts'

test('chapter marks are spread end to end along the track', () => {
  assert.equal(markFraction(0, 5), 0)
  assert.equal(markFraction(4, 5), 1)
  assert.equal(markFraction(2, 5), 0.5)
  assert.equal(markFraction(0, 1), 1, 'a single chapter is the live end')
})

test('the pointer picks the nearest mark, clamped to the track', () => {
  // A 400 px track starting at x = 100, 5 marks every 100 px.
  assert.equal(nearestMark(100, 100, 400, 5), 0)
  assert.equal(nearestMark(149, 100, 400, 5), 0)
  assert.equal(nearestMark(151, 100, 400, 5), 1)
  assert.equal(nearestMark(500, 100, 400, 5), 4)
  assert.equal(nearestMark(-20, 100, 400, 5), 0)
  assert.equal(nearestMark(9999, 100, 400, 5), 4)
  assert.equal(nearestMark(300, 100, 400, 0), 0, 'no chapters')
})

test('play advances one chapter at a time and stops at the live end', () => {
  assert.equal(nextPlayIndex(0, 5), 1)
  assert.equal(nextPlayIndex(3, 5), 4)
  assert.equal(nextPlayIndex(4, 5), null, 'at the live end: stop')
})
