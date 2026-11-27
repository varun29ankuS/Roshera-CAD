import { test } from 'node:test'
import assert from 'node:assert/strict'

const { dockWidth, DOCK_DEFAULT, DOCK_MIN, DOCK_MAX } = await import('../src/lib/dock-width.ts')

test('the Blackboard dock is about 380 px on a normal window', () => {
  assert.equal(DOCK_DEFAULT, 380)
  assert.equal(dockWidth(null, 1920), 380)
  assert.equal(dockWidth(null, 1280), 380)
})

test('a narrow window gets the narrow dock unless the user chose a width', () => {
  assert.equal(dockWidth(null, 1100), 320)
  assert.equal(dockWidth(null, 1279), 320)
})

test('a chosen width is kept, within 320 and 520', () => {
  assert.equal(dockWidth(440, 1920), 440)
  assert.equal(dockWidth(100, 1920), DOCK_MIN)
  assert.equal(dockWidth(900, 1920), DOCK_MAX)
  assert.equal(DOCK_MIN, 320)
  assert.equal(DOCK_MAX, 520)
})

test('the dock never takes more than 40% of the window, and never less than its minimum', () => {
  assert.equal(dockWidth(520, 1100), 440)
  assert.equal(dockWidth(520, 600), DOCK_MIN)
})

test('a saved value that is not a width is ignored', () => {
  assert.equal(dockWidth(NaN, 1920), 380)
  assert.equal(dockWidth(Infinity, 1920), 380)
})
