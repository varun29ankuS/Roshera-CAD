// Run: node --experimental-strip-types --test test/refusal-card.test.mjs
//
// A refusal the agent forwarded "EXACTLY as returned" rendered as
// "roshera:refusal payload failed validation -- reason: Required". The card
// asked for `reason`; the producers it was forwarding name the same verbatim
// sentence differently:
//   - the backend error catalog (api-server/src/error_catalog.rs) says
//     `{ success: false, error_code, error, retryable, hint? }`;
//   - the timeline routes' 200 refusal (handlers/timeline.rs) say
//     `{ success: false, message }`;
//   - the MCP timeline tools wrap a non-catalog body as `{ refused: <body> }`.
// Each is a deliberate, documented alias now. A payload that does not declare
// itself a failure (no `success: false`, no `refused`, no `error_code`) is
// never read as a refusal: the app probes tool results against the refusal
// schema first, and an ordinary result with a `message` string must not
// become a refusal card.
import { test } from 'node:test'
import assert from 'node:assert/strict'

const { parseCard } = await import('../src/lib/blackboard-cards.ts')

const refusal = (payload) => parseCard('refusal', JSON.stringify(payload))

test('the error-catalog body validates, its `error` read as the verbatim reason', () => {
  const body = {
    success: false,
    error_code: 'blend_failed',
    error: 'blend radius 6 mm exceeds the local curvature limit at edge 14',
    retryable: false,
    hint: 'retry with radius <= 4.2mm',
  }
  const r = refusal(body)
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.reason, body.error)
  assert.equal(r.card.card.error_code, 'blend_failed')
  assert.equal(r.card.card.retryable, false)
  assert.equal(r.card.card.hint, body.hint)
})

test('the timeline success:false body validates, its `message` read as the reason', () => {
  const r = refusal({ success: false, message: 'Nothing to undo' })
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.reason, 'Nothing to undo')
})

test('the mould refusal as timeline_mould forwards it (nested reason) validates', () => {
  // handlers/timeline.rs mould 409, wrapped by roshera-mcp refusalOrFail.
  const r = refusal({
    refused: {
      status: 'MouldRejected',
      reason: 'the edit produces an unsound model',
      kind: 'BrokenDownstream',
      target_sequence: 4,
      parameter: 'radius',
    },
  })
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.reason, 'the edit produces an unsound model')
})

test('an MCP `refused: <body>` wrapper with no top-level reason reads the nested body', () => {
  const r = refusal({ refused: { message: 'feature 3 would break downstream feature 5' } })
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.reason, 'feature 3 would break downstream feature 5')
  const r2 = refusal({ refused: { success: false, error_code: 'unknown_parameter', error: 'no parameter named depth' } })
  assert.equal(r2.ok, true, r2.ok ? '' : r2.error)
  assert.equal(r2.card.card.reason, 'no parameter named depth')
  assert.equal(r2.card.card.error_code, 'unknown_parameter')
})

test('an explicit reason always wins over the aliases', () => {
  const r = refusal({ refused: true, reason: 'the kernel words', error: 'other words', error_code: 'x' })
  assert.equal(r.ok, true, r.ok ? '' : r.error)
  assert.equal(r.card.card.reason, 'the kernel words')
})

test('a result that is not a failure is never read as a refusal', () => {
  assert.equal(refusal({ success: true, message: 'Created box' }).ok, false)
  assert.equal(refusal({ message: 'Created box' }).ok, false)
  assert.equal(refusal({ error: 'a field called error on its own' }).ok, false)
  // verify_claim's success shape (claim.rs) carries `refused: false`.
  assert.equal(refusal({ verified: true, refused: false, message: 'claim holds' }).ok, false)
})

test('a raw conflicted MergeView is still a merge, never a refusal', () => {
  // branches.rs MergeView: success:false on conflict, no sentence of its own.
  const merge = {
    success: false,
    merged_into: 'main',
    strategy: 'three-way',
    events_merged: 0,
    conflicts: [],
    statistics: { events_merged: 0, conflicts_count: 1, auto_resolved: 0, entities_affected: 1, duration_ms: 3 },
    recording_branch: 'main',
  }
  assert.equal(refusal(merge).ok, false)
  assert.equal(parseCard('merge', JSON.stringify(merge)).ok, true)
})

test('a failure with no words at all still refuses to render (honest fallback)', () => {
  const r = refusal({ success: false })
  assert.equal(r.ok, false)
  assert.match(r.error, /reason/)
})
