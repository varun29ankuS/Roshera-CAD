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
 *  - The Blackboard's one document notebook (`syncNotebook`,
 *    `lib/blackboard-api.ts`) — the backend resolves it against its own
 *    global `active_document`, so this is a plain re-fetch, nothing to swap.
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
import { createDocument, listDocuments, openDocument, renameDocument, type DocumentInfo } from '@/lib/documents-api'
import { refreshSceneFromServer } from '@/lib/ws-bridge'
import { syncNotebook } from '@/lib/blackboard-api'
import { establishAcpSession, getAcpSessionId, resetAcpClient } from '@/lib/acp-blackboard'
import { enterDocument } from '@/lib/document-scope'
import {
  markStartDocumentUsed,
  planStartDocument,
  readStartDocument,
  writeStartDocument,
} from '@/lib/start-document'
import { useSceneStore } from '@/stores/scene-store'

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
  /** Rename a document's catalog entry. Display name only — never touches
   *  geometry/timeline, and never requires `id` to be the active document.
   *  Updates `documents` locally from the backend's confirmed response, no
   *  refetch/reload needed. */
  rename: (id: string, name: string) => Promise<DocumentSwitchResult>
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : 'backend unreachable'
}

/** This browser's storage, or a store that remembers nothing when it is
 *  blocked (private window, policy): every load then starts a new blank. */
function startStorage(): Pick<Storage, 'getItem' | 'setItem'> {
  try {
    return window.localStorage
  } catch {
    return { getItem: () => null, setItem: () => undefined }
  }
}

/** The start-in-a-fresh-document step runs at most once per page load
 *  (lib/start-document.ts decides which document that is). */
let startedFresh = false

export const useDocumentStore = create<DocumentStoreState>((set, get) => ({
  documents: [],
  switchingId: null,
  epoch: 0,

  refresh: async () => {
    try {
      const docs = await listDocuments()
      set({ documents: docs })
      // A load starts in a fresh document (Varun 2026-11-18): the blank this
      // browser made last time if nothing was built in it, else a new one.
      // Never by reopening the last document -- opening one replays and
      // re-certifies its whole history. Once per load, so a later refresh
      // never yanks the user out of a document they chose.
      if (!startedFresh) {
        startedFresh = true
        const storage = startStorage()
        const plan = planStartDocument(docs, readStartDocument(storage))
        if (plan.kind === 'open') {
          void get().switchTo(plan.id)
          return
        }
        if (plan.kind === 'create') {
          void createDocument('Untitled')
            .then((doc) => {
              writeStartDocument(storage, { id: doc.id, used: false })
              // In the list before the switch marks it active: otherwise the
              // list holds no active document until it is re-fetched, and
              // anything that asks it which document is open hears "none".
              set((s) => ({ documents: s.documents.some((d) => d.id === doc.id) ? s.documents : [...s.documents, doc] }))
              return get().switchTo(doc.id)
            })
            .catch(() => undefined)
          return
        }
      }
      const active = docs.find((d) => d.active)
      // The history caches belong to whatever the server has open -- also
      // when something other than this tab strip (an agent over MCP) moved it.
      if (active && !get().switchingId) enterDocument(active.id)
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
      const hadAgentSession = getAcpSessionId() !== null
      resetAcpClient()
      // The backend has CONFIRMED: from here on this is the active document,
      // so say so at once -- the tab, the history caches (`document-scope`)
      // and `epoch`. Every panel that hydrates on `epoch` (history, chapters,
      // checkpoints, datums, units, drawings, restore status) then loads IN
      // PARALLEL with the scene below, instead of waiting behind the
      // snapshot -- the largest payload of the switch, megabytes of mesh on
      // an imported document. Still never optimistic: nothing here moves
      // before `openDocument` returned 200.
      enterDocument(id, { reopened: true })
      set((state) => ({
        documents: state.documents.map((d) => ({ ...d, active: d.id === id })),
        epoch: state.epoch + 1,
      }))
      // Re-hydrate the stores that own server-sourced state. A failure in
      // any one of these is reported by that primitive itself (scene sync
      // already posts a Blackboard line on failure); we still complete the
      // switch rather than leaving the UI half-migrated, since the backend
      // has already committed to the new document regardless.
      // `switchingId` stays set until the model is on screen: the tab shows
      // it is still loading, and a second switch cannot start mid-load.
      try {
        // A failed re-list keeps the locally confirmed list above: the switch
        // itself has landed and is not reported as failed.
        const [docs] = await Promise.all([
          listDocuments().catch(() => null),
          refreshSceneFromServer(),
          syncNotebook(),
        ])
        if (docs) set({ documents: docs })
      } finally {
        set({ switchingId: null })
      }
      // Open the fresh session NOW if the agent was connected before the
      // switch, rather than lazily on the next message: in between, the
      // header read "no session" and the AI chip went red as if the provider
      // had dropped, and the first prompt paid the whole startup on its own
      // clock. Never started when no session existed — a switch must not
      // spawn an agent the user had not connected. A failure here is the
      // same one the next prompt would meet, and it reports itself there.
      if (hadAgentSession) void establishAcpSession().catch(() => undefined)
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
      set((s) => ({ documents: s.documents.some((d) => d.id === doc.id) ? s.documents : [...s.documents, doc] }))
      return await get().switchTo(doc.id)
    } catch (err) {
      return { success: false, error: errorMessage(err) }
    }
  },

  rename: async (id: string, name: string) => {
    try {
      const updated = await renameDocument(id, name)
      set((state) => ({
        documents: state.documents.map((d) => (d.id === id ? updated : d)),
      }))
      return { success: true }
    } catch (err) {
      return { success: false, error: errorMessage(err) }
    }
  },
}))

// Building anything in the blank this browser started in makes it a real
// document: the next load starts a new blank instead of reopening it.
useSceneStore.subscribe((scene) => {
  if (scene.objectOrder.length === 0) return
  const active = useDocumentStore.getState().documents.find((d) => d.active)
  markStartDocumentUsed(startStorage(), active?.id ?? null)
})
