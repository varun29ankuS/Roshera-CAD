import { test } from 'node:test'
import assert from 'node:assert/strict'

const { sizeSummary } = await import('../src/lib/feature-edit.ts')

const F = (key, value) => ({ key, label: key, unit: 'mm', value, edited: false, targets: [] })

test('a cylinder reads as diameter × height', () => {
  assert.equal(sizeSummary('cylinder', [F('radius', 40), F('height', 12), F('base_z', 0)], 0), 'Ø80 × 12')
})

test('a cut or hole reads as its diameter only (its tool length is not a size)', () => {
  assert.equal(sizeSummary('cut', [F('radius', 15), F('height', 40)], 0), 'Ø30')
  assert.equal(sizeSummary('hole', [F('radius', 4), F('height', 20)], 0), 'Ø8')
})

test('a pattern leads with its count', () => {
  assert.equal(sizeSummary('pattern', [F('radius', 4), F('height', 20)], 4), '4× Ø8')
})

test('a fillet reads as R, a chamfer as distance × 45°', () => {
  assert.equal(sizeSummary('fillet', [F('radius', 3)], 0), 'R3')
  assert.equal(sizeSummary('chamfer', [F('distance', 0.5)], 0), '0.5 × 45°')
})

test('a box reads as its three sides; positions never show', () => {
  assert.equal(
    sizeSummary('box', [F('width', 10), F('depth', 20), F('height', 5), F('base_x', 3)], 0),
    '10 × 20 × 5',
  )
})

test('no sizes reads as nothing', () => {
  assert.equal(sizeSummary('transform', [F('base_x', 3)], 0), '')
})
