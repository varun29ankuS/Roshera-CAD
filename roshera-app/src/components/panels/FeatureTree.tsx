/**
 * Feature Tree — read-only operation-graph body.
 *
 * Pure renderer over `GET /api/feature-tree/{branch_id}`. The kernel
 * is the authoritative source of the hierarchy: parent edges,
 * inputs/outputs lineage, and per-kind running indices are all
 * computed in `handlers/timeline.rs::build_feature_tree`. The
 * frontend ships zero derivation logic — same architectural
 * contract as the rest of the codebase (backend-driven, frontend is
 * a thin display layer).
 *
 * This module exports `FeatureTreeBody` only — the data-fetch + tree
 * render with no panel chrome (no header chip, no ScrollArea
 * wrapper). The consolidated browser (`ModelTree`) hosts it inside
 * its own chrome behind the `features` mode toggle so the user
 * sees one panel, one mental location, with a single tab switch to
 * flip between assembly and lineage views.
 *
 * Re-fetches on a 150 ms debounce when the WebSocket emits an
 * `ObjectCreated` / `ObjectUpdated` / `ObjectDeleted` / `SceneSync`
 * / `GeometryUpdate` frame. Slice 2 will replace this with a
 * dedicated `TimelineEventRecorded` push and add rollback / rename /
 * hide / delete affordances plus a branch selector.
 */

import { useState, useEffect, useCallback, useRef } from 'react'
import { useWSStore } from '@/stores/ws-store'
import { wsClient } from '@/lib/ws-client'
import { cn } from '@/lib/utils'
import {
  type EventSummary,
  symbolForOperation,
  shortLabel,
  relativeTime,
  formatTimestamp,
  formatAuthor,
  authorKind,
  authorGlyph,
  authorTextClass,
} from '@/lib/timeline-events'

const API_BASE = import.meta.env.VITE_API_URL || ''

// ─── Wire shape (matches handlers/timeline.rs::FeatureNode) ─────────

interface FeatureNode {
  event: EventSummary
  inputs: string[]
  outputs: string[]
  parent_event_id: string | null
  /** Per-kind running index (1-based) computed kernel-side. */
  kind_index: number
  children: FeatureNode[]
}

// ─── Tree row ───────────────────────────────────────────────────────

function FeatureRow({
  node,
  depth,
  collapsed,
  toggleCollapsed,
  now,
}: {
  node: FeatureNode
  depth: number
  collapsed: Set<string>
  toggleCollapsed: (eventId: string) => void
  /** Re-render anchor so relative timestamps tick. */
  now: number
}) {
  const event = node.event
  const symbol = symbolForOperation(event.operation_type)
  const label = shortLabel(event.operation_type)
  const kind = authorKind(event.author, event.author_kind)
  const glyph = authorGlyph(kind)
  void now
  const rel = relativeTime(event.timestamp)
  const symbolColor = authorTextClass(kind, false)
  const hasChildren = node.children.length > 0
  const isOpen = !collapsed.has(event.id)
  const indentPx = depth * 12
  const arm = hasChildren ? (isOpen ? '▾' : '▸') : '─'

  return (
    <div>
      <div
        className={cn(
          'flex items-center cursor-default select-none transition-colors group font-mono text-[13px] leading-snug',
          'text-foreground/70 hover:bg-accent/50 hover:text-foreground',
        )}
        style={{ paddingLeft: indentPx }}
        title={`${event.operation_type}\n${formatAuthor(event.author)} · ${formatTimestamp(event.timestamp)} · #${event.sequence_number}`}
        // Right-click is a slice-2 hook (rollback / rename); preventing
        // the browser context menu here is the only thing slice 1
        // does, so users discover the affordance is "coming" without
        // seeing the OS menu pop up over our future actions.
        onContextMenu={(e) => e.preventDefault()}
      >
        <span className="whitespace-pre shrink-0">
          {hasChildren ? (
            <button
              type="button"
              onClick={() => toggleCollapsed(event.id)}
              className="text-foreground/70 hover:text-foreground transition-colors"
              aria-label={isOpen ? 'Collapse' : 'Expand'}
              aria-expanded={isOpen}
            >
              {arm}
            </button>
          ) : (
            <span className="text-muted-foreground/50">{arm}</span>
          )}
          <span className="text-muted-foreground/50"> </span>
        </span>
        <span className={cn('shrink-0 mr-1', symbolColor)}>{symbol}</span>
        <span className="truncate flex-1">
          {label}-{node.kind_index}
        </span>
        <div className="flex items-center gap-0.5 px-1 text-[10px] leading-tight shrink-0">
          <span className={authorTextClass(kind, false)}>{glyph}</span>
          <span className="text-muted-foreground/60">{rel}</span>
        </div>
      </div>
      {hasChildren && isOpen && (
        <div>
          {node.children.map((child) => (
            <FeatureRow
              key={child.event.id}
              node={child}
              depth={depth + 1}
              collapsed={collapsed}
              toggleCollapsed={toggleCollapsed}
              now={now}
            />
          ))}
        </div>
      )}
    </div>
  )
}

// ─── Body (data fetch + tree render only, no panel chrome) ──────────

interface FeatureTreeBodyProps {
  /**
   * Branch whose timeline to render. Slice 1 hardcodes `'main'` at
   * the call site; slice 2 will add a branch selector.
   */
  branchId?: string
}

export function FeatureTreeBody({ branchId = 'main' }: FeatureTreeBodyProps) {
  const wsStatus = useWSStore((s) => s.status)
  const [roots, setRoots] = useState<FeatureNode[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [now, setNow] = useState(() => Date.now())

  // Track the latest in-flight fetch so the cleanup path in
  // `useEffect` can abort it on unmount or branch change — prevents
  // a stale response from overwriting fresh state.
  const abortRef = useRef<AbortController | null>(null)
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const fetchTree = useCallback(async () => {
    abortRef.current?.abort()
    const controller = new AbortController()
    abortRef.current = controller
    setLoading(true)
    try {
      const resp = await fetch(
        `${API_BASE}/api/feature-tree/${branchId}`,
        { signal: controller.signal },
      )
      if (!resp.ok) {
        setError(`HTTP ${resp.status}`)
        setRoots([])
        return
      }
      const data: unknown = await resp.json()
      const next: FeatureNode[] = Array.isArray(data)
        ? (data as FeatureNode[])
        : []
      setRoots(next)
      setError(null)
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') return
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
      setRoots([])
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null
        setLoading(false)
      }
    }
  }, [branchId])

  // Initial fetch + re-fetch when the WebSocket reconnects.
  useEffect(() => {
    fetchTree()
    return () => {
      abortRef.current?.abort()
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current)
        debounceRef.current = null
      }
    }
  }, [fetchTree])

  useEffect(() => {
    if (wsStatus === 'connected') {
      fetchTree()
    }
  }, [wsStatus, fetchTree])

  // Re-fetch on WS pushes that imply a new timeline event landed.
  // Debounced so a burst (e.g., SceneSync replaying many objects) only
  // triggers one round-trip. Slice 2 will replace this with a
  // dedicated `TimelineEventRecorded` push from the backend.
  useEffect(() => {
    const unsubscribe = wsClient.onMessage((msg) => {
      switch (msg.type) {
        case 'ObjectCreated':
        case 'ObjectUpdated':
        case 'ObjectDeleted':
        case 'SceneSync':
        case 'GeometryUpdate':
          if (debounceRef.current !== null) {
            clearTimeout(debounceRef.current)
          }
          debounceRef.current = setTimeout(() => {
            debounceRef.current = null
            fetchTree()
          }, 150)
          break
        default:
          break
      }
    })
    return () => {
      unsubscribe()
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current)
        debounceRef.current = null
      }
    }
  }, [fetchTree])

  // Tick relative-time labels so "5s" → "6s" without re-fetching.
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  const toggleCollapsed = useCallback((eventId: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(eventId)) {
        next.delete(eventId)
      } else {
        next.add(eventId)
      }
      return next
    })
  }, [])

  if (error) {
    return (
      <div className="px-3 py-2 text-[11px] text-muted-foreground/80 font-mono flex items-center gap-2">
        <span className="text-destructive/80 truncate flex-1">
          Failed to load history: {error}
        </span>
        <button
          type="button"
          onClick={() => fetchTree()}
          className="text-foreground/70 hover:text-foreground transition-colors underline underline-offset-2"
          disabled={loading}
        >
          retry
        </button>
      </div>
    )
  }

  if (roots.length === 0) {
    return (
      <div className="p-3 text-[13px] text-muted-foreground/60 text-center font-mono">
        {loading
          ? '… loading'
          : 'No features yet — create a primitive to start.'}
      </div>
    )
  }

  return (
    <div className="py-1 px-1">
      {roots.map((root) => (
        <FeatureRow
          key={root.event.id}
          node={root}
          depth={0}
          collapsed={collapsed}
          toggleCollapsed={toggleCollapsed}
          now={now}
        />
      ))}
    </div>
  )
}

