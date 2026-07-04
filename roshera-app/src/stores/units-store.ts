/**
 * Document-unit store.
 *
 * Tracks the backend's active display unit and a monotonically-increasing
 * `unitEpoch` counter. The epoch is folded into the fetch-key of every
 * hook that pulls unit-formatted labels from the kernel
 * (use-part-dimensions, use-part-labels, use-pinned-measurements). When
 * the unit changes, bumping the epoch makes all three hooks re-fire their
 * fetch — identical to a geometry change — so annotations re-render in
 * the new unit without a topology mutation having occurred.
 *
 * ## Design rationale
 * A separate store (not a slice of scene-store) keeps the unit concern
 * isolated: scene-store is already very large and the unit lives at
 * document level, not object level. The store is tiny and intentionally
 * has no persistence — the authoritative source is the backend; the app
 * GETs it at mount (TopBar.tsx) and stays in sync via the selector's PATCH
 * round-trip.
 *
 * ## Replacement not mutation
 * `setDocumentUnitState` always bumps `unitEpoch` by replacement
 * (`epoch + 1`), never via a direct mutation. Zustand's equality check
 * fires on the new primitive value, which React then propagates to every
 * subscriber.
 */

import { create } from 'zustand'
import type { UnitToken } from '@/lib/units-api'

interface UnitsState {
  /** Active display unit. Default "mm" until the mount GET resolves. */
  documentUnit: UnitToken
  /**
   * Monotonically-increasing counter. Bumped by `setDocumentUnitState`
   * every time the unit changes. Hooks fold this into their fetch keys so
   * a unit change triggers an immediate re-fetch of label/dimension data
   * without waiting for a geometry mutation.
   */
  unitEpoch: number
  /**
   * Update the active unit and bump the epoch. Always replaces both
   * fields atomically so subscribers see a consistent snapshot.
   */
  setDocumentUnitState: (token: UnitToken) => void
}

export const useUnitsStore = create<UnitsState>((set, get) => ({
  documentUnit: 'mm',
  unitEpoch: 0,
  setDocumentUnitState: (token) =>
    set({ documentUnit: token, unitEpoch: get().unitEpoch + 1 }),
}))
