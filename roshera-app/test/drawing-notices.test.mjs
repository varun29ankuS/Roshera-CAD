// Run: node --experimental-strip-types --test test/drawing-notices.test.mjs
//
// What the Drawing workspace says to the engineer, and asks of them
// (2026-11-26, after the STEP -> drawing recording):
//
//   - its notices are ONE stack. "Removed view TOP -- Undo" and the note of
//     the last edit were two boxes placed on the same spot, one drawn over
//     the other;
//   - the removed-view Undo is a history undo, so it is offered only while
//     the removal is the newest step: any other edit withdraws it;
//   - deleting a drawing asks once. The switcher asked, then the workspace
//     asked again;
//   - what is open about one sheet in the Sheet panel (the check's findings,
//     the add-a-view menu) does not stay open over the next sheet, and the
//     add-a-view menu closes on a press elsewhere;
//   - a popover's "press elsewhere" is heard even where a panel stops the
//     press from travelling on.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const panel = (name) =>
  readFileSync(join(here, '..', 'src', 'components', 'panels', name), 'utf8').replace(/\r\n/g, '\n')
const ws = panel('DrawingsWorkspace.tsx')
const overlays = panel('drawing-overlays.tsx')
const sheetPanel = panel('SheetPanel.tsx')
const dimPanel = panel('DimensionPanel.tsx')

/** The body of `const name = useCallback(` up to the next top-level const. */
function callback(name) {
  const fn = `const ${name} = useCallback(`
  const at = ws.indexOf(fn)
  assert.ok(at > 0, fn)
  return ws.slice(at, ws.indexOf('\n  const ', at + fn.length))
}

test('the notices are one stack: the note above, the Undo at the bottom', () => {
  const s = overlays.indexOf('export function SheetNotices(')
  assert.ok(s > 0, 'the stack is a component')
  const stack = overlays.slice(s, overlays.indexOf('\n}\n', s))
  // One positioned box; the notices inside it are laid out, not placed.
  assert.equal((stack.match(/\babsolute\b/g) ?? []).length, 1, 'one positioned container')
  assert.ok(/className="[^"]*\babsolute bottom-14\b[^"]*\bflex flex-col\b[^"]*"/.test(stack), 'a column at the bottom')
  const note = stack.indexOf('{note !== null && (')
  const removed = stack.indexOf('{removedView !== null && (')
  assert.ok(note > 0 && removed > note, 'the Undo comes last in the column: it keeps the bottom place')
  assert.ok(stack.slice(removed).includes('onClick={onUndo}'), 'the Undo is in the stack')
  // Presses pass through the stack's empty width to the sheet.
  assert.ok(/className="pointer-events-none /.test(stack))
  assert.ok(/const notice = `[^`]*pointer-events-auto/.test(stack))
})

test('the workspace places no notice of its own', () => {
  assert.equal((ws.match(/<SheetNotices\b/g) ?? []).length, 1, 'it renders the stack once')
  assert.ok(!/\{removedView && \(/.test(ws), 'no removed-view box of its own')
  assert.ok(!/\{editNote && \(/.test(ws), 'no note box of its own')
  // Nothing else in the drawing workspace is placed at that spot.
  for (const [name, text] of [['DrawingsWorkspace', ws], ['SheetPanel', sheetPanel], ['DimensionPanel', dimPanel]]) {
    assert.ok(!/\bbottom-14 left-/.test(text), `${name} places nothing at the bottom centre`)
    assert.ok(!/left-1\/2[^"`]*\bbottom-14\b|\bbottom-14\b[^"`]*left-1\/2/.test(text), name)
  }
  assert.equal((overlays.match(/\bbottom-14\b/g) ?? []).length, 1, 'the stack is the one thing at bottom-14 here')
})

test('the removed-view Undo is withdrawn by every other edit of the sheet', () => {
  for (const name of [
    'commitRename',
    'handleAddLike',
    'handleViewEdit',
    'commitViewMove',
    'commitDimMove',
    'applyDimEdit',
    'annotated',
    'changeProjection',
  ]) {
    assert.ok(callback(name).includes('setRemovedView(null)'), `${name} withdraws the offer`)
  }
  // Both annotation panels report through `annotated`.
  assert.equal((ws.match(/onChanged=\{\(note\) => annotated\(activeDrawing\.id, note\)\}/g) ?? []).length, 2)
  // The title block card's save.
  const tb = ws.slice(ws.indexOf('<TitleBlockCard'), ws.indexOf('onError={setError}'))
  assert.ok(tb.includes('setRemovedView(null)'))
  // Undo, redo and an agent's turn change the history under it.
  assert.ok(/const onHistory = \(\) => \{\s*setRemovedView\(null\)/.test(ws))
  assert.ok(/const onTurnEnded = \(\) => \{\s*setRemovedView\(null\)/.test(ws))
})

test('deleting a drawing asks once, where it is deleted', () => {
  assert.ok(!/\bconfirm\(/.test(overlays), 'the switcher only passes the request on')
  assert.ok(/onClick=\{\(\) => onDelete\(d\.id\)\}/.test(overlays))
  assert.equal((ws.match(/\bconfirm\(/g) ?? []).length, 1, 'one question in the workspace')
  const del = callback('handleDelete')
  assert.ok(/\bconfirm\(/.test(del), 'asked by the function that deletes')
  assert.ok(del.indexOf('confirm(') < del.indexOf('await deleteDrawing(id)'), 'before anything is deleted')
  assert.ok(/drawings\.find\(\(d\) => d\.id === id\)\?\.name/.test(del), 'naming the drawing')
})

test("the Sheet panel lets go of one sheet's open boxes when another sheet is shown", () => {
  assert.ok(
    /const \[sheetId, setSheetId\] = useState\(drawing\.id\)\s*if \(sheetId !== drawing\.id\) \{\s*setSheetId\(drawing\.id\)\s*setIssuesOpen\(false\)\s*setAdding\(false\)\s*\}/.test(
      sheetPanel,
    ),
    'the findings and the add-a-view menu are per sheet',
  )
  // Folded or open is how the engineer keeps the panel, not about a sheet.
  assert.ok(!/if \(sheetId !== drawing\.id\) \{[^}]*setOpen\(/.test(sheetPanel))
})

test('the add-a-view menu closes on a press elsewhere', () => {
  assert.ok(/const addMenuRef = useDismiss\(adding, closeAdding\)/.test(sheetPanel))
  assert.ok(/<div ref=\{addMenuRef\} className="relative">/.test(sheetPanel))
})

test("a popover hears a press elsewhere even where a panel stops the press", () => {
  const hook = readFileSync(join(here, '..', 'src', 'hooks', 'use-dismiss.ts'), 'utf8').replace(/\r\n/g, '\n')
  assert.ok(hook.includes('export function useDismiss('))
  // Every popover of the drawing workspace closes through it.
  assert.ok(overlays.includes("import { useDismiss } from '@/hooks/use-dismiss'"))
  assert.ok(!/addEventListener\('mousedown'/.test(overlays), 'none listens for the press on its own')
  // The Sheet panel stops its mouse-downs from travelling on; a bubble-phase
  // listener on the window never heard them.
  assert.ok(/onMouseDown=\{\(e\) => e\.stopPropagation\(\)\}/.test(sheetPanel), 'the panel that stops presses')
  assert.ok(hook.includes("window.addEventListener('mousedown', onDown, true)"), 'heard in the capture phase')
  assert.ok(hook.includes("window.removeEventListener('mousedown', onDown, true)"), 'and removed from the same phase')
})
