import { test } from 'node:test'
import assert from 'node:assert/strict'

const { plotLayer, plotKind, plotPlan, plotAt, shouldPlot, PLOT_LAYERS, PLOT_TOTAL_MS } = await import(
  '../src/lib/sheet-plot.ts'
)

const el = (tag, classes = [], where = {}) => ({
  tag,
  classes,
  inView: false,
  inTitleBlock: false,
  inZones: false,
  ...where,
})

test('the pen works in a draughtsman\'s order: border, title block, visible edges, hidden lines, dimensions, text', () => {
  assert.deepEqual(PLOT_LAYERS, ['frame', 'title', 'visible', 'hidden', 'dims', 'text'])
})

test('every mark on a sheet belongs to one layer', () => {
  assert.equal(plotLayer(el('rect', ['sheet'])), null, 'the paper itself is not drawn')
  assert.equal(plotLayer(el('rect', ['frame'])), 'frame')
  assert.equal(plotLayer(el('line', ['zone-mark'], { inZones: true })), 'frame')
  assert.equal(plotLayer(el('rect', ['titleblock-inner'], { inTitleBlock: true })), 'title')
  assert.equal(plotLayer(el('polyline', [], { inView: true })), 'visible')
  assert.equal(plotLayer(el('circle', [], { inView: true })), 'visible')
  assert.equal(plotLayer(el('polyline', ['hidden'], { inView: true })), 'hidden')
  assert.equal(plotLayer(el('line', ['centerline'], { inView: true })), 'hidden')
  assert.equal(plotLayer(el('line', ['dim-line'])), 'dims')
  assert.equal(plotLayer(el('polygon', ['dim-arrow'])), 'dims')
  assert.equal(plotLayer(el('line', ['hole-table-inner'])), 'dims')
})

test('lettering is stamped last, wherever it sits', () => {
  assert.equal(plotLayer(el('text', ['zone'], { inZones: true })), 'text')
  assert.equal(plotLayer(el('text', ['field-value'], { inTitleBlock: true })), 'text')
  assert.equal(plotLayer(el('text', ['label'], { inView: true })), 'text')
  assert.equal(plotLayer(el('text', ['dim-text'])), 'text')
  assert.equal(plotLayer(el('image', ['view-shaded'])), 'text', 'a shaded picture is laid on at the end, not stroked')
})

test('lines are stroked by the pen; filled marks, dashed lines and lettering are set down whole', () => {
  assert.equal(plotKind(el('polyline', [], { inView: true })), 'stroke')
  assert.equal(plotKind(el('line', ['dim-line'])), 'stroke')
  assert.equal(plotKind(el('rect', ['frame'])), 'stroke')
  // A dash pattern is the line's meaning; stroking it would overwrite the pattern.
  assert.equal(plotKind(el('polyline', ['hidden'], { inView: true })), 'show')
  assert.equal(plotKind(el('line', ['centerline'], { inView: true })), 'show')
  assert.equal(plotKind(el('polygon', ['dim-arrow'])), 'show')
  assert.equal(plotKind(el('rect', ['logo-mark-fill'], { inTitleBlock: true })), 'show')
  assert.equal(plotKind(el('text', ['dim-text'])), 'show')
})

test('the layers follow one another and the whole plot fits the cap', () => {
  const plan = plotPlan()
  let cursor = 0
  for (const layer of PLOT_LAYERS) {
    assert.ok(plan[layer].startMs >= cursor - 1e-9, `${layer} starts after the layer before it`)
    assert.ok(plan[layer].endMs > plan[layer].startMs)
    cursor = plan[layer].endMs
  }
  assert.ok(cursor <= PLOT_TOTAL_MS)
  assert.ok(PLOT_TOTAL_MS <= 1200)
})

test('within a layer the marks go down in order, and none finishes after its layer', () => {
  const plan = plotPlan()
  const n = 977
  let last = -1
  for (const k of [0, 1, 200, 500, 976]) {
    const at = plotAt(plan.visible, k, n)
    assert.ok(at.atMs >= last, 'in order')
    assert.ok(at.atMs >= plan.visible.startMs)
    assert.ok(at.atMs + at.strokeMs <= plan.visible.endMs + 1e-9, `mark ${k} ends inside the layer`)
    last = at.atMs
  }
  // One mark alone still takes a moment to draw, and stays inside its layer.
  const solo = plotAt(plan.frame, 0, 1)
  assert.ok(solo.strokeMs > 0 && solo.atMs + solo.strokeMs <= plan.frame.endMs + 1e-9)
})

test('a huge sheet takes no longer than a small one', () => {
  const plan = plotPlan()
  for (const n of [1, 50, 5000, 100000]) {
    const at = plotAt(plan.visible, n - 1, n)
    assert.ok(at.atMs + at.strokeMs <= PLOT_TOTAL_MS)
  }
})

test('marks are batched onto frame boundaries, not given a thousand different start times', () => {
  const plan = plotPlan()
  const starts = new Set()
  for (let k = 0; k < 977; k++) starts.add(plotAt(plan.visible, k, 977).atMs)
  assert.ok(starts.size <= 40, `${starts.size} distinct start times`)
})

test('a sheet is plotted the first time it is opened, and never again', () => {
  const seen = new Set()
  assert.equal(shouldPlot(seen, 'sheet-a', false), true)
  seen.add('sheet-a')
  assert.equal(shouldPlot(seen, 'sheet-a', false), false, 'an edit or a re-render does not replot it')
  assert.equal(shouldPlot(seen, 'sheet-b', false), true, 'another sheet is fresh')
})

test('reduced motion, or no sheet, plots nothing', () => {
  assert.equal(shouldPlot(new Set(), 'sheet-a', true), false)
  assert.equal(shouldPlot(new Set(), null, false), false)
})
