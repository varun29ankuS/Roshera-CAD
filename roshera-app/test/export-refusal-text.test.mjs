// Run: node --experimental-strip-types --test test/export-refusal-text.test.mjs
//
// Recording the wing-rib video (2026-11-19), Download > PDF showed the
// server's agent-facing refusal: "drawing 343a7e65-24a3-4262-83f6-... failed
// its layout-quality certificate -- 1 Error-severity finding(s) (...):
// exactly what a drawing checker rejects on sight." A person is told what is
// wrong and what to do, in plain words, with no ids or API flags.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { exportRefusalText } from '../src/lib/export-refusal.ts'

const here = dirname(fileURLToPath(import.meta.url))
const refusal = (message, gate) => Object.assign(new Error(message), { name: 'DrawingRefusal', gate })
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i

test('a layout error reads as what to do, not as a gate', () => {
  const t = exportRefusalText(
    refusal('drawing 343a7e65-24a3-4262-83f6-a8a6fdd81bdd failed its layout-quality certificate — 1 Error-severity finding(s)', 'sheet_quality'),
  )
  assert.doesNotMatch(t, UUID)
  assert.doesNotMatch(t, /acknowledge_|Error-severity|certificate —/)
  assert.match(t, /layout/i)
  assert.match(t, /sheet panel/i)
})

test('a sheet that no longer matches its part says so', () => {
  const t = exportRefusalText(refusal('drawing 1d2c… is UNSOUND against the live model: 2 stale', 'sheet_unsound'))
  assert.match(t, /no longer matches/i)
  assert.doesNotMatch(t, UUID)
})

test('a missing part says so', () => {
  assert.match(exportRefusalText(refusal('…', 'sheet_part_missing')), /part.*not in the model/i)
})

test('anything else keeps its words but loses raw ids', () => {
  const t = exportRefusalText(new Error('drawing 343a7e65-24a3-4262-83f6-a8a6fdd81bdd not found'))
  assert.doesNotMatch(t, UUID)
  assert.match(t, /not found/)
})

test('the Download action uses it', () => {
  const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')
  assert.ok(ws.includes('exportRefusalText('), 'the download error goes through the plain-words text')
})
