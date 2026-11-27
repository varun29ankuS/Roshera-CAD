import { test } from 'node:test'
import assert from 'node:assert/strict'

const { sheetPointMm, formatSheetPoint } = await import('../src/lib/sheet-cursor.ts')

// An A3 sheet (420 x 297 mm) shown 840 x 594 px with its top-left at (100, 50).
const rect = { left: 100, top: 50, width: 840, height: 594 }
const sheet = { widthMm: 420, heightMm: 297 }

test('the cursor is read in sheet millimetres from the bottom-left corner', () => {
  assert.deepEqual(sheetPointMm({ x: 100, y: 644 }, rect, sheet), { x: 0, y: 0 })
  assert.deepEqual(sheetPointMm({ x: 940, y: 50 }, rect, sheet), { x: 420, y: 297 })
  assert.deepEqual(sheetPointMm({ x: 520, y: 347 }, rect, sheet), { x: 210, y: 148.5 })
})

test('zoom changes the pixels, not the millimetres', () => {
  const zoomed = { left: -300, top: -200, width: 1680, height: 1188 }
  assert.deepEqual(sheetPointMm({ x: 540, y: 394 }, zoomed, sheet), { x: 210, y: 148.5 })
})

test('off the paper there is no position', () => {
  assert.equal(sheetPointMm({ x: 99, y: 300 }, rect, sheet), null)
  assert.equal(sheetPointMm({ x: 500, y: 645 }, rect, sheet), null)
  assert.equal(sheetPointMm({ x: 500, y: 300 }, { ...rect, width: 0 }, sheet), null)
})

test('the readout is to a tenth of a millimetre, with the unit', () => {
  assert.equal(formatSheetPoint({ x: 210, y: 148.5 }), 'X 210.0  Y 148.5 mm')
  assert.equal(formatSheetPoint({ x: 3.14159, y: 0 }), 'X 3.1  Y 0.0 mm')
})
