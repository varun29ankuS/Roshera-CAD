/**
 * The title block card's draft after the server's title block changes.
 *
 * Each field saves when the person leaves it, so the server's answer to one
 * field's save can arrive while the person is typing in the next. A field
 * keeps what the person typed if they changed it since the last server value
 * (`before`); every other field takes the new server value.
 */
/**
 * The open drawing with a saved title block applied -- only when the save
 * was for THAT drawing. A field saves when the person leaves it, and the
 * answer can arrive after another sheet was opened; applied blindly it put
 * one drawing's title block on the other, and the next save from the card
 * then wrote the difference to the wrong drawing.
 */
export function applySavedTitleBlock<D extends { id: string }, T>(
  open: D | null,
  drawingId: string,
  saved: T,
): D | null {
  if (!open || open.id !== drawingId) return open
  return { ...open, title_block: saved }
}

export function mergeTitleBlockDraft<T extends object>(before: T, draft: T, server: T): T {
  const out = { ...server }
  for (const key of Object.keys(draft) as (keyof T)[]) {
    if (draft[key] !== before[key]) out[key] = draft[key]
  }
  return out
}
