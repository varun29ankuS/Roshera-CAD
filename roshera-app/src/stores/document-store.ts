/**
 * Document registry + in-place switch — the single place that knows how to
 * move the live app from one document to another WITHOUT a page reload.
 *
 * Switching used to be `openDocument()` followed by `window.location.reload()`.
 * That worked (every store re-hydrates from scratch) but reads as tearing the
 * whole app down for what should feel like clicking a tab — Varun, live:
 * "the switching is not easy." The backend's `POST /api/documents/{id}/open`
 * (`documents.rs::activate`) already resets model/timeline/notebooks
 * server-side and replays through the cold-boot path; nothing broadcasts that
 * over the WebSocket (verified against `documents.rs` — no broadcast call in
 * `activate`), so the client's job is to re-fetch the same things a reload
 * would have re-fetched, not to restart.
 *
 * What this does NOT need to do, and why:
 *  - Re-handshake the WebSocket. `AppState.active_document` is a single
 *    process-wide value, not session- or connection-scoped (verified in
 *    `main.rs`) — the existing WS connection already serves whichever
 *    document is active the moment the backend flips it.
 *  - Touch the auth token. Auth is purely user-scoped (`auth_middleware.rs`);
 *    switching documents neither requires nor invalidates it.
 *
 * What it DOES re-fetch after the backend confirms the switch:
 *  - The scene (`refreshSceneFromServer`, `lib/ws-bridge.ts`) — same primitive
 *    the WS reconnect path uses for a full scene resync.
 *  - The Blackboard's active-scope notebook (`syncActiveScope`,
 *    `lib/blackboard-api.ts`) — the scope KEY doesn't change (the backend
 *    resolves "document" scope against its own global `active_document`), so
 *    this is a plain re-fetch, not a scope swap.
 *  - The document list itself, so every tab's `active` flag is current.
 *
 * What it DOES reset: the ACP agent session (`resetAcpClient`,
 * `lib/acp-blackboard.ts`). A session's conversational history belongs to
 * the document it was opened against — carrying it across a switch would
 * have the agent answer the next turn about a document it can no longer
 * see. Unlike the WS connection above, this is genuinely session-scoped, so
 * it genuinely needs the reset; the next `getAcpClient()` call opens a
 * fresh session lazily, same as after any other client-discarded reset.
 *
 * `epoch` is the invalidation signal for everything else that hydrates from
 * server state on its own schedule but has no reason to import this store's
 * internals — Timeline resets its selected branch to `main` (the backend
 * resets it there too) and re-fetches history; ModelTree re-fetches hierarchy
 * + datums; the unit selector re-fetches the document's display unit. Same
 * idiom as `units-store.ts`'s `unitEpoch` — a monotonic counter a subscriber
 * adds to its own fetch-on-mount `useEffect` dependency array.
 *
 * ★ Never optimistic: `active` on every `DocumentInfo` in `documents`, and
 * `epoch`, only change AFTER the backend confirms — never on click. A tab
 * that highlights before the switch actually lands is the exact
 * data-loss-shaped mistake this was built to avoid.
 */
import { create } from 'zustand'
import { createDocument, listDocuments, openDocument, type DocumentInfo } from '@/lib/documents-api'
import { refreshSceneFromServer } from '@/lib/ws-bridge'
import { syncActiveScope } from '@/lib/blackboard-api'
import { resetAcpClient } from '@/lib/acp-blackboard'

export interface DocumentSwitchResult {
  success: boolean
  error?: string
}

interface DocumentStoreState {
  documents: DocumentInfo[]
  /** Id of the document currently being switched to, else `null`. Drives the
   *  busy indicator on its tab — a switch with zero visible feedback reads
   *  as a dead click, not a fast one. */
  switchingId: string | null
  epoch: number
  /** Re-fetch the document list without switching anything. */
  refresh: () => Promise<void>
  /** Create a document and switch to it in place. */
  createAndSwitch: (name?: string) => Promise<DocumentSwitchResult>
  /** Switch to an already-registered document in place. No-ops (success)
   *  if `id` is already active. */
  switchTo: (id: string) => Promise<DocumentSwitchResult>
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : 'backend unreachable'
}

export const useDocumentStore = create<DocumentStoreState>((set, get) => ({
  documents: [],
  switchingId: null,
  epoch: 0,

  refresh: async () => {
    try {
      const docs = await listDocuments()
      set({ documents: docs })
    } catch {
      // Backend not reachable yet — leave the previous list (possibly
      // empty) in place; the caller decides whether to retry.
    }
  },

  switchTo: async (id: string) => {
    const already = get().documents.find((d) => d.id === id)
    if (already?.active) return { success: true }
    if (get().switchingId) return { success: false, error: 'a switch is already in progress' }

    set({ switchingId: id })
    try {
      await openDocument(id)
      // Backend confirmed (200) — the new document is live server-side.
      // The agent's ACP session is scoped to the document it was talking
      // about (its cwd, its conversational history) — carrying it across a
      // switch would answer the next turn about the WRONG document. Reset
      // it here, never optimistically before the backend confirms, so the
      // next `getAcpClient()` call opens a fresh session against whatever
      // document is active by the time that call happens.
      resetAcpClient()
      // Re-hydrate the stores that own server-sourced state. A failure in
      // any one of these is reported by that primitive itself (scene sync
      // already posts a Blackboard line on failure); we still complete the
      // switch rather than leaving the UI half-migrated, since the backend
      // has already committed to the new document regardless.
      const [docs] = await Promise.all([listDocuments(), refreshSceneFromServer(), syncActiveScope()])
      set((state) => ({ documents: docs, switchingId: null, epoch: state.epoch + 1 }))
      return { success: true }
    } catch (err) {
      set({ switchingId: null })
      return { success: false, error: errorMessage(err) }
    }
  },

  createAndSwitch: async (name?: string) => {
    if (get().switchingId) return { success: false, error: 'a switch is already in progress' }
    try {
      const doc = await createDocument(name)
      return await get().switchTo(doc.id)
    } catch (err) {
      return { success: false, error: errorMessage(err) }
    }
  },
}))
