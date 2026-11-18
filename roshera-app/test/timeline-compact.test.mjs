// Run: node --experimental-strip-types --test test/timeline-compact.test.mjs
//
// Varun (2026-11-18): "warning cant take up 50% of timeline... even the
// buttons to the left of pictures are taking up too much space". The
// history's pictures get the bar: a durability warning is a short chip
// (the full account on hover), and the transport is one Play button -- the
// arrow keys, Home, End and Space step and play, and a click on a picture
// goes to it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { durabilityNotice } from '../src/lib/timeline-events.ts'

const here = dirname(fileURLToPath(import.meta.url))

test('a durability warning is a short chip; the full account is on hover', () => {
  const partly = durabilityNotice({
    state: 'quarantined',
    events_served: 35,
    events_total: 51,
    first_break_sequence: 36,
    first_break_kind: 'import_step',
    reason: 'the stored file is missing',
  })
  assert.ok(partly, 'a partly restored history is disclosed')
  assert.equal(partly.label, '16 steps set aside')
  assert.ok(partly.detail.includes('The first 35 of 51 steps are restored'), 'the full account stays, on hover')
  const none = durabilityNotice({
    state: 'quarantined',
    events_served: 0,
    events_total: 9,
    first_break_sequence: 1,
    first_break_kind: 'x',
    reason: 'y',
  })
  assert.equal(none.label, 'history set aside')
  for (const n of [partly, none, durabilityNotice({ state: 'failed', reason: 'x' }), durabilityNotice({ state: 'disabled' })]) {
    if (!n) continue
    assert.ok(n.label.length <= 20, `chip label is short: "${n.label}"`)
  }
})

test('the transport is one Play button; stepping is by keys and pictures', () => {
  const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'FilmStrip.tsx'), 'utf8')
  for (const gone of ['To the start', 'Back one feature', 'Forward one feature', 'To the end (the live part)']) {
    assert.ok(!src.includes(gone), `no "${gone}" button`)
  }
  assert.ok(src.includes("'Play the history'"), 'Play stays')
  assert.ok(/e\.key === 'Home'/.test(src) && /e\.key === 'End'/.test(src), 'Home / End still jump')
})
