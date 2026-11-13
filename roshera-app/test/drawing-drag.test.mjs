// Run: node --experimental-strip-types --test test/drawing-drag.test.mjs
//
// Dragging a view on the drawing sheet. The sheet is the kernel's SVG: a view
// group sits at `translate(position_mm[0], sheet_h - position_mm[1])`, so a
// view's x runs with the screen and its y runs UP the sheet. These tests pin
// the mapping from a mouse drag (screen px) to the one PATCH the drop sends,
// the click-vs-drag threshold, and which sheet-check findings a move caused.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  DRAG_THRESHOLD_PX,
  describeMove,
  draggedPosition,
  isDrag,
  newLayoutIssues,
  planViewDrag,
  screenPxPerMm,
} from '../src/lib/drawing-drag.ts'

// The kernel's own placement (svg.rs): sheet-space top-left of the view's
// origin for a position, y-down.
const sheetOrigin = (position, sheetH) => [position[0], sheetH - position[1]]

test('a drag right and down moves the view right and DOWN the sheet', () => {
  // 2 screen px per sheet mm; drag 20 px right, 10 px down.
  const p = draggedPosition([100, 150], 20, 10, 2)
  assert.deepEqual(p, [110, 145])
})

test('the dropped view lands where the cursor took it on the kernel sheet', () => {
  const sheetH = 297
  const start = [63.25, 201.5]
  const pxPerMm = 3.1
  const [dx, dy] = [-47, 88]
  const before = sheetOrigin(start, sheetH)
  const after = sheetOrigin(draggedPosition(start, dx, dy, pxPerMm), sheetH)
  assert.ok(Math.abs(after[0] - before[0] - dx / pxPerMm) < 0.006)
  assert.ok(Math.abs(after[1] - before[1] - dy / pxPerMm) < 0.006)
})

test('positions are rounded to 0.01 mm and never carry -0', () => {
  const p = draggedPosition([0, 0], 1 / 3, 0, 1)
  assert.deepEqual(p, [0.33, 0])
  assert.ok(!Object.is(p[1], -0))
})

test('a press that barely moves is a click, not a drag', () => {
  assert.equal(isDrag(3, 2), false)
  assert.equal(isDrag(DRAG_THRESHOLD_PX, 0), false)
  assert.equal(isDrag(DRAG_THRESHOLD_PX + 0.5, 0), true)
  assert.equal(isDrag(0, -6), true)
})

test('a click sends no PATCH (a zero move would still be an undo step)', () => {
  assert.equal(planViewDrag([10, 10], 2, 1, 3), null)
})

test('a drag sends exactly one position edit', () => {
  assert.deepEqual(planViewDrag([10, 10], 30, -15, 3), { position_mm: [20, 15] })
})

test('a drag with no usable sheet scale sends nothing', () => {
  assert.equal(planViewDrag([10, 10], 30, 30, 0), null)
  assert.equal(planViewDrag([10, 10], 30, 30, Number.NaN), null)
})

test('screen px per mm comes from the rendered sheet width', () => {
  assert.equal(screenPxPerMm(840, 420), 2)
  assert.equal(screenPxPerMm(0, 420), null)
  assert.equal(screenPxPerMm(840, 0), null)
  assert.equal(screenPxPerMm(Number.POSITIVE_INFINITY, 420), null)
})

test('the move is described in sheet directions', () => {
  assert.equal(describeMove('FRONT', [10, 10], [22.5, 7]), 'FRONT moved 12.5 mm right, 3 mm down')
  assert.equal(describeMove('TOP', [10, 10], [10, 30]), 'TOP moved 20 mm up')
  assert.equal(describeMove('RIGHT', [10, 10], [4, 10]), 'RIGHT moved 6 mm left')
})

const issue = (kind, message, severity = 'error', view = null) => ({ kind, message, severity, view })

test('only layout findings the move introduced are reported', () => {
  const before = {
    passed: false,
    issues: [
      issue('view_overlap', "views 'TOP' and 'ISO' overlap"),
      issue('undimensioned_view', "view 'ISO' has no dimensions", 'warning', 'ISO'),
    ],
  }
  const after = {
    passed: false,
    issues: [
      issue('view_overlap', "views 'TOP' and 'ISO' overlap"),
      issue('view_overlap', "views 'FRONT' and 'RIGHT' overlap"),
      issue('projection_misaligned', 'Right view is not horizontally aligned with the Front view', 'warning'),
      issue('undimensioned_view', "view 'ISO' has no dimensions", 'warning', 'ISO'),
      issue('dimension_label_collision', 'labels collide', 'warning', 'FRONT'),
    ],
  }
  assert.deepEqual(newLayoutIssues(before, after), [
    "views 'FRONT' and 'RIGHT' overlap",
    'Right view is not horizontally aligned with the Front view',
  ])
})

test('without a before-check every layout finding is reported', () => {
  const after = {
    passed: false,
    issues: [
      issue('view_outside_frame', "view 'FRONT' (with its dimensions) extends past the drawing frame"),
      issue('view_overlaps_title_block', "view 'FRONT' overlaps the title block"),
      issue('sheet_underutilized', 'views fill only 5% of the sheet', 'warning'),
    ],
  }
  assert.deepEqual(newLayoutIssues(null, after), [
    "view 'FRONT' (with its dimensions) extends past the drawing frame",
    "view 'FRONT' overlaps the title block",
  ])
})
