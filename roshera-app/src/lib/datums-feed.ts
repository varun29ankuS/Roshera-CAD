/**
 * The document's datums (origin, reference planes, axes), read once for
 * everyone who shows them: the parts tree and each 3D viewport. One request
 * every five seconds while the tab is visible and someone is reading, in
 * place of one loop per reader (lib/shared-poll.ts).
 *
 * The server sends no event when a datum changes, so the list is polled; a
 * change made from this app (create, rename, delete, a document switch)
 * calls `refreshDatums()` rather than waiting for the next tick.
 */

import { browserPollClock, createSharedPoll } from './shared-poll.ts'

const API_BASE = import.meta.env.VITE_API_URL || ''

/** One row of `GET /api/datums`. */
export interface DatumDto {
  id: number
  name: string
  kind: 'origin' | 'plane' | 'axis'
  plane_orientation?: 'xy' | 'xz' | 'yz' | 'custom'
  axis_direction?: 'x' | 'y' | 'z'
  origin: [number, number, number]
  visible: boolean
  is_default: boolean
}

const poll = createSharedPoll<DatumDto[]>({
  load: async () => {
    const resp = await fetch(`${API_BASE}/api/datums`)
    if (!resp.ok) {
      // A 401 is surfaced globally (lib/auth.ts opens the login dialog); this
      // line keeps any other failure diagnosable.
      console.error(`[datums] GET /api/datums failed: ${resp.status} ${resp.statusText}`)
      throw new Error(`datums ${resp.status}`)
    }
    const data: { datums?: unknown } = await resp.json()
    if (!Array.isArray(data.datums)) throw new Error('datums: not a list')
    return data.datums as DatumDto[]
  },
  intervalMs: 5000,
  ...browserPollClock(),
})

/** Read the datums: `listener` gets the list now (if known) and on every
 *  change. Returns the function that stops reading. */
export const subscribeDatums = poll.subscribe

/** Fetch the datums now (after a change made here). */
export const refreshDatums = poll.refresh
