// Run: node --experimental-strip-types --test test/sheet-size.test.mjs
//
// An existing sheet is put on other paper, or drawn at another scale, from
// the Sheet control. The size was fixed when the sheet was made; an engineer
// could change one view's scale but not the sheet's. The kernel refuses a
// scale the paper cannot hold (it never shrinks the views quietly), and the
// control has to say why in plain words and offer the scale that fits.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ISO_SHEET_SIZES,
  scaleLabel,
  scaleOptions,
  sheetRequest,
  sheetRefusal,
} from '../src/lib/sheet-size.ts'

const here = dirname(fileURLToPath(import.meta.url))
const read = (...p) => readFileSync(join(here, '..', 'src', ...p), 'utf8').replace(/\r\n/g, '\n')

test('the control offers the five ISO sizes, smallest first', () => {
  assert.deepEqual([...ISO_SHEET_SIZES], ['A4', 'A3', 'A2', 'A1', 'A0'])
})

test('a scale reads as a drawing states it', () => {
  assert.equal(scaleLabel(1), '1:1')
  assert.equal(scaleLabel(2), '2:1')
  assert.equal(scaleLabel(0.5), '1:2')
  assert.equal(scaleLabel(0.2), '1:5')
  assert.equal(scaleLabel(0.05), '1:20')
  assert.equal(scaleLabel(0), '?')
  assert.equal(scaleLabel(Number.NaN), '?')
})

test('the scale list always holds the scale the sheet is at', () => {
  assert.ok(scaleOptions(1).includes(1))
  const odd = scaleOptions(0.75)
  assert.ok(odd.includes(0.75), 'a sheet at 1:1.33 still shows its own scale')
  assert.deepEqual(odd, [...odd].sort((a, b) => b - a), 'largest first')
  assert.equal(scaleOptions(2).filter((s) => s === 2).length, 1, 'never twice')
})

test('asking for the paper a sheet is on sends nothing', () => {
  const now = { size: 'A3', way: 'landscape', scale: 1 }
  assert.equal(sheetRequest(now, { ...now }), null)
})

test('a change sends the whole choice: size, way up and scale', () => {
  const now = { size: 'A3', way: 'landscape', scale: 1 }
  assert.deepEqual(sheetRequest(now, { size: 'A2', way: 'landscape', scale: 1 }), {
    sheet_size: 'A2',
    orientation: 'landscape',
    scale: 1,
  })
  assert.deepEqual(sheetRequest(now, { size: 'A3', way: 'portrait', scale: 1 }), {
    sheet_size: 'A3',
    orientation: 'portrait',
    scale: 1,
  })
  assert.deepEqual(sheetRequest(now, { size: 'A3', way: 'landscape', scale: 0.5 }), {
    sheet_size: 'A3',
    orientation: 'landscape',
    scale: 0.5,
  })
})

test('a sheet that does not fit says by how much and what would fit', () => {
  const r = sheetRefusal('scale 5 does not fit sheet A4 landscape', {
    gate: 'sheet_does_not_fit',
    scale: 5,
    sheet: 'A4 landscape',
    overflow_x_mm: 512.3,
    overflow_y_mm: 0,
    largest_scale_that_fits: 0.5,
  })
  assert.equal(
    r.text,
    'The views at 5:1 are too large for A4 landscape: 513 mm too wide. The largest scale that fits is 1:2.',
  )
  assert.equal(r.fits, 0.5)
})

test('a sheet no scale fits says to take larger paper', () => {
  const r = sheetRefusal('x', {
    gate: 'sheet_does_not_fit',
    scale: 1,
    sheet: 'A4 portrait',
    overflow_x_mm: 10,
    overflow_y_mm: 20,
    largest_scale_that_fits: null,
  })
  assert.match(r.text, /10 mm too wide and 20 mm too tall/)
  assert.match(r.text, /choose a larger sheet/)
  assert.equal(r.fits, null)
})

test('any other refusal is the server’s own sentence', () => {
  const r = sheetRefusal('REFUSED: drawing 7 has no view of a part to lay out again', {
    gate: 'sheet_has_no_part_view',
  })
  assert.equal(r.text, 'drawing 7 has no view of a part to lay out again')
  assert.equal(r.fits, null)
  assert.equal(sheetRefusal('500 Internal Server Error', null).text, '500 Internal Server Error')
})

// The control is wired to the route: the workspace calls the API function,
// the API function PATCHes /sheet, and the overlay shows the refusal.
test('the Sheet control reaches the kernel route', () => {
  const api = read('lib', 'drawings-api.ts')
  assert.match(api, /export async function setDrawingSheet\(/)
  assert.match(api, /\/api\/drawings\/\$\{drawingId\}\/sheet`/)
  const ws = read('components', 'panels', 'DrawingsWorkspace.tsx')
  assert.match(ws, /setDrawingSheet\(/, 'the workspace calls the route')
  assert.match(ws, /onSheet=\{/, 'and hands the control its action')
  const overlays = read('components', 'panels', 'drawing-overlays.tsx')
  assert.match(overlays, /refusal\.text/, 'a refusal is shown on the control')
  assert.match(overlays, /role="alert"/, 'as an alert, not a console line')
})
