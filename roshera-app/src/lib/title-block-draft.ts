/**
 * The title block card's draft after the server's title block changes.
 *
 * Each field saves when the person leaves it, so the server's answer to one
 * field's save can arrive while the person is typing in the next. A field
 * keeps what the person typed if they changed it since the last server value
 * (`before`); every other field takes the new server value.
 */
export function mergeTitleBlockDraft<T extends object>(before: T, draft: T, server: T): T {
  const out = { ...server }
  for (const key of Object.keys(draft) as (keyof T)[]) {
    if (draft[key] !== before[key]) out[key] = draft[key]
  }
  return out
}
