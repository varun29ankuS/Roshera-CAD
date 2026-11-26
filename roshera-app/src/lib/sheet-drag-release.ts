/**
 * Did a drag on the drawing sheet lose its mouse-up?
 *
 * A drag (panning the sheet, moving a view, moving a dimension) is held in
 * the workspace from the mouse-down until the mouse-up. The mouse-up does
 * not always arrive: Alt+Tab mid-drag, a system dialog, the screen locking,
 * a second button pressed and released over a native menu -- the button
 * comes up where the page is not listening. The drag then stayed in hand,
 * and on return the sheet (or the view's ghost, or the dimension's text)
 * followed a pointer nobody was pressing, until some later click ended it --
 * as a DROP, sending a move the engineer never made (2026-11-26).
 *
 * The browser does say the truth on the very next pointer movement it
 * reports: `buttons` is the set of buttons held NOW. A move with the primary
 * button up, while a drag is in hand, means the release was missed.
 *
 * Why this and not pointer capture: capture needs pointer events, which a
 * script cannot synthesise the way the app's own checks and recordings do
 * (`dispatchEvent(new MouseEvent(...))` raises no pointer event, and
 * `setPointerCapture` throws for a pointer the browser did not create); and
 * it does not help the case that matters -- the OS taking focus away fires
 * neither `pointerup` nor `pointercancel` for a mouse.
 *
 * Only an event the BROWSER made is believed. A scripted `MouseEvent`
 * carries `buttons: 0` unless its author thought to set it, so it says
 * nothing about the button; treating it as a release would end every
 * scripted drag at its first move. (Input injected through the browser's
 * own automation channel is trusted and must state the held button --
 * `buttons: 1` -- as a real mouse does; Puppeteer and the DevTools MCP do.)
 */
export interface PointerMoveLike {
  /** Made by the browser from real (or injected) input, not by a script. */
  isTrusted: boolean
  /** Bit set of the buttons held now; bit 0 is the primary button. */
  buttons: number
}

const PRIMARY_BUTTON = 1

/** True when this pointer movement shows the primary button is no longer
 *  held -- so a drag still in hand has lost its mouse-up. */
export function releaseWasMissed(move: PointerMoveLike): boolean {
  return move.isTrusted && (move.buttons & PRIMARY_BUTTON) === 0
}
