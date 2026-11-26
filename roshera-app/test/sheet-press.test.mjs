// Run: node --experimental-strip-types --test test/sheet-press.test.mjs
//
// Found recording the STEP -> drawing session (2026-11-26): after dragging a
// dimension or a callout on the sheet, the first click on a dimension did
// nothing; the second opened its panel. The drop set a flag ("the next click
// is this drag's drop, swallow it") that only a click ON THE SHEET cleared.
// A drag released off the sheet (over a panel, past the paper's edge), or one
// whose text was redrawn under the cursor before the release, delivers no
// click to the sheet -- so the flag stayed set and swallowed the next real
// click. Whether a click is a drop is now a fact about ITS OWN press: every
// press starts clean.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { sheetPress } from '../src/lib/sheet-press.ts'

const here = dirname(fileURLToPath(import.meta.url))
const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8').replace(/\r\n/g, '\n')

test('the click that ends a drag is the drop, not a selection', () => {
  const press = sheetPress()
  press.begin()
  press.dragged()
  assert.equal(press.clickIsDrop(), true)
})

test('a press that did not travel is a click', () => {
  const press = sheetPress()
  press.begin()
  assert.equal(press.clickIsDrop(), false)
})

test('a drag dropped off the sheet does not swallow the next click', () => {
  const press = sheetPress()
  // Drag a callout and let go over the Sheet panel: no click reaches the sheet.
  press.begin()
  press.dragged()
  // The engineer now clicks a dimension.
  press.begin()
  assert.equal(press.clickIsDrop(), false, 'the new press starts clean')
})

test('a drag abandoned with Escape still ends in a drop, not a selection', () => {
  const press = sheetPress()
  press.begin()
  press.dragged()
  // Escape drops the drag; the button is still down and its release clicks.
  assert.equal(press.clickIsDrop(), true)
  press.begin()
  assert.equal(press.clickIsDrop(), false)
})

test('the workspace judges each click by its own press', () => {
  assert.ok(!/dimDraggedRef/.test(ws), 'no flag waits for a click that may never come')
  const down = ws.slice(ws.indexOf('const onCanvasMouseDown = useCallback('), ws.indexOf('const onSheetClick = useCallback('))
  assert.ok(/press\.current\.begin\(\)/.test(down), 'every mouse-down starts a new press')
  assert.ok(
    down.indexOf('press.current.begin()') < down.indexOf("closest('button')"),
    'before any early return',
  )
  assert.ok(/dd\.moved = true\s*\n\s*press\.current\.dragged\(\)/.test(down), 'a press that travels is marked as it travels')
  const click = ws.slice(ws.indexOf('const onSheetClick = useCallback('), ws.indexOf('const onSheetContextMenu'))
  assert.ok(/if \(press\.current\.clickIsDrop\(\)\) return/.test(click), 'the drop is not a selection')
})
