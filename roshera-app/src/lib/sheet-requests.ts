/**
 * Which answers may land on the sheet that is on screen.
 *
 * The Drawing workspace reads the open sheet with several requests (the
 * drawing, its picture, its dimension listing with the certificate) and
 * re-reads it after every edit, when an agent's turn ends and when another
 * sheet is chosen. Those reads overlap. Two things must never happen:
 *
 *  - an answer for the sheet that was open a moment ago landing on the sheet
 *    that is open now (its certificate would vouch for a sheet it never
 *    read);
 *  - an older read of the same sheet landing after a newer one (the sheet
 *    would go back to before the last edit).
 *
 * `show` is called when the sheet on screen changes; every read takes a
 * ticket with `begin` and applies its answer only while `isCurrent`. A read
 * of a sheet that is not on screen is not started at all (`begin` gives
 * `null`): an edit that finishes after the engineer moved on must not pull
 * its own sheet back.
 */
export interface SheetRequests {
  /** The sheet on screen is now `id` (`null`: none). Every read in flight
   *  was for what was there before. */
  show: (id: string | null) => void
  /** Start a read of `id`: a ticket, or `null` when `id` is not on screen. */
  begin: (id: string) => number | null
  /** May the read holding `ticket` still apply its answer? */
  isCurrent: (ticket: number | null) => boolean
  /** Is `id` the sheet on screen? */
  isShowing: (id: string) => boolean
}

export function sheetRequests(): SheetRequests {
  let showing: string | null = null
  let newest = 0
  return {
    show: (id) => {
      showing = id
      newest += 1
    },
    begin: (id) => {
      if (id !== showing) return null
      newest += 1
      return newest
    },
    isCurrent: (ticket) => ticket !== null && ticket === newest,
    isShowing: (id) => id === showing,
  }
}
