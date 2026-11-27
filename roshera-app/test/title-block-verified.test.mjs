import { test } from 'node:test'
import assert from 'node:assert/strict'

const { titleBlockVerified, approvalFlag } = await import('../src/lib/title-block-verified.ts')

const block = (value) =>
  `<g class="title-block"><text class="field-label" x="1" y="2">STATUS</text><text class="field-value-strong" x="1" y="3">APPROVED</text>` +
  `<text class="field-label" x="5" y="2">VERIFIED</text><text class="field-value" x="5" y="3">${value}</text>` +
  `<text class="field-label" x="9" y="2">SHEET</text><text class="field-value" x="9" y="3">1 OF 1</text></g>`

test('the sheet says in its own title block whether the kernel verified it', () => {
  assert.equal(titleBlockVerified(block('UNVERIFIED')), false)
  assert.equal(titleBlockVerified(block('VERIFIED')), true)
  assert.equal(titleBlockVerified(block('a3f9c2')), true, 'a stamp of any other kind is a verification')
})

test('a sheet with no verification field is unknown, not unverified and not verified', () => {
  assert.equal(titleBlockVerified('<svg><text class="field-label">STATUS</text></svg>'), null)
  assert.equal(titleBlockVerified(''), null)
  assert.equal(titleBlockVerified(block('')), null)
  assert.equal(titleBlockVerified(block('—')), null, 'a dash is an empty field')
})

test('approved and not verified is flagged; nothing else is', () => {
  assert.equal(approvalFlag(true, false), 'approved-unverified')
  assert.equal(approvalFlag(true, true), 'approved')
  assert.equal(approvalFlag(true, null), 'approved', 'unknown is not accused')
  assert.equal(approvalFlag(false, false), 'for-review')
  assert.equal(approvalFlag(false, true), 'for-review')
})
