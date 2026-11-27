/**
 * What the mark beside a PART in the tree says: the kernel's certificate for
 * that part -- sound, not sound, or not certified since it last changed.
 *
 * The tree used to show a tick that meant "this history step re-measured
 * sound", which read as "this part is sound" and stayed green beside parts
 * whose certificate said UNSOUND. A step's re-measure belongs to the history
 * (the timeline shows it); the part row shows the part's own verdict, the
 * same one the Soundness card and the status bar report.
 */

export type PartVerdict = 'sound' | 'unsound' | 'uncertified'

/** `sound` is the scene object's field: true / false from the kernel, absent
 *  (or null) when the part has no certificate since it last changed. Absent
 *  is its own state -- never rounded up to sound. */
export function partVerdict(sound: boolean | null | undefined): PartVerdict {
  if (sound === true) return 'sound'
  if (sound === false) return 'unsound'
  return 'uncertified'
}

/** Hover text and accessible name for each state. */
export const PART_VERDICT_TEXT: Readonly<Record<PartVerdict, string>> = {
  sound: "Sound: the kernel's certificate for this part holds",
  unsound: "Not sound: this part fails the kernel's certificate (ask \"Is this part sound?\" for the checks)",
  uncertified: 'Not certified yet: the kernel has not certified this part since it last changed',
}
