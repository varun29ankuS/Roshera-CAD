/**
 * A refused download, in words a person can act on (2026-11-19). The server
 * writes its refusals for agents -- the drawing's id, the gate, the API flag
 * that overrides it -- and the Download menu showed that text as is:
 * "drawing 343a7e65-... failed its layout-quality certificate -- 1
 * Error-severity finding(s) ... exactly what a drawing checker rejects on
 * sight." Each gate gets a plain sentence saying what is wrong and what to
 * do; anything else keeps its words without raw ids.
 */
const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi

export function exportRefusalText(e: unknown): string {
  const gate = (e as { gate?: unknown } | null)?.gate
  switch (gate) {
    case 'sheet_quality':
      return 'This sheet still has a layout problem, so it cannot be released yet. The sheet panel lists it: fix it on the sheet, then download again.'
    case 'sheet_unsound':
      return 'This sheet no longer matches the part: some printed values or features have changed. Make a fresh drawing from the current part, then download it.'
    case 'sheet_part_missing':
      return 'The part this sheet was drawn from is not in the model. Rebuild the part or reopen its document, then make the drawing again.'
    case 'sheet_uncertified':
      return 'The sheet could not be checked against the part just now, so it was not released. Try the download again in a moment.'
  }
  const message = e instanceof Error ? e.message : String(e)
  return message.replace(UUID, 'this drawing').replace(/^drawing this drawing/i, 'This drawing')
}
