/**
 * Undo / redo -- one implementation for every surface (Edit menu, command
 * palette, the Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y shortcuts in `shortcuts.ts`)
 * and every workspace (modelling, drawing, assembly).
 *
 * Undo moves the document's history back one step on the server; the server
 * rebuilds the part, its sheets and the viewport scene from the history at
 * that point. On success a `roshera:history-changed` window event tells open
 * panels (e.g. the drawing window) to reload what they show. A refusal is
 * flashed on screen, not only logged.
 */

import { useWSStore } from '@/stores/ws-store'
import { useActionErrorStore } from '@/stores/action-error-store'
import { refusalMessage, tryReadJson } from '@/lib/backend-refusal'

const API_BASE = import.meta.env.VITE_API_URL || ''

/** Fired on `window` after a successful undo or redo. */
export const HISTORY_CHANGED_EVENT = 'roshera:history-changed'

/** Resolves true when the server moved the history. */
export async function timelineAction(action: 'undo' | 'redo'): Promise<boolean> {
  // The server's undo/redo needs the live session id (seeded by the WS
  // `Welcome` frame); with none yet there is nothing to undo against.
  const sessionId = useWSStore.getState().sessionId
  if (!sessionId) return false
  const label = action === 'undo' ? 'Undo' : 'Redo'
  const flash = useActionErrorStore.getState().flash
  try {
    const resp = await fetch(`${API_BASE}/api/timeline/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: sessionId }),
    })
    const body = await tryReadJson(resp)
    if (!resp.ok) {
      flash(`${label} failed: ${refusalMessage(body, resp.status)}`)
      return false
    }
    // Expected refusals (nothing to undo) answer 200 with success: false.
    if (body && body.success === false) {
      flash(`${label}: ${refusalMessage(body, resp.status)}`)
      return false
    }
    if (body && body.model_reconciled === false) {
      flash(`${label} applied, but the 3D model may be stale (reconciliation failed)`)
    }
    window.dispatchEvent(new CustomEvent(HISTORY_CHANGED_EVENT, { detail: { action } }))
    return true
  } catch {
    flash(`Backend unreachable: ${action} not applied`)
    return false
  }
}
