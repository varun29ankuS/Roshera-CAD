// Run: node --experimental-strip-types --test test/sheet-fit.test.mjs
//
// Found on the wing rib's sheet (2026-11-19): "Fit" padded the sheet 24 px
// on every side, but the sheet's toolbar (projection, Annotate, Title block,
// Download) sits over the TOP of the canvas and the zoom bar over the
// BOTTOM -- so the toolbar covered the sheet's frame and zone row, and the
// zoom bar and the verdict badge covered the title block. Fit now keeps the
// sheet clear of both bands.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { fitSheetScale, SHEET_FIT_INSETS } from '../src/lib/sheet-fit.ts'

const here = dirname(fileURLToPath(import.meta.url))

test('a fitted sheet clears the toolbar band and the zoom band', () => {
  // A wide window: height is the binding side.
  const [cw, ch] = [1300, 820]
  const [sw, sh] = [3179, 2245] // A1 at 96 dpi
  const s = fitSheetScale(cw, ch, sw, sh)
  const top = (ch - sh * s) / 2
  assert.ok(top >= SHEET_FIT_INSETS.top - 1e-9, `sheet top ${top} under the toolbar`)
  assert.ok(ch - (top + sh * s) >= SHEET_FIT_INSETS.bottom - 1e-9, 'sheet bottom under the zoom bar')
  assert.ok(SHEET_FIT_INSETS.top >= 52 && SHEET_FIT_INSETS.bottom >= 52)
})

test('a narrow window fits by width with the side insets', () => {
  const s = fitSheetScale(600, 2000, 1123, 794)
  assert.ok(Math.abs(1123 * s - (600 - 2 * SHEET_FIT_INSETS.side)) < 1e-6)
})

test('the workspace fits through it', () => {
  const ws = readFileSync(join(here, '..', 'src', 'components', 'panels', 'DrawingsWorkspace.tsx'), 'utf8')
  assert.ok(ws.includes('fitSheetScale('), 'Fit uses the shared insets')
})
