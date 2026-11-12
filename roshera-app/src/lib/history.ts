/**
 * Undo / redo -- one implementation for every surface (Edit menu, command
 * palette, Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y) and every workspace (modelling,
 * drawing, assembly).
 *
 * Undo moves the document's history back one step on the server; the server
 * rebuilds the part, its sheets and the viewport scene from the history at
 * that point. On success a `roshera:history-changed` window event tells open
 * panels (e.g. the drawing window) to reload what they show.
 */

import { useWSStore } from '@/stores/ws-store'
import { useBlackboardStore } from '@/stores/blackboard-store'
import { useSceneStore } from '@/stores/scene-store'
import { refusalMessage, tryReadJson } from '@/lib/backend-refusal'

const API_BASE = import.meta.env.VITE_API_URL || ''

/** Fired on `window` after a successful undo or redo. */
export const HISTORY_CHANGED_EVENT = 'roshera:history-changed'

export async function timelineAction(action: 'undo' | 'redo'): Promise<void> {
  // The server's undo/redo needs the live session id (seeded by the WS
  // `Welcome` frame); with none yet there is nothing to undo against.
  const sessionId = useWSStore.getState().sessionId
  if (!sessionId) return
  const label = action === 'undo' ? 'Undo' : 'Redo'
  try {
    const resp = await fetch(`${API_BASE}/api/timeline/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: sessionId }),
    })
    const body = await tryReadJson(resp)
    if (!resp.ok) {
      useBlackboardStore.getState().addLine(`${label} failed: ${refusalMessage(body, resp.status)}`, 'system')
      return
    }
    // Expected refusals (nothing to undo) answer 200 with success: false.
    if (body && body.success === false) {
      useBlackboardStore.getState().addLine(`${label}: ${refusalMessage(body, resp.status)}`, 'system')
      return
    }
    window.dispatchEvent(new CustomEvent(HISTORY_CHANGED_EVENT, { detail: { action } }))
  } catch {
    useBlackboardStore.getState().addLine(`${label} failed: backend unreachable.`, 'system')
  }
}

/** True when the key press belongs to a text field, not to the app. */
function typingInField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable
}

/**
 * App-wide Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y (Cmd on macOS). Ignored while the
 * engineer types in a field (the field keeps its own text undo) and while a
 * sketch is open (the sketch editor's Ctrl+Z removes its last point).
 * Returns the cleanup.
 */
export function installUndoRedoShortcuts(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return
    if (typingInField(e.target)) return
    if (useSceneStore.getState().sketch?.active) return
    const k = e.key.toLowerCase()
    if (k === 'z' && !e.shiftKey) {
      e.preventDefault()
      void timelineAction('undo')
    } else if ((k === 'z' && e.shiftKey) || (k === 'y' && !e.shiftKey)) {
      e.preventDefault()
      void timelineAction('redo')
    }
  }
  window.addEventListener('keydown', onKey)
  return () => window.removeEventListener('keydown', onKey)
}
