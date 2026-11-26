// Run: node --experimental-strip-types --test test/sheet-requests.test.mjs
//
// The sheet, its dimension listing and its certificate are read with
// separate requests, and reads overlap (an edit's re-read, an agent turn
// ending, a switch to another sheet). An answer for the sheet that was open
// a moment ago must never land on the sheet that is open now: the chip then
// vouches "Sound and complete" for a sheet it never read, and the title
// block card edits one drawing with another's fields.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { sheetRequests } from '../src/lib/sheet-requests.ts'
import { applySavedTitleBlock } from '../src/lib/title-block-draft.ts'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8').replace(/\r\n/g, '\n')

test('an answer for the sheet on screen applies', () => {
  const reads = sheetRequests()
  reads.show('A')
  const t = reads.begin('A')
  assert.notEqual(t, null)
  assert.equal(reads.isCurrent(t), true)
})

test('an answer for the previous sheet is dropped', () => {
  const reads = sheetRequests()
  reads.show('A')
  const slow = reads.begin('A')
  reads.show('B')
  const fresh = reads.begin('B')
  assert.equal(reads.isCurrent(slow), false, "A's answer must not land on B")
  assert.equal(reads.isCurrent(fresh), true)
})

test('an answer that arrives after the sheet was closed is dropped', () => {
  const reads = sheetRequests()
  reads.show('A')
  const slow = reads.begin('A')
  reads.show(null)
  assert.equal(reads.isCurrent(slow), false)
})

test('a read of a sheet that is not on screen does not start', () => {
  const reads = sheetRequests()
  reads.show('B')
  const current = reads.begin('B')
  // An edit of A finishes after the engineer moved to B and asks to re-read A.
  assert.equal(reads.begin('A'), null)
  assert.equal(reads.isCurrent(current), true, "and B's own read still applies")
})

test('of two reads of the same sheet only the newer applies', () => {
  const reads = sheetRequests()
  reads.show('A')
  const older = reads.begin('A')
  const newer = reads.begin('A')
  assert.equal(reads.isCurrent(older), false)
  assert.equal(reads.isCurrent(newer), true)
})

test('a note about a sheet is said only while that sheet is on screen', () => {
  const reads = sheetRequests()
  reads.show('A')
  assert.equal(reads.isShowing('A'), true)
  reads.show('B')
  assert.equal(reads.isShowing('A'), false)
})

test('a saved title block lands only on the drawing it was saved for', () => {
  const a = { id: 'A', title_block: { date: '2026-11-26' } }
  const b = { id: 'B', title_block: { date: '2026-01-01' } }
  const saved = { date: '2026-11-27' }
  assert.deepEqual(applySavedTitleBlock(a, 'A', saved), { id: 'A', title_block: saved })
  assert.equal(applySavedTitleBlock(b, 'A', saved), b, "A's save does not overwrite B")
  assert.equal(applySavedTitleBlock(null, 'A', saved), null)
})

test('the workspace reads the sheet through the guard', () => {
  const start = ws.indexOf('const refreshActive = useCallback(')
  const body = ws.slice(start, ws.indexOf('}, [])', start))
  assert.ok(start > 0, 'refreshActive found')
  assert.ok(/const ticket = sheetReads\.current\.begin\(id\)/.test(body), 'each read takes a ticket')
  assert.ok(/if \(ticket === null\) return/.test(body), 'a read of a sheet not on screen does not start')
  const guard = body.indexOf('if (!sheetReads.current.isCurrent(ticket)) return')
  assert.ok(guard > 0 && guard < body.indexOf('setActiveDrawing(d)'), 'a stale sheet is dropped before it is applied')
  assert.ok(
    /listDimensions\(id\)\s*\.then\(\(listing\) => \{\s*if \(sheetReads\.current\.isCurrent\(ticket\)\) setDimListing\(listing\)/.test(body),
    'and so is a stale certificate',
  )
  assert.ok(/applySavedTitleBlock\(prev, id, updated\)/.test(ws), 'a title block save is applied to its own drawing')
})
