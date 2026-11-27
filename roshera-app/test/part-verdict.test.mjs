import { test } from 'node:test'
import assert from 'node:assert/strict'

const { partVerdict, PART_VERDICT_TEXT } = await import('../src/lib/part-verdict.ts')

test('a part row shows the part certificate: sound, not sound, or not certified', () => {
  assert.equal(partVerdict(true), 'sound')
  assert.equal(partVerdict(false), 'unsound')
})

test('no certificate is never shown as sound', () => {
  assert.equal(partVerdict(undefined), 'uncertified')
  assert.equal(partVerdict(null), 'uncertified')
})

test('each state says in words what it means', () => {
  assert.match(PART_VERDICT_TEXT.sound, /certificate/i)
  assert.match(PART_VERDICT_TEXT.unsound, /not sound/i)
  assert.match(PART_VERDICT_TEXT.uncertified, /not certified/i)
  assert.equal(new Set(Object.values(PART_VERDICT_TEXT)).size, 3)
})
