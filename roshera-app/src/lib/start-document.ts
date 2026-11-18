/**
 * Which document the app starts in.
 *
 * Varun (2026-11-18): "it should start with a new document everytime ...
 * unless someone saves something and opens an old one". It used to reopen
 * the last document on every load, and opening a document replays and
 * re-certifies its whole history -- minutes on one with STEP imports. Now a
 * load starts in a fresh document: the blank one this browser made last time
 * if nothing was built in it (so blanks do not pile up), else a new one.
 * Old documents are opened on purpose, from the tab strip's + menu.
 */

/** localStorage key: the blank document this browser started in, and
 *  whether anything has been built in it since. */
export const START_DOCUMENT_KEY = 'roshera.startDocument'

export interface StartDocument {
  id: string
  /** Something was built in it: it is a real document now, not a blank. */
  used: boolean
}

export type StartPlan = { kind: 'stay' } | { kind: 'open'; id: string } | { kind: 'create' }

type KeyValueStore = Pick<Storage, 'getItem' | 'setItem'>

/** Where a load should start, given the server's documents and the blank
 *  this browser remembers. */
export function planStartDocument(
  docs: readonly { id: string; active: boolean }[],
  remembered: StartDocument | null,
): StartPlan {
  if (remembered && !remembered.used) {
    const doc = docs.find((d) => d.id === remembered.id)
    if (doc) return doc.active ? { kind: 'stay' } : { kind: 'open', id: doc.id }
  }
  return { kind: 'create' }
}

export function readStartDocument(storage: KeyValueStore): StartDocument | null {
  try {
    const raw = storage.getItem(START_DOCUMENT_KEY)
    if (!raw) return null
    const v = JSON.parse(raw) as Partial<StartDocument>
    return typeof v.id === 'string' && typeof v.used === 'boolean' ? { id: v.id, used: v.used } : null
  } catch {
    return null
  }
}

export function writeStartDocument(storage: KeyValueStore, doc: StartDocument): void {
  try {
    storage.setItem(START_DOCUMENT_KEY, JSON.stringify(doc))
  } catch {
    /* storage blocked: the next load makes another blank */
  }
}

/** Something was built in `activeId`: if it is the remembered blank, it is a
 *  real document now and the next load starts a new one. */
export function markStartDocumentUsed(storage: KeyValueStore, activeId: string | null): void {
  const s = readStartDocument(storage)
  if (s && !s.used && activeId === s.id) writeStartDocument(storage, { id: s.id, used: true })
}
