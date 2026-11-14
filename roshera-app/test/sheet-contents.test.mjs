import { test } from 'node:test'
import assert from 'node:assert/strict'

const { sheetContents, dimensionWord, checkSummary, sheetHeading } = await import('../src/lib/sheet-contents.ts')

const dim = (id, kind, value, label) => ({ id, kind, value, unit: 'mm', label })
const view = (id, name, scale, dims) => ({ id, name, scale, dimensions: dims, projection: 'orthographic', source: {}, position_mm: [0, 0], polylines: [], extent: {} })
const drawing = {
  id: 'd1',
  name: 'solid_12',
  sheet_size: 'A3',
  projection: 'third',
  views: [
    view('v1', 'Front', 1, [dim('a', 'length', 32, '32.00'), dim('b', 'length', 12, '12.00')]),
    view('v2', 'Top', 1, [dim('c', 'diameter', 80, 'Ø80.00'), dim('d', 'hole_callout', 10, '4× Ø10.00 THRU EQ SP')]),
    view('v3', 'Isometric', 0.5, []),
  ],
}

test('the sheet reads as views with their dimensions, in sheet order', () => {
  const c = sheetContents(drawing)
  assert.deepEqual(c.views.map((v) => [v.name, v.scale, v.dimensions.length]), [['Front', '1:1', 2], ['Top', '1:1', 2], ['Isometric', '1:2', 0]])
  assert.equal(c.dimensionCount, 4)
})

test('the heading names size, projection and view count', () => {
  assert.equal(sheetHeading(drawing), 'A3 · third angle · 3 views')
  assert.equal(sheetHeading({ ...drawing, projection: 'first', views: [drawing.views[0]] }), 'A3 · first angle · 1 view')
})

test('dimension kinds read as words', () => {
  assert.equal(dimensionWord('hole_callout'), 'hole')
  assert.equal(dimensionWord('pcd'), 'pitch circle')
  assert.equal(dimensionWord('some_new_kind'), 'some new kind')
})

test('the sheet check reads as a verdict, problems first', () => {
  assert.deepEqual(checkSummary({ passed: true, sheet_utilization: 0.4213, issues: [] }), { tone: 'positive', text: 'Sheet check passed · 42% of the sheet used' })
  assert.deepEqual(
    checkSummary({ passed: false, sheet_utilization: 0.5, issues: [{ severity: 'error', kind: 'view_overlap', message: 'x' }, { severity: 'warning', kind: 'w', message: 'y' }] }),
    { tone: 'negative', text: '1 problem, 1 warning' },
  )
  assert.deepEqual(checkSummary({ passed: true, sheet_utilization: 0.3, issues: [{ severity: 'warning', kind: 'w', message: 'y' }] }), { tone: 'caution', text: 'Passed with 1 warning · 30% of the sheet used' })
  assert.deepEqual(checkSummary(null), { tone: 'muted', text: 'Sheet not checked yet' })
})
