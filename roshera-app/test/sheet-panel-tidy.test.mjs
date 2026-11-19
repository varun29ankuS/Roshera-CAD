// Run: node --experimental-strip-types --test test/sheet-panel-tidy.test.mjs
//
// Varun (2026-11-19, rehearsing the Volvo video): "the left column that just
// bunches a lot of dimensions looks very unprofessional", then "why are the
// dimensions on the left column .. the dimensions should be edited directly
// on the drawing". The sheet panel lists the VIEWS (name, how many
// dimensions, scale, menu); a dimension is picked, edited and dragged on the
// sheet itself.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const src = (p) => readFileSync(join(here, '..', 'src', p), 'utf8')

test('the sheet panel lists views, not dimensions', () => {
  const panel = src('components/panels/SheetPanel.tsx')
  assert.ok(!panel.includes('onOpenDimension'), 'no dimension opens from the panel')
  assert.ok(!panel.includes('v.dimensions.map'), 'no dimension is listed in the panel')
  assert.ok(panel.includes('v.dimensions.length'), 'a view still says how many dimensions it carries')
})

test('a dimension opens from the sheet', () => {
  const ws = src('components/panels/DrawingsWorkspace.tsx')
  const tag = ws.slice(ws.indexOf('<SheetPanel'), ws.indexOf('/>', ws.indexOf('<SheetPanel')))
  assert.ok(!tag.includes('onOpenDimension'), 'the panel is not handed the dimension editor')
  assert.ok(/openDimension\(/.test(ws), 'the sheet click still opens the dimension editor')
})
