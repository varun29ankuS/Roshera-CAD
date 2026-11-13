// Run: node --experimental-strip-types --test test/dimension-kinds.test.mjs
//
// The 3D viewport's dimension chips. Every kind the kernel's dimension table
// emits (`readable/dimensions.rs`) either has a chip or is deliberately never
// filtered; the blend callouts `fillet` (R3.00) and `chamfer` (0.50 x 45 deg)
// get chips of their own, so turning chips off hides them like any other kind.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { ALL_DIMENSION_KINDS, DIMENSION_CHIP_LABELS, chipKindOf } from '../src/lib/dimension-kinds.ts'

test('fillet and chamfer rows are governed by their own chips', () => {
  assert.equal(chipKindOf('fillet'), 'fillet')
  assert.equal(chipKindOf('chamfer'), 'chamfer')
  assert.ok(ALL_DIMENSION_KINDS.includes('fillet'))
  assert.ok(ALL_DIMENSION_KINDS.includes('chamfer'))
})

test('the existing chips are unchanged', () => {
  assert.equal(chipKindOf('extent'), 'extent')
  assert.equal(chipKindOf('length'), 'length')
  assert.equal(chipKindOf('position'), 'position')
  assert.equal(chipKindOf('diameter'), 'diameter')
  assert.equal(chipKindOf('radius'), 'diameter')
})

test('angles and interactive-measure kinds are never filtered', () => {
  assert.equal(chipKindOf('angle'), null)
  assert.equal(chipKindOf('face_info'), null)
})

test('every chip has a label, and every chip key routes to itself', () => {
  for (const k of ALL_DIMENSION_KINDS) {
    assert.equal(typeof DIMENSION_CHIP_LABELS[k], 'string')
    assert.ok(DIMENSION_CHIP_LABELS[k].length > 0)
    assert.equal(chipKindOf(k), k)
  }
})
