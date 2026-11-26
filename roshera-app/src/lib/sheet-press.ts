/**
 * One press on the drawing sheet, from the mouse button going down to the
 * click that may follow its release.
 *
 * A press on a dimension that travels is a drag: its release is the DROP,
 * and the click the browser fires after it must not also open the
 * dimension's panel. That used to be a flag set at the drop and cleared by
 * the next click on the sheet. But a drop does not always end in a click on
 * the sheet: released over a panel or past the paper's edge the click goes
 * elsewhere, and when the dragged text was redrawn before the release the
 * browser fires none. The flag then stayed set and swallowed the NEXT click
 * -- the first click on a dimension after a drag did nothing (found
 * recording the STEP -> drawing session, 2026-11-26).
 *
 * Whether a click is a drop is a fact about its own press, so it is kept
 * per press: every mouse-down starts a clean one, and nothing a previous
 * press did can reach the next.
 */
export interface SheetPress {
  /** The mouse button went down: a new press, whatever the last one did. */
  begin: () => void
  /** This press travelled far enough to be a drag. */
  dragged: () => void
  /** Is the click that ends this press the drop of a drag (not a selection)? */
  clickIsDrop: () => boolean
}

export function sheetPress(): SheetPress {
  let travelled = false
  return {
    begin: () => {
      travelled = false
    },
    dragged: () => {
      travelled = true
    },
    clickIsDrop: () => travelled,
  }
}
