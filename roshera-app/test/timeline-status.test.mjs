// Run: node --experimental-strip-types --test test/timeline-status.test.mjs
//
// Varun (2026-11-18): "the old timeline is back". While the timeline's
// chapters loaded (41-52 s before the server kept them), the strip fell back
// to the old diamond row of raw operations ("Name", "IS"). The film strip of
// pictured chapters is now the ONE history surface: while it loads, or when
// there is nothing yet, the strip says so in one line. The old row's code
// (RecentOpsStrip, EventNode, Connector) is gone.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'Timeline.tsx'), 'utf8')

test('the timeline has one history surface: the pictured film strip', () => {
  for (const gone of ['function RecentOpsStrip', 'function EventNode', 'function Connector', '<RecentOpsStrip']) {
    assert.ok(!src.includes(gone), `the old event row is gone: ${gone}`)
  }
  assert.ok(src.includes('<FilmStrip'), 'the film strip renders the history')
  assert.ok(/historyStatusLine\(loading \|\| chapterState\.loading, durability\)/.test(src), 'while chapters load, the strip says so')
})

test('the status line tells the truth about an empty or unreadable history', () => {
  assert.ok(src.includes("'Loading the history…'"))
  assert.ok(src.includes('event log unreadable at boot'))
  assert.ok(src.includes('persistence off'))
  assert.ok(src.includes('no steps yet'))
  assert.ok(!/â€|Â·/.test(src), 'no mis-encoded characters')
})
