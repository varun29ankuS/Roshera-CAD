// Run: node --experimental-strip-types --test test/drawing-list-latest.test.mjs
//
// Found rehearsing the demo video (2026-11-18): after a reload into a fresh
// document, the Drawing workspace opened the PREVIOUS document's drawing and
// showed a red "does not resolve against the server's current live state".
// The workspace opened at once (it was the last one used) and listed sheets
// while the server still had the previous document open; the switch to the
// fresh document then reloaded the list correctly -- and the first, older
// listing finished last and put the old drawing back. Same race as the
// scene (test/latest-only.test.mjs): only the newest listing may apply.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')

test('the sheet list applies only its newest listing', () => {
  const start = src.indexOf('const refreshList = useCallback(')
  const body = src.slice(start, src.indexOf('setDrawings(summaries)', start))
  assert.ok(start > 0, 'refreshList found')
  assert.ok(/listRequests\.current\.begin\(\)/.test(body), 'each listing takes a ticket')
  assert.ok(/if \(!listRequests\.current\.isLatest\(ticket\)\) return/.test(body), 'a stale listing is dropped before it is applied')
})
