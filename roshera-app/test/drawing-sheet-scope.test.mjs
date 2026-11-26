// Run: node --experimental-strip-types --test test/drawing-sheet-scope.test.mjs
//
// Found recording the STEP -> drawing session (2026-11-26): the title block
// panel and the "moved" note of one document were still on screen in a
// freshly created document. The workspace stays mounted for the life of the
// page, and what the engineer had opened or been told lived in that one
// component -- so it belonged to no document and no sheet. The rule now:
//
//   - the workspace is one component PER DOCUMENT (keyed by the document
//     store's epoch): nothing held for one document can reach the next;
//   - what was opened or said about one sheet (the note, the removed-view
//     undo, the dimension editor, the title block card, the certificate, the
//     sheet check) is dropped when another sheet is shown;
//   - a panel about one dimension or one sheet is keyed by it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8').replace(/\r\n/g, '\n')

test('the workspace is one component per document', () => {
  const start = ws.indexOf('export function DrawingsWorkspace()')
  assert.ok(start > 0)
  const shell = ws.slice(start, ws.indexOf('\n}\n', start))
  assert.ok(/useDocumentStore\(\(st\) => st\.epoch\)/.test(shell), 'it follows the open document')
  assert.ok(/<DocumentDrawings key=\{documentEpoch\} \/>/.test(shell), 'and starts over when the document changes')
  assert.ok(!/useState|useRef/.test(shell), 'it holds nothing itself that could outlive a document')
  assert.ok(!/firstEpoch/.test(ws), 'no hand-kept list of what to clear on a document change')
})

test('another sheet starts clean', () => {
  const at = ws.indexOf('// Another sheet starts clean')
  assert.ok(at > 0, 'the per-sheet reset is found')
  const effect = ws.slice(at, ws.indexOf('}, [activeId])', at))
  for (const dropped of [
    'setEditNote(null)',
    'setRemovedView(null)',
    'setDimEdit(null)',
    'dimEditAbort.current?.abort()',
    'setTitleBlockOpen(false)',
    'setDimListing(null)',
    'setSheetCheck(null)',
    'setSelectedViewId(null)',
    'setViewMenu(null)',
    'setViewGhost(null)',
    'dimDragRef.current = null',
    'viewDragRef.current = null',
  ]) {
    assert.ok(effect.includes(dropped), `${dropped} when the sheet changes`)
  }
})

test('a note about one sheet is not said on another', () => {
  const s = ws.indexOf('const noteFor = useCallback(')
  assert.ok(s > 0, 'notes name the sheet they are about')
  const say = ws.slice(s, s + 400)
  assert.ok(/if \(!sheetReads\.current\.isShowing\(drawingId\)\) return/.test(say))
  // The edits that answer after a wait.
  for (const fn of [
    'const commitViewMove = useCallback(',
    'const commitDimMove = useCallback(',
    'const handleViewEdit = useCallback(',
    'const handleAddLike = useCallback(',
    'const applyDimEdit = useCallback(',
  ]) {
    const at = ws.indexOf(fn)
    assert.ok(at > 0, fn)
    const body = ws.slice(at, ws.indexOf('\n  const ', at + fn.length))
    assert.ok(/noteFor\(/.test(body), `${fn} says its note through noteFor`)
    assert.ok(!/setEditNote\(/.test(body), `${fn} does not say it unconditionally`)
  }
})

test('a panel about one dimension or one sheet is keyed by it', () => {
  assert.ok(
    /<DimensionPanel\s+key=\{`\$\{activeDrawing\.id\}:\$\{dimEdit\.dim\.id\}`\}/.test(ws),
    'the dimension panel: a half-written suppression must not move to the next dimension clicked',
  )
  assert.ok(/<CompletenessChip\s+key=\{activeDrawing\.id\}/.test(ws), 'the completeness card')
  assert.ok(/<TitleBlockCard\s+key=\{activeDrawing\.id\}/.test(ws), 'the title block card')
})

test('coming back to the Drawing workspace re-reads the open sheet', () => {
  // The part may have changed while the workspace was parked: the sheet and
  // its certificate on screen were read before that.
  assert.ok(
    /useEffect\(\(\) => \{\s*if \(docMode === 'drawing' && activeId\) void refreshActive\(activeId\)/.test(ws),
  )
})
