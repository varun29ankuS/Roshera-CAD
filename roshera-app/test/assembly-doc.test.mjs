import { test } from 'node:test'
import assert from 'node:assert/strict'

const { assemblyStatus, freedomLabel, mateGlyph, mateKindName } = await import('../src/lib/assembly-doc.ts')

test('the assembly status reads as one short verdict', () => {
  assert.deepEqual(assemblyStatus({ status: 'fully_constrained' }), { tone: 'positive', text: '✓ fully located' })
  assert.deepEqual(assemblyStatus({ status: 'mobile', dof: 1 }), { tone: 'info', text: '1 free motion' })
  assert.deepEqual(assemblyStatus({ status: 'mobile', dof: 6 }), { tone: 'info', text: '6 free motions' })
  assert.deepEqual(assemblyStatus({ status: 'over_constrained', redundant: 2 }), { tone: 'caution', text: '2 redundant mates' })
  assert.deepEqual(assemblyStatus({ status: 'conflicting', conflicts: 1 }), { tone: 'negative', text: '✗ 1 conflict' })
  assert.deepEqual(assemblyStatus(null), { tone: 'muted', text: 'not solved' })
})

test('an instance reads as grounded, fixed, or its free motions in words', () => {
  assert.equal(freedomLabel({ status: 'fully_constrained' }, true), 'grounded')
  assert.equal(freedomLabel({ status: 'fully_constrained' }, false), 'fixed')
  assert.equal(
    freedomLabel({ status: 'mobile', dof: 1, motions: [{ motion: 'rotation_about', point: [0, 0, 0], axis: [0, 0, 1] }] }, false),
    'rotates about Z',
  )
  assert.equal(
    freedomLabel({ status: 'mobile', dof: 2, motions: [{ motion: 'rotation_about', point: [0, 0, 0], axis: [0, 0, -1] }, { motion: 'translation_along', direction: [0, 0, 1] }] }, false),
    'rotates about Z · slides along Z',
  )
  assert.equal(freedomLabel({ status: 'mobile', dof: 6, motions: [] }, false), 'free (6)')
  assert.equal(
    freedomLabel({ status: 'mobile', dof: 1, motions: [{ motion: 'translation_along', direction: [0.6, 0.8, 0] }] }, false),
    'slides along (0.6, 0.8, 0)',
  )
})

test('mate kinds have a glyph and a name', () => {
  assert.equal(mateGlyph({ Revolute: { limits: null } }), '⟳')
  assert.equal(mateGlyph('Fastened'), '⚓')
  assert.equal(mateKindName({ Slider: { limits: null } }), 'Slider')
  assert.equal(mateKindName('Ball'), 'Ball')
})
