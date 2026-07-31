/**
 * REST client for the document registry — the top-level scope above
 * branches (see `api-server/src/documents.rs`).
 *
 * `POST /api/documents`          → register a new, empty document
 * `GET  /api/documents`          → list every registered document
 * `POST /api/documents/{id}/open` → make one the live document
 *
 * Opening a document resets the ENTIRE live server state (model, timeline,
 * blackboard, id mappings) to that document's own — every frontend store
 * that hydrates from the backend on mount (blackboard, sketches, model
 * tree, timeline panel, …) would otherwise keep serving the previous
 * document's cached data. Rather than hunt down and reset each store
 * individually — and risk missing one, which is a worse bug than a reload
 * — `File → New` reloads the page after the switch succeeds so every store
 * re-hydrates from the now-current document in one pass.
 */

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

export interface DocumentInfo {
  id: string
  name: string
  createdAt: number
  createdBy: string
  /** Whether this is the document currently loaded into the live model. */
  active: boolean
}

interface DocumentWire {
  id: string
  name: string
  created_at: number
  created_by: string
  active: boolean
}

function fromWire(w: DocumentWire): DocumentInfo {
  return {
    id: w.id,
    name: w.name,
    createdAt: w.created_at,
    createdBy: w.created_by,
    active: w.active,
  }
}

/** List every registered document, oldest first. */
export async function listDocuments(): Promise<DocumentInfo[]> {
  const resp = await fetch(`${API_BASE}/documents`, {
    headers: { Accept: 'application/json' },
  })
  if (!resp.ok) {
    throw new Error(`listDocuments: ${resp.status}`)
  }
  const data = (await resp.json()) as DocumentWire[]
  return data.map(fromWire)
}

/** Register a new, empty document. Does not open it. */
export async function createDocument(name?: string): Promise<DocumentInfo> {
  const resp = await fetch(`${API_BASE}/documents`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
    },
    body: JSON.stringify(name ? { name } : {}),
  })
  if (!resp.ok) {
    throw new Error(`createDocument: ${resp.status}`)
  }
  const data = (await resp.json()) as DocumentWire
  return fromWire(data)
}

/**
 * Make `id` the live document. Resolves once the backend confirms the
 * switch (the reset + replay); callers still need to refresh the UI —
 * `newDocument` below is the one-call helper that does both.
 */
export async function openDocument(id: string): Promise<void> {
  const resp = await fetch(`${API_BASE}/documents/${encodeURIComponent(id)}/open`, {
    method: 'POST',
  })
  if (!resp.ok) {
    throw new Error(`openDocument: ${resp.status}`)
  }
}

/**
 * `File → New`: create a fresh document, open it, then reload the page so
 * every frontend store re-hydrates against the empty document instead of
 * showing stale geometry/notes/timeline from whatever was open before.
 */
export async function newDocument(name?: string): Promise<void> {
  const doc = await createDocument(name)
  await openDocument(doc.id)
  window.location.reload()
}
