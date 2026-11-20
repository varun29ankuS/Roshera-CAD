// Run: node --experimental-strip-types --test test/sheet-look.test.mjs
//
// Agent eyes, slice 2 (2026-11-20): when the agent looks at a space of a
// drawing sheet, the server sends one AgentShow `sheet_space` frame; the app
// opens that drawing, glides the sheet to the space's box and outlines it,
// with the space's worst finding as the caption. Reduced motion: the same
// move, instant.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { serverMessageSchema } from '../src/lib/ws-schemas.ts'
import { showCaption } from '../src/lib/agent-show.ts'
import {
  isSheetRect,
  lookGlide,
  sheetLookTarget,
  LOOK_MARGIN_PX,
  LOOK_ZOOM_MAX,
  PX_PER_MM,
} from '../src/lib/sheet-look.ts'
import { SHEET_FIT_INSETS, fitSheetScale } from '../src/lib/sheet-fit.ts'

/** Where paper box `rect` lands on screen at `pose`, by the workspace's own
 *  model (DrawingsWorkspace: the sheet centred in the canvas, translated by
 *  `pan`, scaled by `zoom`) -- this test's arithmetic, not the library's. */
function rectOnScreen(canvas, sheetMm, pose, rect) {
  const sx = (mm) => canvas.w / 2 + pose.pan.x + pose.zoom * (mm * PX_PER_MM - (sheetMm.w * PX_PER_MM) / 2)
  const sy = (mm) => canvas.h / 2 + pose.pan.y + pose.zoom * (mm * PX_PER_MM - (sheetMm.h * PX_PER_MM) / 2)
  return { left: sx(rect.x0), top: sy(rect.y0), right: sx(rect.x1), bottom: sy(rect.y1) }
}

const here = dirname(fileURLToPath(import.meta.url))
const src = (...p) => readFileSync(join(here, '..', 'src', ...p), 'utf8')
const DRAWING = 'aaaaaaaa-1111-4222-8333-444444444444'

/** The frame the server's render.png route emits, exactly as serde writes it. */
const frame = (payload) => ({ type: 'AgentShow', payload })
const LOOK = {
  action: 'sheet_space',
  drawing_id: DRAWING,
  space: 'view:FRONT/band:top',
  rect: { x0: 120.5, y0: 80, x1: 260, y1: 96.25 },
  caption: 'FRONT top band: LANE-PITCH lane:1 and lane:2 are 4.10 mm apart (4.1 mm vs 6.5..8.5) -- score 0.85',
}

test('the sheet-space show frame parses, with or without a space', () => {
  const r = serverMessageSchema.safeParse(frame(LOOK))
  assert.ok(r.success, JSON.stringify(r.error?.issues))
  assert.deepEqual(r.data.payload, LOOK)
  const whole = serverMessageSchema.safeParse(
    frame({ action: 'sheet_space', drawing_id: DRAWING, space: null, rect: null, caption: 'the whole sheet, grade 0.43' }),
  )
  assert.ok(whole.success, JSON.stringify(whole.error?.issues))
})

test('a malformed sheet-space frame is refused at the boundary', () => {
  for (const [why, payload] of [
    ['no drawing', { action: 'sheet_space', space: 'frame' }],
    ['not a drawing id', { ...LOOK, drawing_id: 'drawing-7' }],
    ['an inverted box', { ...LOOK, rect: { x0: 260, y0: 80, x1: 120, y1: 96 } }],
    ['a box with a hole in it', { ...LOOK, rect: { x0: 1, y0: 2, x1: 3 } }],
    ['an empty space id', { ...LOOK, space: '' }],
  ]) {
    assert.equal(serverMessageSchema.safeParse(frame(payload)).success, false, why)
  }
  assert.equal(isSheetRect({ x0: 0, y0: 0, x1: Number.NaN, y1: 1 }), false)
  assert.equal(isSheetRect({ x0: 5, y0: 0, x1: 1, y1: 1 }), false)
  assert.equal(isSheetRect(LOOK.rect), true)
})

test('the bar says the agent caption, else names the space', () => {
  assert.equal(showCaption(LOOK), LOOK.caption)
  assert.equal(showCaption({ ...LOOK, caption: null }), 'the sheet space view:FRONT/band:top')
  assert.equal(showCaption({ action: 'sheet_space', drawing_id: DRAWING }), 'the whole sheet')
})

test('the target fits the space box with a margin inside the room the overlays leave, centred', () => {
  const canvas = { w: 1300, h: 820 }
  const sheet = { w: 420, h: 297 } // A3
  const pose = sheetLookTarget(canvas, sheet, LOOK.rect)
  const box = rectOnScreen(canvas, sheet, pose, LOOK.rect)
  const room = {
    left: SHEET_FIT_INSETS.side + LOOK_MARGIN_PX,
    right: canvas.w - SHEET_FIT_INSETS.side - LOOK_MARGIN_PX,
    top: SHEET_FIT_INSETS.top + LOOK_MARGIN_PX,
    bottom: canvas.h - SHEET_FIT_INSETS.bottom - LOOK_MARGIN_PX,
  }
  const eps = 1e-6
  assert.ok(box.left >= room.left - eps && box.right <= room.right + eps, `x ${box.left}..${box.right}`)
  assert.ok(box.top >= room.top - eps && box.bottom <= room.bottom + eps, `y ${box.top}..${box.bottom}`)
  // It FITS: the binding side spans the room (a wide band binds by width).
  assert.ok(Math.abs(box.right - box.left - (room.right - room.left)) < 1e-6, 'fills the room by width')
  // Centred on the canvas.
  assert.ok(Math.abs((box.left + box.right) / 2 - canvas.w / 2) < 1e-6)
  assert.ok(Math.abs((box.top + box.bottom) / 2 - canvas.h / 2) < 1e-6)
  assert.ok(pose.zoom > fitSheetScale(canvas.w, canvas.h, sheet.w * PX_PER_MM, sheet.h * PX_PER_MM), 'zoomed in')
})

test('a tall space binds by height; a hairline lane stays within the zoom limit; no box is Fit', () => {
  const canvas = { w: 1300, h: 820 }
  const sheet = { w: 594, h: 420 }
  const tall = { x0: 300, y0: 40, x1: 312, y1: 380 }
  const pose = sheetLookTarget(canvas, sheet, tall)
  const box = rectOnScreen(canvas, sheet, pose, tall)
  const roomH = canvas.h - SHEET_FIT_INSETS.top - SHEET_FIT_INSETS.bottom - 2 * LOOK_MARGIN_PX
  assert.ok(Math.abs(box.bottom - box.top - roomH) < 1e-6, 'fills the room by height')
  const hair = sheetLookTarget(canvas, sheet, { x0: 100, y0: 100, x1: 100, y1: 100 })
  assert.ok(hair.zoom <= LOOK_ZOOM_MAX && Number.isFinite(hair.pan.x) && Number.isFinite(hair.pan.y))
  const fit = sheetLookTarget(canvas, sheet, null)
  assert.deepEqual(fit.pan, { x: 0, y: 0 })
  assert.equal(fit.zoom, fitSheetScale(canvas.w, canvas.h, sheet.w * PX_PER_MM, sheet.h * PX_PER_MM))
})

test('the glide runs from where the sheet is to the target; reduced motion is instant', () => {
  const from = { zoom: 0.4, pan: { x: 0, y: 0 } }
  const to = { zoom: 3.2, pan: { x: -900, y: 420 } }
  const g = lookGlide(from, to, false)
  assert.ok(g.durationMs > 0)
  assert.deepEqual(g.at(0), from)
  assert.deepEqual(g.at(g.durationMs), to)
  const mid = g.at(g.durationMs / 2)
  assert.ok(mid.zoom > from.zoom && mid.zoom < to.zoom, `mid zoom ${mid.zoom}`)
  assert.ok(mid.pan.x < 0 && mid.pan.x > to.pan.x)
  const still = lookGlide(from, to, true)
  assert.equal(still.durationMs, 0, 'reduced motion: no glide')
  assert.deepEqual(still.at(0), to, 'reduced motion: straight to the space')
})

test('the store carries a sheet-space show to the viewer, and the viewer glides and outlines', () => {
  const store = src('stores', 'agent-show-store.ts')
  const at = store.indexOf("case 'sheet_space'")
  assert.ok(at > 0, 'the store handles the action')
  const body = store.slice(at, store.indexOf('return', at))
  assert.ok(body.includes('openDrawing(a.drawing_id)'), 'it opens that drawing in the Drawing workspace')
  assert.ok(/set\(\{\s*sheetLook:/.test(body), 'it hands the look to the viewer')
  assert.ok(/takeOver: \(\) => set\(\{[^}]*sheetLook: null/.test(store), 'Take over clears the look')
  const ws = src('components', 'panels', 'DrawingsWorkspace.tsx')
  assert.ok(ws.includes('useAgentShowStore((s) => s.sheetLook)'), 'the viewer reads the look')
  assert.ok(ws.includes('sheetLookTarget(containerSize'), 'it aims with the pure target')
  assert.ok(ws.includes('lookGlide(from, to, prefersReducedMotion())'), 'it glides, honouring reduced motion')
  assert.ok(ws.includes('data-agent-look='), 'it outlines the space')
  assert.ok((ws.match(/cancelLook\(\)/g) ?? []).length >= 3, "the user's wheel and press stop the glide")
  assert.ok(
    /if \(sheetLook === null\) cancelLook\(\)/.test(ws),
    'Take over stops a glide in flight and leaves the sheet where it is',
  )
  assert.ok(/import \{[^}]*PX_PER_MM[^}]*\} from '@\/lib\/sheet-look'/.test(ws), 'one PX_PER_MM for the sheet and the look')
})
