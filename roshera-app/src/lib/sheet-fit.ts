/**
 * How "Fit" places a drawing sheet in the Drawing workspace's canvas.
 *
 * The sheet's toolbar (projection, Annotate, Title block, Download) overlays
 * the TOP of the canvas and the zoom bar / verdict badge overlay the BOTTOM.
 * Fit used to pad 24 px on every side, so the toolbar covered the sheet's
 * frame and zone row and the zoom bar covered the title block (2026-11-19).
 * The insets keep the fitted sheet clear of both bands; the sheet stays
 * centred because the two bands are equal.
 */
export const SHEET_FIT_INSETS = { top: 56, bottom: 56, side: 24 } as const

/** The scale that fits a `sheetW` x `sheetH` px sheet inside the canvas, clear of the overlays. */
export function fitSheetScale(canvasW: number, canvasH: number, sheetW: number, sheetH: number): number {
  if (canvasW <= 0 || canvasH <= 0 || sheetW <= 0 || sheetH <= 0) return 1
  const sx = Math.max(0.01, (canvasW - 2 * SHEET_FIT_INSETS.side) / sheetW)
  const sy = Math.max(0.01, (canvasH - SHEET_FIT_INSETS.top - SHEET_FIT_INSETS.bottom) / sheetH)
  return Math.min(sx, sy)
}
