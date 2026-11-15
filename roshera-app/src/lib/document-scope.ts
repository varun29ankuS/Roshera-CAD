/**
 * Which document the client's history caches belong to.
 *
 * A document's history is numbered from its own step 0, so two documents both
 * have a "step 3 on main". Every client-side cache of history -- the film
 * strip's pictures, the meshes of a scrubbed step -- was keyed `branch:step`
 * only, and after a tab switch the strip showed the PREVIOUS document's
 * pictures under the new document's steps (Varun 2026-11-15: "timeline
 * doesn't update with tab").
 *
 * The rule here: a history cache is scoped to ONE document. Entering another
 * document empties every scoped cache, and an answer fetched for the old
 * document that arrives after the switch is dropped instead of kept.
 *
 * `document-store.ts` calls `enterDocument` the moment the backend confirms a
 * switch (and with the active document when the list first loads). This
 * module has no imports so the stores and components that cache history can
 * all depend on it without an import cycle through the document store.
 */

/** A snapshot of the scope a request was started in. */
export interface ScopeToken {
  readonly document: string | null
  readonly generation: number
}

type Listener = (documentId: string) => void

let current: string | null = null
let generation = 0
const listeners = new Set<Listener>()

/** The document the history caches currently belong to (null before the
 *  document list first loads). */
export function currentDocument(): string | null {
  return current
}

/** Capture the current scope, to check with `isCurrentScope` when a request
 *  started now comes back. */
export function scopeToken(): ScopeToken {
  return { document: current, generation }
}

/** True while no document change has happened since `token` was taken. */
export function isCurrentScope(token: ScopeToken): boolean {
  return token.generation === generation
}

/**
 * Make `documentId` the scope. A different document -- or the same one
 * re-opened, whose history the backend has just replayed -- starts a new
 * scope: every subscriber hears it once and every scoped cache empties.
 * Entering the document already in scope is a no-op.
 */
export function enterDocument(documentId: string, opts: { reopened?: boolean } = {}): void {
  if (documentId === current && !opts.reopened) return
  current = documentId
  generation += 1
  for (const listener of [...listeners]) listener(documentId)
}

/** Hear every document change. Returns the unsubscribe. */
export function onDocumentChange(listener: Listener): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/**
 * A bounded least-recently-used cache whose entries belong to the document in
 * scope. A document change empties it (handing each dropped value to
 * `onDrop`, so an object URL can be revoked), and `setIfCurrent` refuses a
 * value fetched in a scope that has since ended.
 */
export class DocumentScopedCache<V> {
  private readonly entries = new Map<string, V>()
  private readonly max: number
  private readonly onDrop: ((value: V) => void) | undefined

  constructor(max: number, onDrop?: (value: V) => void) {
    this.max = max
    this.onDrop = onDrop
    // Module-lifetime caches: the subscription lives as long as the page.
    onDocumentChange(() => this.clear())
  }

  get size(): number {
    return this.entries.size
  }

  has(key: string): boolean {
    return this.entries.has(key)
  }

  /** The value for `key`, refreshing it as the most recently used. */
  get(key: string): V | undefined {
    const value = this.entries.get(key)
    if (value === undefined) return undefined
    this.entries.delete(key)
    this.entries.set(key, value)
    return value
  }

  set(key: string, value: V): void {
    const previous = this.entries.get(key)
    if (previous !== undefined && previous !== value) this.onDrop?.(previous)
    this.entries.delete(key)
    this.entries.set(key, value)
    while (this.entries.size > this.max) {
      const oldest = this.entries.keys().next()
      if (oldest.done) break
      const dropped = this.entries.get(oldest.value)
      this.entries.delete(oldest.value)
      if (dropped !== undefined) this.onDrop?.(dropped)
    }
  }

  /** Keep `value` only if no document change happened since `token`. Returns
   *  whether it was kept; a refused value is handed to `onDrop`. */
  setIfCurrent(token: ScopeToken, key: string, value: V): boolean {
    if (!isCurrentScope(token)) {
      this.onDrop?.(value)
      return false
    }
    this.set(key, value)
    return true
  }

  clear(): void {
    const dropped = [...this.entries.values()]
    this.entries.clear()
    if (this.onDrop) for (const value of dropped) this.onDrop(value)
  }
}
