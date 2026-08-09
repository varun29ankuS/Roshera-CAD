/**
 * Persistent document tab strip (see vault
 * `Research/2026-07-31-ui-pass-spec.md` §3c).
 *
 * Documents used to be reachable only through `File → New` / `File →
 * Open Document` — right plumbing, wrong shape: a submenu suits rare
 * actions, and switching documents is something an engineer does
 * constantly. This renders documents as tabs: click to switch (in
 * place — see `stores/document-store.ts`), `×` or right-click → Close
 * to close the VIEW, `+` for New or to reopen a hidden document.
 *
 * ★ Source of truth for WHICH DOCUMENTS EXIST is always `GET /api/documents`
 * (via `useDocumentStore`), never localStorage. A document created outside
 * this UI — by an agent over MCP, another browser, a fresh profile — must
 * still be reachable. localStorage here remembers only which tabs a user
 * has explicitly CLOSED (a per-browser view preference); the active
 * document is always shown regardless of that set, since hiding the tab
 * you're standing in would make the document unreachable from the strip.
 *
 * Documents are durable: closing a tab only adds it to that closed-set.
 * The document itself stays registered on the backend and reappears under
 * the `+` menu's "Open existing" section — nothing here ever deletes
 * anything, and the wording below must never imply otherwise.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Plus, X } from 'lucide-react'
import { ContextMenu, ContextMenuItem } from '@/components/ui/context-menu'
import { cn } from '@/lib/utils'
import { type DocumentInfo } from '@/lib/documents-api'
import { useDocumentStore } from '@/stores/document-store'
import { useBlackboardStore } from '@/stores/blackboard-store'

const CLOSED_TABS_KEY = 'roshera.tabs.closed.v1'

function readClosedIds(): Set<string> {
  try {
    const raw = window.localStorage.getItem(CLOSED_TABS_KEY)
    if (!raw) return new Set()
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? new Set(parsed.filter((x): x is string => typeof x === 'string')) : new Set()
  } catch {
    return new Set()
  }
}

function writeClosedIds(ids: Set<string>) {
  try {
    window.localStorage.setItem(CLOSED_TABS_KEY, JSON.stringify(Array.from(ids)))
  } catch {
    // Quota / private-mode failure — the strip still works for this
    // session, it just won't remember closed tabs across a reload.
  }
}

function reportFailure(action: string, error?: string) {
  console.error(`[DocumentTabs] ${action} failed:`, error)
  useBlackboardStore.getState().addLine(`${action} failed: ${error ?? 'backend unreachable'}`, 'system')
}

const SHORT_MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** "Aug 8, 09:12" — the row's date discriminator. Rows sharing a name
 *  (four "Untitled" documents on this server today) are otherwise
 *  indistinguishable; this is the human-legible tiebreaker, with the id
 *  fragment below it as the last-resort one. */
function formatShortDate(epochMs: number): string {
  const d = new Date(epochMs)
  const hh = String(d.getHours()).padStart(2, '0')
  const mm = String(d.getMinutes()).padStart(2, '0')
  return `${SHORT_MONTHS[d.getMonth()]} ${d.getDate()}, ${hh}:${mm}`
}

/** First 8 chars of the document id, monospaced — the last-resort
 *  discriminator when two documents share both a name and a created-date
 *  display value. */
function idFragment(id: string): string {
  return id.slice(0, 8)
}

interface ContextMenuState {
  doc: DocumentInfo
  x: number
  y: number
}

export function DocumentTabs() {
  const documents = useDocumentStore((s) => s.documents)
  const switchingId = useDocumentStore((s) => s.switchingId)
  const refresh = useDocumentStore((s) => s.refresh)
  const switchTo = useDocumentStore((s) => s.switchTo)
  const createAndSwitch = useDocumentStore((s) => s.createAndSwitch)

  const [closedIds, setClosedIdsState] = useState<Set<string>>(readClosedIds)
  const [addMenuOpen, setAddMenuOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)
  const addMenuRef = useRef<HTMLDivElement | null>(null)

  const setClosedIds = useCallback((updater: (prev: Set<string>) => Set<string>) => {
    setClosedIdsState((prev) => {
      const next = updater(prev)
      writeClosedIds(next)
      return next
    })
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    if (!addMenuOpen) return
    const onPointer = (e: MouseEvent) => {
      if (addMenuRef.current && !addMenuRef.current.contains(e.target as Node)) {
        setAddMenuOpen(false)
      }
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setAddMenuOpen(false)
    }
    document.addEventListener('mousedown', onPointer)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onPointer)
      document.removeEventListener('keydown', onKey)
    }
  }, [addMenuOpen])

  // Visible tabs = every document EXCEPT ones the user explicitly closed —
  // and the active document is ALWAYS visible, closed-set or not, so the
  // document you're standing in can never vanish from the strip.
  const tabs = useMemo(
    () => documents.filter((d) => d.active || !closedIds.has(d.id)),
    [documents, closedIds],
  )
  const hiddenDocuments = useMemo(
    () => documents.filter((d) => !d.active && closedIds.has(d.id)),
    [documents, closedIds],
  )

  const handleSwitch = useCallback(
    async (doc: DocumentInfo) => {
      if (doc.active || switchingId) return
      const result = await switchTo(doc.id)
      if (!result.success) reportFailure(`Switching to ${doc.name}`, result.error)
    },
    [switchingId, switchTo],
  )

  /** Close `doc`'s tab. If it's the active one, switch to a neighbour
   *  FIRST (never optimistic — the tab only disappears once that switch
   *  is confirmed), then close. If there is no neighbour to fall back to,
   *  keep the tab — never leave zero tabs. */
  const closeTab = useCallback(
    async (doc: DocumentInfo) => {
      if (!doc.active) {
        setClosedIds((prev) => new Set(prev).add(doc.id))
        return
      }
      const idx = tabs.findIndex((d) => d.id === doc.id)
      const neighbor = tabs[idx - 1] ?? tabs[idx + 1]
      if (!neighbor) return // only tab open — nothing to fall back to
      const result = await switchTo(neighbor.id)
      if (!result.success) {
        reportFailure(`Closing ${doc.name} (switching to ${neighbor.name})`, result.error)
        return
      }
      setClosedIds((prev) => new Set(prev).add(doc.id))
    },
    [tabs, switchTo, setClosedIds],
  )

  const handleCloseClick = useCallback(
    (e: React.MouseEvent, doc: DocumentInfo) => {
      e.stopPropagation()
      void closeTab(doc)
    },
    [closeTab],
  )

  /** Close every tab except `keep`. If `keep` isn't active, switch to it
   *  first (confirmed) so the surviving tab is never one you've left. */
  const closeOthers = useCallback(
    async (keep: DocumentInfo) => {
      if (!keep.active) {
        const result = await switchTo(keep.id)
        if (!result.success) {
          reportFailure(`Switching to ${keep.name}`, result.error)
          return
        }
      }
      setClosedIds(() => new Set(documents.filter((d) => d.id !== keep.id).map((d) => d.id)))
    },
    [documents, switchTo, setClosedIds],
  )

  const handleNew = useCallback(async () => {
    setAddMenuOpen(false)
    const result = await createAndSwitch()
    if (!result.success) reportFailure('Creating a new document', result.error)
  }, [createAndSwitch])

  const handleOpenExisting = useCallback(
    async (doc: DocumentInfo) => {
      setAddMenuOpen(false)
      setClosedIds((prev) => {
        if (!prev.has(doc.id)) return prev
        const next = new Set(prev)
        next.delete(doc.id)
        return next
      })
      if (!doc.active) {
        const result = await switchTo(doc.id)
        if (!result.success) reportFailure(`Opening ${doc.name}`, result.error)
      }
    },
    [switchTo, setClosedIds],
  )

  const handleContextMenu = useCallback((e: React.MouseEvent, doc: DocumentInfo) => {
    e.preventDefault()
    setContextMenu({ doc, x: e.clientX, y: e.clientY })
  }, [])

  // Nothing to show before the first successful fetch — a bare `+` with
  // no context reads as broken chrome rather than "loading".
  if (documents.length === 0) return null

  return (
    <div className="flex items-center gap-0.5 h-11 px-1.5 border-b border-border bg-card/60 shrink-0">
      {/* Scrollable region for the tabs ONLY. `overflow-x-auto` on this
          element implicitly computes `overflow-y: auto` too (a CSS
          coupling gotcha — you cannot set only one axis to non-visible),
          which CLIPS any `position: absolute` popover anchored inside it
          the instant it extends below this row's height — the `+`
          menu was silently invisible for exactly this reason (confirmed
          live: correct DOM, z-index, computed styles, but
          `elementFromPoint` hit the panel behind it). The `+` button and
          its dropdown below live OUTSIDE this scroller so they're never
          clipped. */}
      <div className="flex items-center gap-0.5 h-full overflow-x-auto min-w-0" role="tablist" aria-label="Open documents">
      {tabs.map((doc) => {
        const isSwitchingHere = switchingId === doc.id
        const onlyTab = tabs.length === 1
        return (
          <div
            key={doc.id}
            role="tab"
            aria-selected={doc.active}
            onClick={() => void handleSwitch(doc)}
            onContextMenu={(e) => handleContextMenu(e, doc)}
            title={`${doc.name} — created ${formatShortDate(doc.createdAt)} · ${doc.id} — its own Blackboard notes, timeline, and model. Switching loads that document's notebook, not this one's.`}
            className={cn(
              'group relative flex items-center gap-1.5 py-1 pl-2.5 pr-1 rounded-t text-[12px] shrink-0 max-w-[180px] cursor-pointer transition-colors border-b-2',
              doc.active
                ? 'bg-primary/10 text-foreground font-semibold border-primary'
                : 'text-muted-foreground hover:text-foreground hover:bg-accent/30 border-transparent',
              switchingId && !isSwitchingHere && 'pointer-events-none opacity-60',
            )}
          >
            {doc.active && !isSwitchingHere && (
              <span
                aria-hidden
                className="inline-block w-2 h-2 rounded-full bg-primary ring-2 ring-primary/25 shrink-0"
              />
            )}
            {isSwitchingHere && (
              <span
                aria-hidden
                className="inline-block w-2 h-2 rounded-full bg-primary shrink-0 animate-pulse"
              />
            )}
            <span className="min-w-0 flex flex-col justify-center leading-tight">
              <span className="truncate">{doc.name}</span>
              <span className="truncate text-[9px] font-normal text-muted-foreground/60 tabular-nums">
                {formatShortDate(doc.createdAt)}
                <span className="font-mono"> · {idFragment(doc.id)}</span>
              </span>
            </span>
            <button
              type="button"
              onClick={(e) => handleCloseClick(e, doc)}
              title={
                onlyTab && doc.active
                  ? 'Nothing to switch to — open another document first'
                  : doc.active
                    ? 'Close — switches to a neighbouring tab first. The document stays open — reopen it from the + menu'
                    : "Close this tab. The document stays open — reopen it from the + menu"
              }
              aria-label={`Close ${doc.name} tab`}
              disabled={onlyTab && doc.active}
              className={cn(
                'shrink-0 rounded p-0.5 transition-opacity',
                onlyTab && doc.active
                  ? 'opacity-0 pointer-events-none'
                  : 'opacity-0 group-hover:opacity-100 text-muted-foreground/60 hover:text-foreground hover:bg-accent/50',
              )}
            >
              <X size={11} />
            </button>
          </div>
        )
      })}
      </div>

      <div ref={addMenuRef} className="relative shrink-0">
        <button
          type="button"
          onClick={() => setAddMenuOpen((v) => !v)}
          title="New document, or reopen one that's closed"
          aria-label="New or open document"
          aria-expanded={addMenuOpen}
          className="flex items-center justify-center h-6 w-6 rounded text-muted-foreground hover:text-foreground hover:bg-accent/30 transition-colors"
        >
          <Plus size={14} />
        </button>
        {addMenuOpen && (
          <div className="absolute left-0 top-full mt-1 z-[100] min-w-[200px] rounded-md border border-border bg-card shadow-lg py-1 text-[12px]">
            <button
              type="button"
              onClick={() => void handleNew()}
              className="w-full text-left px-3 py-1.5 hover:bg-accent/40 text-foreground/90"
            >
              + New document
            </button>
            {hiddenDocuments.length > 0 && (
              <>
                <div className="my-1 border-t border-border/40" />
                <div className="px-3 py-1 text-[10px] uppercase tracking-wide text-muted-foreground/60">
                  Open existing
                </div>
                {hiddenDocuments.map((doc) => (
                  <button
                    key={doc.id}
                    type="button"
                    onClick={() => void handleOpenExisting(doc)}
                    title={`${doc.name} — created ${formatShortDate(doc.createdAt)} · ${doc.id}`}
                    className="w-full flex flex-col min-w-0 text-left px-3 py-1.5 hover:bg-accent/40"
                  >
                    <span className="truncate text-foreground/90">{doc.name}</span>
                    <span className="truncate text-[10px] text-muted-foreground/60 tabular-nums">
                      {formatShortDate(doc.createdAt)}
                      <span className="font-mono"> · {idFragment(doc.id)}</span>
                    </span>
                  </button>
                ))}
              </>
            )}
          </div>
        )}
      </div>

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          aria-label="Document tab actions"
        >
          <ContextMenuItem
            disabled={tabs.length === 1 && contextMenu.doc.active}
            title={
              tabs.length === 1 && contextMenu.doc.active
                ? 'Nothing to switch to — open another document first'
                : 'Close this tab. The document stays open — reopen it from the + menu'
            }
            onClick={() => {
              void closeTab(contextMenu.doc)
              setContextMenu(null)
            }}
          >
            Close
          </ContextMenuItem>
          <ContextMenuItem
            disabled={tabs.length === 1}
            title={tabs.length === 1 ? 'No other tabs to close' : undefined}
            onClick={() => {
              void closeOthers(contextMenu.doc)
              setContextMenu(null)
            }}
          >
            Close others
          </ContextMenuItem>
          {/* No Rename: the backend document registry (`documents.rs`) has
              no PATCH/rename route today — an item that can't do anything
              is worse than no item. */}
        </ContextMenu>
      )}
    </div>
  )
}
