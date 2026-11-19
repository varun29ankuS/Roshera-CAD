// Run: node --experimental-strip-types --test test/drawing-preview.test.mjs
//
// Varun (2026-11-18): "its unable to make the drawing... why ?" The sheet
// WAS made; the app showed nothing because it fetched the on-screen sheet
// through the export route, which refuses a sheet whose layout-quality
// certificate fails (label collisions, redundant dimensions). The server
// documents an escape for exactly this, a draft for human review
// (`acknowledge_layout_issues=true`). The on-screen preview uses it; the
// sheet panel lists the findings beside it; every DOWNLOAD stays refused
// until the layout passes; and the escape never covers an UNSOUND sheet
// (`acknowledge_unsound` is never sent).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { drawingSvgUrl } from '../src/lib/drawing-svg-url.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

test('the on-screen preview asks for the draft; a download does not', () => {
  // The screen asks for the PREVIEW (2026-11-18: through the export route
  // every redraw took 7-8 s and recorded an export in the history).
  assert.equal(drawingSvgUrl('', 'abc', { review: true }), '/api/drawings/abc/preview.svg')
  assert.equal(drawingSvgUrl('', 'abc', { review: false }), '/api/drawings/abc/svg')
  assert.equal(drawingSvgUrl('http://h:8081', 'a b', { review: false }), 'http://h:8081/api/drawings/a%20b/svg')
  for (const review of [true, false]) {
    assert.ok(!drawingSvgUrl('', 'x', { review }).includes('acknowledge_unsound'), 'never bypasses soundness')
  }
})

test('the workspace previews with review; downloads keep the gate', () => {
  const api = src('lib/drawings-api.ts')
  assert.ok(/export async function previewDrawingSvg/.test(api), 'a preview fetch exists')
  assert.ok(/review: true/.test(api), 'and it asks for the review draft')
  const ws = src('components/panels/DrawingsWorkspace.tsx')
  assert.ok(ws.includes('previewDrawingSvg(id)'), 'the sheet on screen uses the preview')
  assert.ok(!/fetchDrawingSvg\(id\)/.test(ws), 'not the export route')
  const download = api.slice(api.indexOf('export async function downloadDrawing'))
  assert.ok(download.includes('fetchDrawingSvg(id)'), 'an SVG download uses the gated route')
  assert.ok(!download.includes('previewDrawingSvg'), 'a download never uses the review draft')
})
