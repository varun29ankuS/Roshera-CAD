import { Fragment, useState, useEffect, useCallback } from 'react'
import { useWSStore } from '@/stores/ws-store'
import { cn } from '@/lib/utils'

// ─── Preview mode (opt-in via ?preview URL param) ───────────────────

function isPreviewMode(): boolean {
  if (typeof window === 'undefined') return false
  return new URLSearchParams(window.location.search).has('preview')
}

const MOCK_EVENTS: EventSummary[] = (() => {
  const now = Date.now()
  return [
    { id: 'm1', sequence_number: 1, timestamp: new Date(now - 240_000).toISOString(),
      operation_type: 'CreatePrimitive { shape_type: Box }',
      author: 'User { id: 1, name: Varun }' },
    { id: 'm2', sequence_number: 2, timestamp: new Date(now - 200_000).toISOString(),
      operation_type: 'CreatePrimitive { shape_type: Sphere }',
      author: 'AIAgent { provider: Claude }' },
    { id: 'm3', sequence_number: 3, timestamp: new Date(now - 150_000).toISOString(),
      operation_type: 'BooleanUnion',
      author: 'User { id: 1, name: Varun }' },
    { id: 'm4', sequence_number: 4, timestamp: new Date(now - 90_000).toISOString(),
      operation_type: 'CreatePrimitive { shape_type: Cylinder }',
      author: 'AIAgent { provider: Claude }' },
    { id: 'm5', sequence_number: 5, timestamp: new Date(now - 45_000).toISOString(),
      operation_type: 'BooleanDifference',
      author: 'AIAgent { provider: Claude }' },
    { id: 'm6', sequence_number: 6, timestamp: new Date(now - 25_000).toISOString(),
      operation_type: 'FilletEdges',
      author: 'User { id: 1, name: Varun }' },
    { id: 'm7', sequence_number: 7, timestamp: new Date(now - 8_000).toISOString(),
      operation_type: 'ExtrudeFace',
      author: 'AIAgent { provider: Claude }' },
    { id: 'm8', sequence_number: 8, timestamp: new Date(now - 2_000).toISOString(),
      operation_type: 'ChamferEdges',
      author: 'User { id: 1, name: Varun }' },
  ]
})()

// ─── Types matching backend GET /api/timeline/history/{branch_id} ──

interface EventSummary {
  id: string
  sequence_number: number
  timestamp: string // ISO 8601
  operation_type: string // clean kernel kind, e.g. "create_box_3d"
  /** Full structured operation as tagged JSON (backend-emitted). */
  operation?: unknown
  author: string // clean display name
  /** Backend-emitted classification: "user" | "ai" | "system". */
  author_kind?: AuthorKind
}

// ─── Kernel kind → symbol/label (terminal aesthetic) ────────────────
//
// `operation_type` is the clean kernel command name emitted by the
// timeline-engine bridge — e.g. "create_box_3d", "extrude_face",
// "boolean_operation", "fillet_edges". The legacy debug-string format
// (`Generic { command_type: "create_box_3d", ... }`) is still tolerated
// as a fallback so old timelines on disk render correctly.

function normalizeKind(op: string): string {
  // Legacy: "Generic { command_type: \"create_box_3d\", ... }"
  const inner = op.match(/command_type:\s*"?(\w+)"?/)
  if (inner) return inner[1]
  // Legacy: "CreatePrimitive { shape_type: Box, ... }" → "createprimitive_box"
  const shape = op.match(/shape_type:\s*(\w+)/i)
  if (shape && /^createprimitive/i.test(op)) {
    return `create_${shape[1].toLowerCase()}_3d`
  }
  return op
}

function symbolForOperation(op: string): string {
  const k = normalizeKind(op).toLowerCase()
  if (k.startsWith('create_box') || k === 'create_cube_3d') return '▣'
  if (k.startsWith('create_sphere')) return '◯'
  if (k.startsWith('create_cylinder')) return '⊟'
  if (k.startsWith('create_cone')) return '△'
  if (k.startsWith('create_torus')) return '◎'
  if (k.startsWith('create_point')) return '·'
  if (k.startsWith('create_line')) return '─'
  if (k.startsWith('create_circle')) return '○'
  if (k.startsWith('create_rectangle')) return '▭'
  if (k.startsWith('plane_')) return '▱'
  if (k.startsWith('extrude')) return '↑'
  if (k.startsWith('revolve')) return '↻'
  if (k.startsWith('sweep')) return '↝'
  if (k.startsWith('loft')) return '≋'
  if (k.startsWith('fillet')) return '◜'
  if (k.startsWith('chamfer')) return '⬡'
  if (k.startsWith('transform')) return '⇆'
  if (k.startsWith('boolean')) return '⊕'
  if (k.includes('union')) return '∪'
  if (k.includes('intersection')) return '∩'
  if (k.includes('difference') || k.includes('subtract')) return '⊖'
  if (k.includes('delete')) return '✕'
  if (k.includes('update')) return '✎'
  if (k.startsWith('create')) return '▣'
  return '◆'
}

function shortLabel(op: string): string {
  const k = normalizeKind(op).toLowerCase()
  if (k.startsWith('create_box') || k === 'create_cube_3d') return 'Box'
  if (k.startsWith('create_sphere')) return 'Sph'
  if (k.startsWith('create_cylinder')) return 'Cyl'
  if (k.startsWith('create_cone')) return 'Con'
  if (k.startsWith('create_torus')) return 'Tor'
  if (k.startsWith('create_point')) return 'Pt'
  if (k.startsWith('create_line')) return 'Lin'
  if (k.startsWith('create_circle')) return 'Cir'
  if (k.startsWith('create_rectangle')) return 'Rec'
  if (k.startsWith('plane_')) return 'Pln'
  if (k.startsWith('extrude')) return 'Ext'
  if (k.startsWith('revolve')) return 'Rev'
  if (k.startsWith('sweep')) return 'Swp'
  if (k.startsWith('loft')) return 'Lft'
  if (k.startsWith('fillet')) return 'Fil'
  if (k.startsWith('chamfer')) return 'Cha'
  if (k.startsWith('transform')) return 'Tr'
  if (k.startsWith('boolean')) return 'Bool'
  if (k.includes('union')) return 'Un'
  if (k.includes('intersection')) return 'Int'
  if (k.includes('difference')) return 'Df'
  if (k.includes('delete')) return 'Del'
  if (k.includes('update')) return 'Upd'
  const match = k.match(/^(\w+)/)
  return match ? match[1].slice(0, 4) : '?'
}

function formatTimestamp(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return ts
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

// Relative human time: "now", "5s", "3m", "2h", "4d"
function relativeTime(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return '?'
  const deltaMs = Date.now() - d.getTime()
  if (deltaMs < 2000) return 'now'
  const s = Math.floor(deltaMs / 1000)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h`
  const day = Math.floor(h / 24)
  return `${day}d`
}

function formatAuthor(author: string): string {
  // New backend already emits clean strings ("Varun", "Claude", "System").
  // Legacy Debug strings ("User { id: 1, name: Varun }") still parsed as
  // a fallback so persisted timelines render the right name.
  if (!author) return '?'
  const nameMatch = author.match(/name:\s*(\w+)/)
  if (nameMatch) return nameMatch[1]
  return author
}

type AuthorKind = 'user' | 'ai' | 'system'

function authorKind(author: string, hint?: AuthorKind): AuthorKind {
  // Prefer the backend-supplied classification when present.
  if (hint === 'user' || hint === 'ai' || hint === 'system') return hint
  if (author === 'System') return 'system'
  if (author.includes('AIAgent') || author.includes('AI')) return 'ai'
  if (author.includes('User') || author.includes('name:')) return 'user'
  return 'system'
}

function authorGlyph(kind: AuthorKind): string {
  switch (kind) {
    case 'user': return 'Ⓤ'
    case 'ai': return 'Ⓒ'
    case 'system': return '§'
  }
}

// Tailwind class for author-tinted text. User = primary, AI = amber, System = muted.
function authorTextClass(kind: AuthorKind, isLatest: boolean): string {
  const base = (() => {
    switch (kind) {
      case 'user': return 'text-primary'
      case 'ai': return 'text-amber-400'
      case 'system': return 'text-muted-foreground'
    }
  })()
  return isLatest ? base : `${base}/70`
}

// ─── Event node (3-line column) ─────────────────────────────────────

function EventNode({
  event,
  isLatest,
  now,
}: {
  event: EventSummary
  isLatest: boolean
  now: number // re-render anchor for relative time
}) {
  const symbol = symbolForOperation(event.operation_type)
  const label = shortLabel(event.operation_type)
  const kind = authorKind(event.author, event.author_kind)
  const glyph = authorGlyph(kind)
  // `now` participates so this re-renders when the parent ticks.
  void now
  const rel = relativeTime(event.timestamp)
  const symbolColor = authorTextClass(kind, isLatest)

  return (
    <div
      className="flex flex-col items-center px-1 cursor-default select-none group"
      title={`${event.operation_type}\n${formatAuthor(event.author)} · ${formatTimestamp(event.timestamp)} · #${event.sequence_number}`}
    >
      <div className={cn('text-base leading-none transition-colors', symbolColor)}>
        {symbol}
      </div>
      <div
        className={cn(
          'text-[11px] leading-tight mt-0.5 min-w-[3ch] text-center transition-colors',
          isLatest ? 'text-foreground/90' : 'text-foreground/50 group-hover:text-foreground/80',
        )}
      >
        {label}
      </div>
      <div className="flex items-center gap-0.5 text-[10px] leading-tight mt-0.5">
        <span className={authorTextClass(kind, isLatest)}>{glyph}</span>
        <span className={isLatest ? 'text-foreground/70' : 'text-muted-foreground/60'}>{rel}</span>
      </div>
    </div>
  )
}

// ─── Connector (── glyph aligned to symbol baseline) ────────────────

function Connector() {
  return (
    <span className="text-muted-foreground/40 text-base leading-none self-start pt-[1px]">
      ──
    </span>
  )
}

// ─── Header glyph button ────────────────────────────────────────────

function HeaderButton({
  children,
  onClick,
  title,
  ariaLabel,
}: {
  children: React.ReactNode
  onClick: () => void
  title: string
  ariaLabel: string
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={ariaLabel}
      className="px-1.5 py-0.5 rounded text-foreground/70 hover:text-foreground hover:bg-accent/40 transition-colors"
    >
      {children}
    </button>
  )
}

// ─── Main strip ─────────────────────────────────────────────────────

export function Timeline() {
  const previewMode = isPreviewMode()
  const [events, setEvents] = useState<EventSummary[]>(previewMode ? MOCK_EVENTS : [])
  const [loading, setLoading] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const wsStatus = useWSStore((s) => s.status)

  // Tick relative-time labels every second
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  const fetchHistory = useCallback(async () => {
    if (previewMode) return // hold the mock data steady in preview mode
    try {
      setLoading(true)
      const resp = await fetch('/api/timeline/history/main')
      if (resp.ok) {
        const data = await resp.json()
        if (Array.isArray(data)) {
          setEvents(data)
        } else if (data.events && Array.isArray(data.events)) {
          setEvents(data.events)
        }
      }
    } catch {
      // Backend not running
    } finally {
      setLoading(false)
    }
  }, [previewMode])

  useEffect(() => {
    if (wsStatus === 'connected') {
      fetchHistory()
    }
  }, [wsStatus, fetchHistory])

  // Poll for updates (pause when tab is hidden)
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | null = null

    function startPolling() {
      stopPolling()
      timer = setInterval(fetchHistory, 5000)
    }

    function stopPolling() {
      if (timer) { clearInterval(timer); timer = null }
    }

    function handleVisibility() {
      if (document.visibilityState === 'visible') {
        fetchHistory()
        startPolling()
      } else {
        stopPolling()
      }
    }

    startPolling()
    document.addEventListener('visibilitychange', handleVisibility)
    return () => {
      stopPolling()
      document.removeEventListener('visibilitychange', handleVisibility)
    }
  }, [fetchHistory])

  const handleUndo = async () => {
    try {
      const resp = await fetch('/api/timeline/undo', { method: 'POST' })
      if (resp.ok) fetchHistory()
    } catch { /* backend not running */ }
  }

  const handleRedo = async () => {
    try {
      const resp = await fetch('/api/timeline/redo', { method: 'POST' })
      if (resp.ok) fetchHistory()
    } catch { /* backend not running */ }
  }

  const handleCheckpoint = async () => {
    try {
      await fetch('/api/timeline/checkpoint', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: `Checkpoint ${new Date().toLocaleTimeString()}` }),
      })
      fetchHistory()
    } catch { /* backend not running */ }
  }

  const handleBranch = async () => {
    try {
      await fetch('/api/timeline/branch/create', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: `Branch ${Date.now()}` }),
      })
    } catch { /* backend not running */ }
  }

  return (
    <div className="font-mono flex items-center gap-2 px-3 py-1.5 border-t border-border bg-card h-[80px] shrink-0">
      {/* Header: title + glyph actions */}
      <div className="flex items-center gap-0.5 shrink-0 text-[13px]">
        <span className="text-foreground/80 mr-1">timeline</span>
        <HeaderButton onClick={handleUndo} title="Undo (Ctrl+Z)" ariaLabel="Undo">↶</HeaderButton>
        <HeaderButton onClick={handleRedo} title="Redo (Ctrl+Shift+Z)" ariaLabel="Redo">↷</HeaderButton>
        <HeaderButton onClick={handleCheckpoint} title="Checkpoint" ariaLabel="Create checkpoint">◈</HeaderButton>
        <HeaderButton onClick={handleBranch} title="Branch" ariaLabel="Create branch">⑂</HeaderButton>
      </div>

      <span className="text-muted-foreground/40 shrink-0 text-[13px]">│</span>

      {/* Event stream with terminal-style connectors */}
      <div className="flex-1 min-w-0 overflow-x-auto overflow-y-hidden">
        {events.length === 0 ? (
          <div className="text-[12px] text-muted-foreground/60 px-2 py-3">
            {loading ? '⋯ loading' : '∅ no operations yet'}
          </div>
        ) : (
          <div className="flex items-start gap-0 whitespace-nowrap">
            {events.map((event, i) => (
              <Fragment key={event.id}>
                {i > 0 && <Connector />}
                <EventNode event={event} isLatest={i === events.length - 1} now={now} />
              </Fragment>
            ))}
            <span className="text-muted-foreground/40 self-start pt-[1px] ml-1 text-base leading-none">→</span>
          </div>
        )}
      </div>
    </div>
  )
}
