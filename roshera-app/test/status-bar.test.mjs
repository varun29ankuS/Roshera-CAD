import { test } from 'node:test'
import assert from 'node:assert/strict'
import { formatPoint, underCursorLabel, soundLabel } from '../src/lib/status-bar.ts'

test('a cursor point reads as X Y Z in millimetres, two decimals', () => {
  assert.equal(formatPoint([12.4, -3, 20.006]), 'X 12.40  Y −3.00  Z 20.01')
})

test('no point under the cursor reads as a dash', () => {
  assert.equal(formatPoint(null), '—')
})

test('what is under the cursor names the part and its kernel face', () => {
  assert.equal(underCursorLabel({ part: 'Flange', faceId: 56 }), 'Flange · face 56')
  assert.equal(underCursorLabel({ part: 'Flange', faceId: undefined }), 'Flange')
  assert.equal(underCursorLabel(null), 'nothing')
})

test('the history verdict: sound, not sound, or unknown when unread', () => {
  assert.deepEqual(soundLabel({ loaded: true, sound: true }), { text: '✓ sound', tone: 'positive' })
  assert.deepEqual(soundLabel({ loaded: true, sound: false }), { text: '✗ not sound', tone: 'negative' })
  assert.deepEqual(soundLabel({ loaded: false, sound: true }), { text: 'history unread', tone: 'muted' })
})

test('only the nearest part under the cursor reports (a part behind never overwrites it)', async () => {
  const { isNearestHit } = await import('../src/lib/status-bar.ts')
  const front = { id: 'front' }
  const back = { id: 'back' }
  const intersections = [{ eventObject: front }, { eventObject: back }]
  assert.equal(isNearestHit(intersections, front), true)
  assert.equal(isNearestHit(intersections, back), false)
  assert.equal(isNearestHit([], front), false)
})
