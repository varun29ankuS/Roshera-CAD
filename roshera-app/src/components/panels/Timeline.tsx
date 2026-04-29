import { useState, useEffect, useCallback } from 'react'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useWSStore } from '@/stores/ws-store'
import {
  Clock,
  Undo2,
  Redo2,
  Bookmark,
  GitBranch,
  Box,
  Combine,
  Diff,
  SquaresIntersect,
  ArrowUpFromLine,
  RefreshCcw,
  Move3d,
  Trash2,
  Disc,
  Hexagon,
  Grip,
  type LucideIcon,
} from 'lucide-react'
import { cn } from '@/lib/utils'

// ─── Types matching backend GET /api/timeline/history/{branch_id} ──

interface EventSummary {
  id: string
  sequence_number: number
  timestamp: string // ISO 8601
  operation_type: string
  author: string
}

// ─── Icon map ───────────────────────────────────────────────────────

function iconForOperation(op: string): LucideIcon {
  const lower = op.toLowerCase()
  if (lower.includes('createprimitive') || lower.includes('create')) return Box
  if (lower.includes('booleanunion') || lower.includes('union')) return Combine
  if (lower.includes('booleanintersection') || lower.includes('intersection')) return SquaresIntersect
  if (lower.includes('booleandifference') || lower.includes('difference') || lower.includes('subtract')) return Diff
  if (lower.includes('extrude')) return ArrowUpFromLine
  if (lower.includes('revolve')) return RefreshCcw
  if (lower.includes('transform')) return Move3d
  if (lower.includes('delete')) return Trash2
  if (lower.includes('fillet')) return Disc
  if (lower.includes('chamfer')) return Hexagon
  return Grip
}

function formatOperationType(op: string): string {
  // Backend sends things like "CreatePrimitive { shape_type: Box, ... }"
  // Extract the readable part
  const match = op.match(/^(\w+)/)
  if (!match) return op
  // Convert PascalCase to spaced words
  return match[1].replace(/([A-Z])/g, ' $1').trim()
}

function formatTimestamp(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return ts
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function formatAuthor(author: string): string {
  // Backend sends "User { id: ..., name: John }" or "AIAgent { ... }" or "System"
  if (author === 'System') return 'System'
  const nameMatch = author.match(/name:\s*(\w+)/)
  if (nameMatch) return nameMatch[1]
  if (author.includes('AIAgent')) return 'AI'
  if (author.includes('User')) return 'User'
  return author
}

// ─── Timeline entry ─────────────────────────────────────────────────

function TimelineEntry({
  event,
  isLatest,
}: {
  event: EventSummary
  isLatest: boolean
}) {
  const icon = iconForOperation(event.operation_type)

  return (
    <div
      className={cn(
        'flex items-start gap-2 px-3 py-1.5 transition-colors',
        isLatest ? 'bg-primary/10' : 'hover:bg-accent/30',
      )}
    >
      <div className="flex flex-col items-center pt-0.5">
        <div
          className={cn(
            'w-5 h-5 rounded-full flex items-center justify-center shrink-0',
            isLatest ? 'bg-primary/20 text-primary' : 'bg-muted text-muted-foreground',
          )}
        >
          {icon({ size: 10, strokeWidth: 1.5 })}
        </div>
      </div>

      <div className="flex-1 min-w-0">
        <div className="text-[11px] text-foreground/80 truncate">
          {formatOperationType(event.operation_type)}
        </div>
        <div className="flex items-center gap-2 text-[9px] text-muted-foreground/50 mt-0.5">
          <span>#{event.sequence_number}</span>
          <span>{formatTimestamp(event.timestamp)}</span>
          <span>{formatAuthor(event.author)}</span>
        </div>
      </div>
    </div>
  )
}

// ─── Main panel ─────────────────────────────────────────────────────

export function Timeline() {
  const [events, setEvents] = useState<EventSummary[]>([])
  const [loading, setLoading] = useState(false)
  const wsStatus = useWSStore((s) => s.status)

  const fetchHistory = useCallback(async () => {
    try {
      setLoading(true)
      const resp = await fetch('/api/timeline/history/main')
      if (resp.ok) {
        const data = await resp.json()
        // Backend returns Vec<EventSummary> directly
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
  }, [])

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
    <div className="flex flex-col h-full">
      <div className="cad-panel-header flex items-center justify-between">
        <div className="flex items-center gap-1.5">
          <Clock size={11} className="text-primary" />
          Timeline
        </div>
        <div className="flex items-center gap-0.5">
          <button
            onClick={handleUndo}
            className="cad-icon-btn h-5 w-5"
            title="Undo (Ctrl+Z)"
            aria-label="Undo"
          >
            <Undo2 size={11} />
          </button>
          <button
            onClick={handleRedo}
            className="cad-icon-btn h-5 w-5"
            title="Redo (Ctrl+Shift+Z)"
            aria-label="Redo"
          >
            <Redo2 size={11} />
          </button>
          <button
            onClick={handleCheckpoint}
            className="cad-icon-btn h-5 w-5"
            title="Create Checkpoint"
            aria-label="Create checkpoint"
          >
            <Bookmark size={11} />
          </button>
          <button
            onClick={handleBranch}
            className="cad-icon-btn h-5 w-5"
            title="Create Branch"
            aria-label="Create branch"
          >
            <GitBranch size={11} />
          </button>
        </div>
      </div>

      <ScrollArea className="flex-1">
        {events.length === 0 ? (
          <div className="p-3 text-[11px] text-muted-foreground/60 text-center">
            {loading ? 'Loading...' : 'No operations yet'}
          </div>
        ) : (
          <div className="py-1">
            {events.map((event, i) => (
              <TimelineEntry
                key={event.id}
                event={event}
                isLatest={i === events.length - 1}
              />
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  )
}
