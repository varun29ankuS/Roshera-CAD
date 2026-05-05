import { Fragment, useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react'
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
  onContextMenu,
}: {
  event: EventSummary
  isLatest: boolean
  now: number // re-render anchor for relative time
  onContextMenu: (e: React.MouseEvent, event: EventSummary) => void
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
      onContextMenu={(e) => onContextMenu(e, event)}
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

// ─── Branch overview (minimap on the right edge of the strip) ──────

interface BranchView {
  id: string
  name: string
  parent: string | null
  state: string
  agent_id: string | null
  author: string
  purpose: string
  event_count: number
  created_at: string
}

const MAIN_BRANCH_ID = '00000000-0000-0000-0000-000000000000'

const MOCK_BRANCHES: BranchView[] = [
  { id: MAIN_BRANCH_ID, name: 'main', parent: null, state: 'active',
    agent_id: null, author: 'system', purpose: 'main',
    event_count: MOCK_EVENTS.length, created_at: new Date(Date.now() - 600_000).toISOString() },
  { id: '11111111-1111-1111-1111-111111111111', name: 'claude/explore', parent: MAIN_BRANCH_ID,
    state: 'active', agent_id: 'claude-1', author: 'agent:claude',
    purpose: 'AIOptimization', event_count: 3,
    created_at: new Date(Date.now() - 120_000).toISOString() },
  { id: '22222222-2222-2222-2222-222222222222', name: 'feat/fillet', parent: MAIN_BRANCH_ID,
    state: 'merged', agent_id: null, author: 'user:varun',
    purpose: 'UserExploration', event_count: 2,
    created_at: new Date(Date.now() - 60_000).toISOString() },
]

function shortBranchName(b: BranchView): string {
  if (b.id === MAIN_BRANCH_ID) return 'main'
  return b.name.length > 10 ? `${b.name.slice(0, 9)}…` : b.name
}

/**
 * Git-style branch graph.
 *
 * Mental model: time flows left → right. Each branch is its own
 * horizontal lane. Operations are dots on the lane in time order. A
 * fork drops as an L-elbow from the parent's lane down (or up) to the
 * child's lane at the moment the child was created.
 *
 *   main:    ●─●─●─●─●─●─●─●─●─●─●─●  →
 *                  └─\
 *   claude:           ●─●─●─●  →
 *                       └─\
 *   fix/bug:               ●─●  →
 *
 * Active branch + label render in gizmo Y-axis green (`#2ecc71`); the
 * latest dot pulses to confirm "this is where the next op will land".
 * All other strokes/fills use `var(--foreground)` so the graph
 * naturally inverts with the theme (navy lines on light card / light
 * lines on navy card).
 *
 * Source of truth:
 *  - Active branch's dots come from `events` (real timestamps).
 *  - Non-active branches show `event_count` evenly spaced from their
 *    `created_at` to "now" — we don't fetch per-event histories for
 *    inactive branches (it would 3× the polling traffic).
 *  - Fork x-position uses `branch.created_at` so the elbow lines up
 *    with the moment the branch diverged from its parent.
 *
 * Click anywhere on a lane → `onSelect(branch.id)` → POST
 * `/api/branches/active`, swapping where the kernel records new ops.
 */
function TimelineOverview({
  branches,
  events,
  activeBranchId,
  now,
  onSelect,
}: {
  branches: BranchView[]
  events: EventSummary[]
  activeBranchId: string
  now: number
  onSelect: (branchId: string) => void
}) {
  const [hoveredId, setHoveredId] = useState<string | null>(null)

  // ── DFS-order branches: main first, then each subtree depth-first
  //    sorted by created_at so siblings stack in chronological order.
  const childrenMap: Record<string, BranchView[]> = {}
  branches.forEach((b) => {
    const key = b.parent ?? '__root__'
    if (!childrenMap[key]) childrenMap[key] = []
    childrenMap[key].push(b)
  })
  Object.values(childrenMap).forEach((arr) =>
    arr.sort((a, b) => a.created_at.localeCompare(b.created_at)),
  )
  const ordered: BranchView[] = []
  const seen = new Set<string>()
  function visit(parentKey: string) {
    const kids = childrenMap[parentKey] ?? []
    for (const k of kids) {
      if (seen.has(k.id)) continue
      seen.add(k.id)
      ordered.push(k)
      visit(k.id)
    }
  }
  const main =
    branches.find((b) => b.id === MAIN_BRANCH_ID) ??
    branches.find((b) => b.parent === null)
  if (main && !seen.has(main.id)) {
    seen.add(main.id)
    ordered.push(main)
    visit(main.id)
  }
  // Anything orphaned (parent not in `branches`) — append at the end.
  branches.forEach((b) => {
    if (!seen.has(b.id)) {
      seen.add(b.id)
      ordered.push(b)
    }
  })

  // ── Canvas dimensions ───────────────────────────────────────────────
  // Sized for the *expanded* timeline panel — roomy enough for ~6
  // branches at 36-px lane height with 11-pt labels. Container scrolls
  // horizontally if the SVG outgrows the viewport.
  const nLanes = Math.max(ordered.length, 1)
  const laneH = 36
  const padT = 14
  const padB = 18
  const padL = 12
  const padR = 28
  const width = 880
  const height = padT + padB + nLanes * laneH
  // Left-side gutter reserved for the branch-name label.
  const labelW = 130
  const xStart = padL + labelW
  const xEnd = width - padR

  // ── Time scale: linear from earliest activity to "now" ─────────────
  const tCreate = branches
    .map((b) => new Date(b.created_at).getTime())
    .filter((t) => !isNaN(t))
  const tEvents = events
    .map((e) => new Date(e.timestamp).getTime())
    .filter((t) => !isNaN(t))
  const fallbackMin = now - 60_000
  const tMinCandidates = [...tCreate, ...tEvents]
  const tMin = tMinCandidates.length > 0 ? Math.min(...tMinCandidates) : fallbackMin
  const tMax = Math.max(now, ...tEvents, tMin + 1000)
  const tSpan = Math.max(1, tMax - tMin)
  const scaleX = (t: number): number =>
    xStart + ((t - tMin) / tSpan) * (xEnd - xStart)

  // ── Lane Y per branch ───────────────────────────────────────────────
  const laneY = (i: number): number => padT + (i + 0.5) * laneH

  const idxOf: Record<string, number> = {}
  ordered.forEach((b, i) => {
    idxOf[b.id] = i
  })
  const activeIdx = idxOf[activeBranchId] ?? -1
  const activeBranch = activeIdx >= 0 ? ordered[activeIdx] : undefined

  return (
    <div
      className="shrink-0 flex items-stretch gap-1 pl-1 select-none"
      title="Branch graph — click a lane to switch active branch"
    >
      <svg width={width} height={height} style={{ display: 'block' }}>
        <defs>
          <filter id="rh-git-glow" x="-80%" y="-80%" width="260%" height="260%">
            <feGaussianBlur stdDeviation="1.4" result="blur" />
            <feMerge>
              <feMergeNode in="blur" />
              <feMergeNode in="SourceGraphic" />
            </feMerge>
          </filter>
        </defs>
        <style>{`
          @keyframes rh-git-pulse {
            0%, 100% { opacity: 1;    transform: scale(1); }
            50%      { opacity: 0.65; transform: scale(1.35); }
          }
          .rh-git-pulse {
            transform-origin: center;
            transform-box: fill-box;
            animation: rh-git-pulse 1.8s ease-in-out infinite;
          }
        `}</style>

        {/* ── Fork elbows: parent lane → child lane at child.created_at ── */}
        {ordered.map((b, i) => {
          if (!b.parent) return null
          const pIdx = idxOf[b.parent]
          if (pIdx === undefined) return null
          const t = new Date(b.created_at).getTime()
          if (isNaN(t)) return null
          const fx = scaleX(t)
          const py = laneY(pIdx)
          const cy = laneY(i)
          if (Math.abs(cy - py) < 0.5) return null
          // L-elbow with a small rounded corner where it meets child lane.
          const dy = cy - py
          const r = Math.min(5, Math.abs(dy) / 2)
          const sgn = dy > 0 ? 1 : -1
          const d =
            `M ${fx} ${py} ` +
            `L ${fx} ${cy - sgn * r} ` +
            `Q ${fx} ${cy} ${fx + r} ${cy}`
          const onActivePath = i === activeIdx || pIdx === activeIdx
          return (
            <path
              key={`fork-${b.id}`}
              d={d}
              fill="none"
              stroke={onActivePath ? '#2ecc71' : 'var(--foreground)'}
              strokeOpacity={onActivePath ? 0.85 : 0.4}
              strokeWidth={onActivePath ? 1.4 : 1.0}
              strokeLinecap="round"
            />
          )
        })}

        {/* ── Lanes ──────────────────────────────────────────────────── */}
        {ordered.map((b, i) => {
          const y = laneY(i)
          const isMain = b.id === MAIN_BRANCH_ID || b.parent === null
          const isActive = i === activeIdx
          const isHover = hoveredId === b.id
          const isMerged = b.state === 'merged'
          const isAbandoned = b.state === 'abandoned'

          // Lane left endpoint: main spans the whole time axis; other
          // branches start at their fork x.
          let xs = xStart
          if (!isMain) {
            const t = new Date(b.created_at).getTime()
            if (!isNaN(t)) xs = scaleX(t)
          }

          // Dot positions:
          //  - Active branch uses real `events` timestamps.
          //  - Inactive: even spacing from xs to xEnd by event_count.
          let dots: { x: number; isLatest: boolean }[] = []
          if (isActive && events.length > 0) {
            dots = events.map((e, j) => {
              const t = new Date(e.timestamp).getTime()
              return {
                x: isNaN(t) ? xs : scaleX(t),
                isLatest: j === events.length - 1,
              }
            })
          } else if (b.event_count > 0) {
            const span = Math.max(0, xEnd - xs)
            const n = b.event_count
            for (let j = 0; j < n; j++) {
              dots.push({
                x: xs + (span * (j + 1)) / (n + 1),
                isLatest: j === n - 1,
              })
            }
          }

          const laneColor = isActive
            ? '#2ecc71'
            : isAbandoned
              ? 'rgba(251,146,60,0.85)'
              : 'var(--foreground)'
          const laneOpacity = isActive
            ? 0.9
            : isHover
              ? 0.7
              : isMerged
                ? 0.32
                : 0.55
          const dotOpacity = isActive
            ? 1
            : isHover
              ? 0.95
              : isMerged
                ? 0.4
                : 0.65

          const labelStyle: React.CSSProperties = isActive
            ? { fill: '#2ecc71' }
            : {}
          const labelClass = isActive
            ? undefined
            : isHover
              ? 'fill-foreground/95'
              : 'fill-foreground/70'

          return (
            <g
              key={`lane-${b.id}`}
              onClick={() => onSelect(b.id)}
              onPointerEnter={() => setHoveredId(b.id)}
              onPointerLeave={() =>
                setHoveredId((id) => (id === b.id ? null : id))
              }
              style={{ cursor: 'pointer' }}
            >
              {/* Hit area: full lane height, full width. */}
              <rect
                x={padL}
                y={y - laneH / 2}
                width={width - padL}
                height={laneH}
                fill="transparent"
              />

              {/* Branch name in left gutter, right-aligned at xStart. */}
              <text
                x={xStart - 8}
                y={y + 4}
                textAnchor="end"
                className={labelClass}
                fontSize="12"
                fontWeight={isActive ? 700 : 500}
                letterSpacing="0.2"
                style={{ fontFamily: 'inherit', ...labelStyle }}
              >
                {b.name && b.name.length > 0 ? b.name : shortBranchName(b)}
              </text>

              {/* Lane line. */}
              <line
                x1={xs}
                y1={y}
                x2={xEnd}
                y2={y}
                stroke={laneColor}
                strokeOpacity={laneOpacity}
                strokeWidth={isActive ? 1.5 : 1.0}
                strokeLinecap="round"
              />

              {/* Right-edge arrow head. */}
              <text
                x={xEnd + 4}
                y={y + 4}
                fontSize="12"
                fill={isActive ? '#2ecc71' : 'var(--foreground)'}
                fillOpacity={isActive ? 0.95 : 0.45}
                style={{ fontFamily: 'inherit' }}
              >
                →
              </text>

              {/* Op dots. Latest dot on the active lane breathes. */}
              {dots.map((dot, j) => {
                const r = isActive ? (dot.isLatest ? 4.5 : 3.5) : 3.0
                const pulse = isActive && dot.isLatest
                return (
                  <circle
                    key={j}
                    cx={dot.x}
                    cy={y}
                    r={r}
                    fill={laneColor}
                    fillOpacity={dotOpacity}
                    className={pulse ? 'rh-git-pulse' : undefined}
                    filter={pulse ? 'url(#rh-git-glow)' : undefined}
                  />
                )
              })}

              {/* Op count near the right end (idle lanes only — active
                  shows live dots). */}
              {!isActive && b.event_count > 0 && (
                <text
                  x={xEnd - 4}
                  y={y - 6}
                  textAnchor="end"
                  className="fill-foreground/55"
                  fontSize="10"
                  style={{ fontFamily: 'inherit' }}
                >
                  {b.event_count} ops
                </text>
              )}
            </g>
          )
        })}

        {/* Footer status — confirms which branch is live. */}
        <text
          x={padL + 4}
          y={height - 4}
          fontSize="10"
          letterSpacing="0.4"
          style={{ fontFamily: 'inherit', fill: '#2ecc71' }}
          fillOpacity={0.85}
        >
          {activeBranch
            ? `▶ active: ${activeBranch.name || shortBranchName(activeBranch)}`
            : '▶ —'}
        </text>
      </svg>
    </div>
  )
}

// ─── Main strip ─────────────────────────────────────────────────────

interface EventContextMenuState {
  x: number
  y: number
  event: EventSummary
}

export function Timeline() {
  const previewMode = isPreviewMode()
  const [events, setEvents] = useState<EventSummary[]>(previewMode ? MOCK_EVENTS : [])
  const [branches, setBranches] = useState<BranchView[]>(previewMode ? MOCK_BRANCHES : [])
  // Active branch = the one whose events are shown in the strip AND
  // the one the kernel records new operations against. The minimap
  // calls `selectBranch(id)` when the user clicks a node; we POST
  // `/api/branches/active` first so the backend's TimelineRecorder
  // swaps its target before we update local state and re-fetch.
  // Without this round-trip, new operations would keep landing on
  // `main` regardless of UI state — the bug Varun was hitting.
  const [activeBranchId, setActiveBranchId] = useState<string>(MAIN_BRANCH_ID)
  // Collapsed = linear strip of the active branch only (the default).
  // Expanded = drops down a roomy git-style graph panel above the
  // strip showing every branch + fork structure. The graph is heavy
  // visually and adds little when you're focused on a single branch,
  // so it stays out of the way until you ask for it.
  const [overviewExpanded, setOverviewExpanded] = useState(false)
  const selectBranch = useCallback(async (branchId: string) => {
    if (branchId === activeBranchId) return
    try {
      const resp = await fetch('/api/branches/active', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ branch_id: branchId }),
      })
      if (!resp.ok) {
        console.error('[timeline] set-active-branch failed:', resp.status)
        return
      }
      setActiveBranchId(branchId)
    } catch (err) {
      console.error('[timeline] set-active-branch threw:', err)
    }
  }, [activeBranchId])
  const [loading, setLoading] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const [menu, setMenu] = useState<EventContextMenuState | null>(null)
  const wsStatus = useWSStore((s) => s.status)

  const handleEventContextMenu = useCallback(
    (e: React.MouseEvent, event: EventSummary) => {
      e.preventDefault()
      e.stopPropagation()
      setMenu({ x: e.clientX, y: e.clientY, event })
    },
    [],
  )

  const closeMenu = useCallback(() => setMenu(null), [])

  // Tick relative-time labels every second
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  const fetchHistory = useCallback(async () => {
    if (previewMode) return // hold the mock data steady in preview mode
    try {
      setLoading(true)
      const [histResp, branchResp] = await Promise.all([
        fetch(`/api/timeline/history/${activeBranchId}`),
        fetch('/api/branches'),
      ])
      if (histResp.ok) {
        const data = await histResp.json()
        if (Array.isArray(data)) {
          setEvents(data)
        } else if (data.events && Array.isArray(data.events)) {
          setEvents(data.events)
        }
      }
      if (branchResp.ok) {
        const data = await branchResp.json()
        if (Array.isArray(data)) {
          setBranches(data as BranchView[])
        }
      }
    } catch {
      // Backend not running
    } finally {
      setLoading(false)
    }
  }, [previewMode, activeBranchId])

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
    // Backend `/api/timeline/undo` requires `session_id` in the body
    // (see handlers/timeline.rs::undo_operation). Without it the call
    // silently 400s and the toolbar button looks broken. Mirror the
    // keyboard-shortcut path in `lib/shortcuts.ts`.
    const sessionId = useWSStore.getState().sessionId
    if (!sessionId) return
    try {
      const resp = await fetch('/api/timeline/undo', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session_id: sessionId }),
      })
      if (resp.ok) fetchHistory()
    } catch { /* backend not running */ }
  }

  const handleRedo = async () => {
    const sessionId = useWSStore.getState().sessionId
    if (!sessionId) return
    try {
      const resp = await fetch('/api/timeline/redo', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session_id: sessionId }),
      })
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
    // Hit `/api/branches` (timeline-backed) rather than the legacy
    // `/api/timeline/branch/create` route — the latter writes into a
    // separate `branch_manager` store that `GET /api/branches` (used
    // by the overview minimap) does not read from, so its branches
    // never show up in the UI.
    try {
      const ts = new Date()
      const stamp = `${ts.getHours().toString().padStart(2, '0')}:${ts
        .getMinutes()
        .toString()
        .padStart(2, '0')}:${ts.getSeconds().toString().padStart(2, '0')}`
      const resp = await fetch('/api/branches', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: `branch-${stamp}`,
          parent: activeBranchId,
          description: 'manual branch from timeline strip',
        }),
      })
      if (!resp.ok) {
        console.error('[timeline] branch create failed:', resp.status)
      }
      fetchHistory()
    } catch (err) {
      console.error('[timeline] branch threw:', err)
    }
  }

  const activeBranch = branches.find((b) => b.id === activeBranchId)
  const activeBranchLabel = activeBranch
    ? activeBranch.name || (activeBranch.id === MAIN_BRANCH_ID ? 'main' : '…')
    : 'main'
  const otherBranchCount = Math.max(0, branches.length - 1)

  return (
    <div className="font-mono flex flex-col border-t border-border bg-card shrink-0">
      {/* Expanded panel: full git-style graph. Sits above the strip
          and only renders when the user clicks ▾. Horizontal scroll
          if the SVG is wider than the viewport. */}
      {overviewExpanded && (
        <div className="border-b border-border bg-card/80 overflow-x-auto overflow-y-auto max-h-[320px]">
          <TimelineOverview
            branches={branches}
            events={events}
            activeBranchId={activeBranchId}
            now={now}
            onSelect={selectBranch}
          />
        </div>
      )}

      <div className="flex items-center gap-2 px-3 py-1.5 h-[80px]">
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
                  <EventNode
                    event={event}
                    isLatest={i === events.length - 1}
                    now={now}
                    onContextMenu={handleEventContextMenu}
                  />
                </Fragment>
              ))}
              <span className="text-muted-foreground/40 self-start pt-[1px] ml-1 text-base leading-none">→</span>
            </div>
          )}
        </div>

        {/* Right side: branch chip (active branch + count of others)
            and the expand/collapse toggle. Click anywhere on the chip
            to expand. */}
        <button
          type="button"
          onClick={() => setOverviewExpanded((v) => !v)}
          title={
            overviewExpanded
              ? 'Hide branch graph'
              : `Show branch graph (${branches.length} branch${branches.length === 1 ? '' : 'es'})`
          }
          className="shrink-0 flex items-center gap-1.5 px-2 py-1 rounded text-[12px] hover:bg-accent/40 transition-colors"
        >
          <span
            aria-hidden
            className="inline-block w-1.5 h-1.5 rounded-full"
            style={{ backgroundColor: '#2ecc71' }}
          />
          <span className="text-foreground/90 font-medium">
            {activeBranchLabel}
          </span>
          {otherBranchCount > 0 && (
            <span className="text-foreground/50">+{otherBranchCount}</span>
          )}
          <span className="text-foreground/60 ml-0.5">
            {overviewExpanded ? '▴' : '▾'}
          </span>
        </button>
      </div>

      {menu && (
        <EventContextMenu
          menu={menu}
          onClose={closeMenu}
          onTruncate={async (mode) => {
            const sessionId = useWSStore.getState().sessionId
            if (!sessionId) {
              closeMenu()
              return
            }
            try {
              const resp = await fetch('/api/timeline/truncate', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                  session_id: sessionId,
                  event_id: menu.event.id,
                  mode,
                }),
              })
              if (!resp.ok) {
                console.error('[timeline] truncate failed:', resp.status)
              }
            } catch (err) {
              console.error('[timeline] truncate threw:', err)
            } finally {
              closeMenu()
              fetchHistory()
            }
          }}
        />
      )}
    </div>
  )
}

// ─── Per-event context menu (right-click → copy id / kind / json) ───

function EventContextMenu({
  menu,
  onClose,
  onTruncate,
}: {
  menu: EventContextMenuState
  onClose: () => void
  onTruncate: (mode: 'from_here' | 'after_here') => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  // Flip upward / inward when the click is near the viewport edge — the
  // Timeline strip lives at the bottom of the screen so a downward menu
  // routinely fell off the page. Measure after first paint, then reposition.
  const [pos, setPos] = useState<{ x: number; y: number; ready: boolean }>({
    x: menu.x,
    y: menu.y,
    ready: false,
  })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight
    const margin = 8
    let x = menu.x
    let y = menu.y
    if (x + rect.width > vw - margin) {
      x = Math.max(margin, menu.x - rect.width)
    }
    if (y + rect.height > vh - margin) {
      y = Math.max(margin, menu.y - rect.height)
    }
    setPos({ x, y, ready: true })
  }, [menu.x, menu.y])

  useEffect(() => {
    const onMouseDown = (e: MouseEvent) => {
      const el = ref.current
      if (el && e.target instanceof Node && el.contains(e.target)) return
      onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const copy = useCallback(
    async (text: string) => {
      try {
        await navigator.clipboard.writeText(text)
      } catch (err) {
        console.error('[timeline] clipboard write failed:', err)
      } finally {
        onClose()
      }
    },
    [onClose],
  )

  // The event details that go to the clipboard for "Copy JSON" — prefer
  // the full structured operation when the backend ships it, fall back
  // to the slim summary so the action always produces something useful.
  const fullJson = JSON.stringify(
    {
      id: menu.event.id,
      sequence_number: menu.event.sequence_number,
      timestamp: menu.event.timestamp,
      operation_type: menu.event.operation_type,
      author: menu.event.author,
      author_kind: menu.event.author_kind,
      operation: menu.event.operation,
    },
    null,
    2,
  )

  const kind = normalizeKind(menu.event.operation_type)

  return (
    <div
      ref={ref}
      className="fixed z-50 cad-panel min-w-[180px] py-1 text-[12px] shadow-lg select-none"
      style={{ left: pos.x, top: pos.y, visibility: pos.ready ? 'visible' : 'hidden' }}
      role="menu"
    >
      <TimelineMenuItem onClick={() => copy(menu.event.id)}>
        <span className="text-muted-foreground">id ·</span> {menu.event.id.slice(0, 8)}…
      </TimelineMenuItem>
      <TimelineMenuItem onClick={() => copy(kind)}>
        <span className="text-muted-foreground">kind ·</span> {kind}
      </TimelineMenuItem>
      <TimelineMenuItem onClick={() => copy(fullJson)}>
        <span className="text-muted-foreground">json ·</span> full event
      </TimelineMenuItem>
      <div className="my-1 border-t border-border/40" />
      <TimelineMenuItem onClick={() => onTruncate('after_here')}>
        <span className="text-muted-foreground">↺ ·</span> rewind to here
      </TimelineMenuItem>
      <TimelineMenuItem onClick={() => onTruncate('from_here')} danger>
        <span className="text-muted-foreground">✕ ·</span> delete from here
      </TimelineMenuItem>
    </div>
  )
}

function TimelineMenuItem({
  children,
  onClick,
  danger,
}: {
  children: React.ReactNode
  onClick: () => void
  danger?: boolean
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className={cn(
        'w-full text-left px-3 py-1.5 hover:bg-accent/40 transition-colors',
        danger ? 'text-orange-500 hover:text-orange-400' : 'text-foreground/90',
      )}
    >
      {children}
    </button>
  )
}
