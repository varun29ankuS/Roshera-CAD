import { Fragment, useState, useEffect, useLayoutEffect, useCallback, useMemo, useRef } from 'react'
import { useWSStore } from '@/stores/ws-store'
import { useSceneStore } from '@/stores/scene-store'
import { cn } from '@/lib/utils'
import {
  type EventSummary,
  normalizeKind,
  symbolForOperation,
  shortLabel,
  formatTimestamp,
  relativeTime,
  formatAuthor,
  authorKind,
  authorGlyph,
  authorTextClass,
} from '@/lib/timeline-events'

// NOTE: these helpers + types are NOT re-exported from here. Consumers must
// import them directly from `@/lib/timeline-events` (the source of truth).
// Re-exporting non-component values from a component module breaks Vite
// fast-refresh (react-refresh/only-export-components).

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

// Types + kind/symbol/label/author helpers now live in
// `@/lib/timeline-events` and are imported + re-exported at the top
// of this file so the bottom Timeline strip and the top-left Feature
// Tree share a single rendering source-of-truth.

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

// ─── Per-part swimlanes (Certified Timeline slice 1) ────────────────
//
// One canonical event log, grouped into lanes by the part each event
// touched (`EventSummary.affected_parts`, backend-computed). Consumed
// operands are NOT here (they're inputs), so a boolean that merges two
// solids into a third appears only on the third's lane. Events with no
// solid output (drawings, parameter moulds) collect in a session lane.

const SESSION_LANE = '·session'

/** Lane label = the SAME name the browser/model tree shows for that solid.
 *  `liveNames` maps a kernel solid id ("solid:2" lane key) to the live scene
 *  object's `name` (custom names from create/rename included) via
 *  `analyticalGeometry.solidId` — the same resolution PartDimensions uses.
 *  Consumed/historical solids have no live object, so they honestly fall back
 *  to the kernel default `solid_N`. The session bucket → "session". */
function laneLabel(key: string, liveNames: Map<string, string>): string {
  const live = liveNames.get(key)
  if (live) return live
  const m = key.match(/^solid:(.+)$/)
  if (m) return `solid_${m[1]}`
  if (key === SESSION_LANE) return 'session'
  return key
}

interface Lane {
  key: string
  label: string
  events: EventSummary[]
}

/** Bucket events into per-part lanes in first-appearance order; the
 *  session lane (non-geometry events) always sorts last. An event that
 *  affects multiple parts appears on each of their lanes. */
function groupIntoLanes(
  events: EventSummary[],
  liveNames: Map<string, string>,
): Lane[] {
  const order: string[] = []
  const buckets = new Map<string, EventSummary[]>()
  for (const ev of events) {
    const keys = ev.affected_parts && ev.affected_parts.length > 0
      ? ev.affected_parts
      : [SESSION_LANE]
    for (const k of keys) {
      const bucket = buckets.get(k)
      if (bucket) {
        bucket.push(ev)
      } else {
        buckets.set(k, [ev])
        order.push(k)
      }
    }
  }
  return order
    .sort((a, b) => (a === SESSION_LANE ? 1 : b === SESSION_LANE ? -1 : 0))
    .map((k) => ({
      key: k,
      label: laneLabel(k, liveNames),
      events: buckets.get(k) ?? [],
    }))
}

function LaneTrack({
  events,
  latestId,
  now,
  onContextMenu,
}: {
  events: EventSummary[]
  latestId: string | undefined
  now: number
  onContextMenu: (e: React.MouseEvent, event: EventSummary) => void
}) {
  return (
    <div className="flex-1 min-w-0 overflow-x-auto overflow-y-hidden flex items-start gap-0 whitespace-nowrap px-2 py-1">
      {events.map((event, i) => (
        <Fragment key={event.id}>
          {i > 0 && <Connector />}
          <EventNode
            event={event}
            isLatest={event.id === latestId}
            now={now}
            onContextMenu={onContextMenu}
          />
        </Fragment>
      ))}
      <span className="text-muted-foreground/40 self-start pt-[1px] ml-1 text-base leading-none">
        →
      </span>
    </div>
  )
}

/** Vertical stack of per-part lanes — the by-part view. The globally
 *  latest event pulses on whichever lane it sits in. */
function Swimlanes({
  events,
  liveNames,
  now,
  onContextMenu,
  loading,
}: {
  events: EventSummary[]
  liveNames: Map<string, string>
  now: number
  onContextMenu: (e: React.MouseEvent, event: EventSummary) => void
  loading: boolean
}) {
  if (events.length === 0) {
    return (
      <div className="text-[12px] text-muted-foreground/60 px-3 py-3">
        {loading ? '⋯ loading' : '∅ no operations yet'}
      </div>
    )
  }
  const lanes = groupIntoLanes(events, liveNames)
  const latestId = events[events.length - 1]?.id
  // Cap the dock at ~2.5 lanes and scroll the rest, so the panel never
  // grows tall enough to crowd the 3D viewport no matter how many parts
  // exist. A subtle scrollbar signals there's more below.
  return (
    <div className="max-h-[124px] overflow-y-auto">
      {lanes.map((lane) => (
        <div
          key={lane.key}
          className="flex items-stretch border-b border-border/40 last:border-b-0 min-h-[46px]"
        >
          <div className="w-[92px] shrink-0 flex flex-col justify-center gap-0 px-2.5 border-r border-border/40">
            <span className="text-[11px] text-foreground/90 truncate leading-tight">
              {lane.label}
            </span>
            <span className="text-[9px] text-muted-foreground/60 leading-tight">
              {lane.events.length} op{lane.events.length === 1 ? '' : 's'}
            </span>
          </div>
          <LaneTrack
            events={lane.events}
            latestId={latestId}
            now={now}
            onContextMenu={onContextMenu}
          />
        </div>
      ))}
    </div>
  )
}

/** Flat, time-ordered strip across all parts — the by-time view (the
 *  original layout). Nodes tint their short label by affected part so the
 *  interleave stays legible. */
function FlatStrip({
  events,
  now,
  onContextMenu,
  loading,
}: {
  events: EventSummary[]
  now: number
  onContextMenu: (e: React.MouseEvent, event: EventSummary) => void
  loading: boolean
}) {
  if (events.length === 0) {
    return (
      <div className="text-[12px] text-muted-foreground/60 px-3 py-3">
        {loading ? '⋯ loading' : '∅ no operations yet'}
      </div>
    )
  }
  return (
    <div className="overflow-x-auto overflow-y-hidden px-3 py-2">
      <div className="flex items-start gap-0 whitespace-nowrap">
        {events.map((event, i) => (
          <Fragment key={event.id}>
            {i > 0 && <Connector />}
            <EventNode
              event={event}
              isLatest={i === events.length - 1}
              now={now}
              onContextMenu={onContextMenu}
            />
          </Fragment>
        ))}
        <span className="text-muted-foreground/40 self-start pt-[1px] ml-1 text-base leading-none">
          →
        </span>
      </div>
    </div>
  )
}

// ─── Header glyph button ────────────────────────────────────────────

function HeaderButton({
  children,
  onClick,
  title,
  ariaLabel,
  disabled,
}: {
  children: React.ReactNode
  onClick: () => void
  title: string
  ariaLabel: string
  disabled?: boolean
}) {
  return (
    <button
      onClick={onClick}
      title={title}
      aria-label={ariaLabel}
      disabled={disabled}
      className="px-1.5 py-0.5 rounded text-foreground/70 hover:text-foreground hover:bg-accent/40 transition-colors disabled:opacity-30 disabled:cursor-not-allowed disabled:hover:bg-transparent disabled:hover:text-foreground/70"
    >
      {children}
    </button>
  )
}

// ─── Branch overview (minimap on the right edge of the strip) ──────

interface ForkPointView {
  /** Parent branch id — `"main"` (literal) or a UUIDv4 string. */
  branch_id: string
  /** Parent's head event index at fork time. 0 on `main`. */
  event_index: number
  /** ISO-8601 timestamp of the fork. */
  timestamp: string
}

interface BranchView {
  id: string
  name: string
  parent: string | null
  state: string
  agent_id: string | null
  author: string
  purpose: string
  /** Total events visible on this branch — includes events inherited
   *  from the parent up to the fork point. Renderers should NOT use
   *  this for plotting per-branch dots, otherwise inherited parent
   *  events get drawn as phantom child events on the child lane. */
  event_count: number
  /** Events recorded on this branch *strictly after* its fork point.
   *  Use this for the per-branch dot count: a freshly-forked branch
   *  with no new ops shows `events_since_fork === 0` and renders as
   *  just a fork-elbow with no extra dots. Optional for back-compat
   *  with backends that pre-date the field. */
  events_since_fork?: number
  created_at: string
  /** Optional for back-compat with persisted timelines that pre-date
   *  the field. New backends always set it; renderers fall back to
   *  `created_at` when absent. */
  fork_point?: ForkPointView
}

const MAIN_BRANCH_ID = '00000000-0000-0000-0000-000000000000'

const MOCK_BRANCHES: BranchView[] = [
  { id: MAIN_BRANCH_ID, name: 'main', parent: null, state: 'active',
    agent_id: null, author: 'system', purpose: 'main',
    event_count: MOCK_EVENTS.length, created_at: new Date(Date.now() - 600_000).toISOString(),
    fork_point: { branch_id: 'main', event_index: 0,
      timestamp: new Date(Date.now() - 600_000).toISOString() } },
  { id: '11111111-1111-1111-1111-111111111111', name: 'claude/explore', parent: MAIN_BRANCH_ID,
    state: 'active', agent_id: 'claude-1', author: 'agent:claude',
    purpose: 'AIOptimization', event_count: 3,
    created_at: new Date(Date.now() - 120_000).toISOString(),
    fork_point: { branch_id: MAIN_BRANCH_ID, event_index: 4,
      timestamp: new Date(Date.now() - 120_000).toISOString() } },
  { id: '22222222-2222-2222-2222-222222222222', name: 'feat/fillet', parent: MAIN_BRANCH_ID,
    state: 'merged', agent_id: null, author: 'user:varun',
    purpose: 'UserExploration', event_count: 2,
    created_at: new Date(Date.now() - 60_000).toISOString(),
    fork_point: { branch_id: MAIN_BRANCH_ID, event_index: 6,
      timestamp: new Date(Date.now() - 60_000).toISOString() } },
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
 *  - Fork x-position is anchored at the parent's Nth event-dot
 *    (where N = `fork_point.event_index`), not at `branch.created_at`.
 *    The wall-clock approach collapsed every fork to ~timeline start
 *    on fresh sessions because the time between forks was tiny next
 *    to the visible time-span. With event-index anchoring, a fork
 *    that happened just after the parent's 5th op stays put even as
 *    new ops land. Falls back to `created_at` for snapshots that
 *    pre-date the `fork_point` field.
 *
 * Click anywhere on a lane → `onSelect(branch.id)` → POST
 * `/api/branches/active`, swapping where the kernel records new ops.
 */
function TimelineOverview({
  branches: rawBranches,
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

  // Hide abandoned and merged branches from the graph. Both linger
  // in the backend's branch list (abandon = soft-delete with events
  // retained for restore; merge = source becomes `Merged` after a
  // fast-forward fold-in) but visually they're "phantom timelines"
  // that confuse the user. Keep them out of every position calc as
  // well as the render passes below — the events they contributed
  // already live on `main` (for merged) or are reachable via the
  // restore flow (for abandoned).
  const branches = rawBranches.filter(
    (b) => b.state !== 'abandoned' && b.state !== 'merged',
  )

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

  // ── Pre-compute lane left endpoints + per-branch dot positions ────
  // We resolve in DFS order (parents before children) so a child's
  // fork-x can read its parent's dot at `fork_point.event_index`.
  // `xsByBranch[b.id]` is also the fork-elbow's x for non-main branches.
  type LaneDot = {
    x: number
    isLatest: boolean
    /** Only populated for the active branch, where we have the
     *  per-event payload from `/api/timeline/history`. Lets the lane
     *  renderer print the operation symbol/label above each dot so it
     *  is clear what was done in this branch. */
    event?: EventSummary
  }
  const xsByBranch: Record<string, number> = {}
  const dotsByBranch: Record<string, LaneDot[]> = {}
  ordered.forEach((b, i) => {
    const isMain = b.id === MAIN_BRANCH_ID || b.parent === null
    const isActive = i === activeIdx

    let xs = xStart
    if (!isMain) {
      const parentId = b.parent ?? MAIN_BRANCH_ID
      const parentDots = dotsByBranch[parentId]
      const parentXs = xsByBranch[parentId]
      // `fork_point.event_index` is the parent's *head sequence number*
      // at fork time (0-based). For 1 inherited event it's 0, for 3
      // it's 2. To anchor the fork-elbow under the corresponding dot
      // we use it as a 0-based index into `parentDots` directly.
      // `event_index === 0` is a real, valid fork (after the parent's
      // first event); the only case where there is nothing to anchor
      // on is when the parent has zero dots at all.
      const forkIdx = b.fork_point?.event_index
      const fx =
        forkIdx !== undefined && parentDots && parentDots.length > 0
          ? parentDots[Math.min(forkIdx, parentDots.length - 1)].x
          : undefined
      if (fx !== undefined) {
        xs = fx
      } else if (parentXs !== undefined) {
        xs = parentXs
      } else {
        // Last-resort fallback for pre-fork_point snapshots.
        const t = new Date(b.created_at).getTime()
        if (!isNaN(t)) xs = scaleX(t)
      }
    }
    xsByBranch[b.id] = xs

    let dots: LaneDot[] = []
    if (isActive && events.length > 0) {
      // Distribute the active branch's events evenly across the lane
      // and attach each event payload so we can label the dots. We
      // deliberately do NOT use timestamp-based positioning: real
      // edit sessions cluster many ops within seconds, which would
      // collapse all dots to a single overlapping pile. Evenly-spaced
      // matches what inactive lanes do, keeps ordering legible, and
      // sidesteps the "everything is bunched at the right edge" bug.
      const n = events.length
      const span = Math.max(0, xEnd - xs)
      dots = events.map((e, j) => ({
        x: xs + (span * (j + 1)) / (n + 1),
        isLatest: j === n - 1,
        event: e,
      }))
    } else {
      // For non-active branches we draw evenly-spaced dots representing
      // ops on *this* branch since the fork. Use `events_since_fork`
      // when the backend supplied it; fall back to `event_count` for
      // pre-`events_since_fork` backends — the latter over-counts on
      // child branches (it includes inherited parent events) but is
      // the only signal those older payloads carry.
      const n = b.events_since_fork ?? b.event_count
      if (n > 0) {
        const span = Math.max(0, xEnd - xs)
        for (let j = 0; j < n; j++) {
          dots.push({
            x: xs + (span * (j + 1)) / (n + 1),
            isLatest: j === n - 1,
          })
        }
      }
    }
    dotsByBranch[b.id] = dots
  })

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

        {/* ── Fork elbows: parent's Nth event-dot → child lane ─────── */}
        {ordered.map((b, i) => {
          if (!b.parent) return null
          const pIdx = idxOf[b.parent]
          if (pIdx === undefined) return null
          const fx = xsByBranch[b.id]
          if (fx === undefined) return null
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
          const isActive = i === activeIdx
          const isHover = hoveredId === b.id
          const isMerged = b.state === 'merged'
          const isAbandoned = b.state === 'abandoned'

          // Lane left endpoint + dots are computed in the DFS pass
          // above so children can read their parent's dot positions.
          const xs = xsByBranch[b.id] ?? xStart
          const dots = dotsByBranch[b.id] ?? []

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

          // Use explicit SVG fill + fillOpacity attributes rather than
          // Tailwind `fill-foreground/NN` classes — Tailwind v4's JIT
          // generates color-mix() values for those that don't always
          // apply cleanly to SVG <text> across browsers, leaving labels
          // invisible. Direct `fill="var(--foreground)"` is bulletproof.
          const labelFill = isActive ? '#2ecc71' : 'var(--foreground)'
          const labelFillOpacity = isActive ? 1 : isHover ? 0.95 : 0.7

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
                fill={labelFill}
                fillOpacity={labelFillOpacity}
                fontSize="12"
                fontWeight={isActive ? 700 : 500}
                letterSpacing="0.2"
                style={{ fontFamily: 'inherit' }}
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

              {/* Op dots. Latest dot on the active lane breathes.
                  Active-lane dots also carry their event payload, so
                  we paint the op symbol just above and a 3-char short
                  label just below — answers "what was done in this
                  branch" at a glance without hovering. */}
              {dots.map((dot, j) => {
                const r = isActive ? (dot.isLatest ? 4.5 : 3.5) : 3.0
                const pulse = isActive && dot.isLatest
                const ev = dot.event
                // Hide labels when neighbor dots are closer than ~30px
                // (short_label is up to ~5 chars × ~6px ≈ 30px at fs9).
                // Latest dot always keeps its label so the user can see
                // what was just done. Symbol-only when slightly tighter.
                const prev = j > 0 ? dots[j - 1] : null
                const next = j < dots.length - 1 ? dots[j + 1] : null
                const minGap = Math.min(
                  prev ? dot.x - prev.x : Infinity,
                  next ? next.x - dot.x : Infinity,
                )
                const showLabel = !!ev && (dot.isLatest || minGap >= 30)
                const showSymbol = !!ev && (dot.isLatest || minGap >= 16)
                return (
                  <g key={j}>
                    <circle
                      cx={dot.x}
                      cy={y}
                      r={r}
                      fill={laneColor}
                      fillOpacity={dotOpacity}
                      className={pulse ? 'rh-git-pulse' : undefined}
                      filter={pulse ? 'url(#rh-git-glow)' : undefined}
                    />
                    {showSymbol && ev && (
                      <text
                        x={dot.x}
                        y={y - r - 4}
                        textAnchor="middle"
                        fill="var(--foreground)"
                        fillOpacity={dot.isLatest ? 0.95 : 0.75}
                        fontSize="10"
                        style={{ fontFamily: 'inherit' }}
                      >
                        {symbolForOperation(ev.operation_type)}
                      </text>
                    )}
                    {showLabel && ev && (
                      <text
                        x={dot.x}
                        y={y + r + 9}
                        textAnchor="middle"
                        fill="var(--foreground)"
                        fillOpacity={dot.isLatest ? 0.85 : 0.55}
                        fontSize="9"
                        letterSpacing="0.2"
                        style={{ fontFamily: 'inherit' }}
                      >
                        {shortLabel(ev.operation_type)}
                      </text>
                    )}
                  </g>
                )
              })}

              {/* Op count near the right end (idle lanes only — active
                  shows live dots). Use `events_since_fork` so we show
                  how many ops THIS branch contributed, not the inflated
                  inherited-events total. */}
              {(() => {
                const idleOps = b.events_since_fork ?? b.event_count
                if (isActive || idleOps <= 0) return null
                return (
                  <text
                    x={xEnd - 4}
                    y={y - 6}
                    textAnchor="end"
                    fill="var(--foreground)"
                    fillOpacity={0.55}
                    fontSize="10"
                    style={{ fontFamily: 'inherit' }}
                  >
                    {idleOps} ops
                  </text>
                )
              })()}
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
  // Grouping mode for the strip. `part` = per-part swimlanes (default),
  // `time` = the flat chronological strip. Remembered across sessions so
  // the panel opens the way it was left.
  const [viewMode, setViewMode] = useState<'part' | 'time'>(() => {
    if (typeof window === 'undefined') return 'part'
    return window.localStorage.getItem('roshera.timeline.view') === 'time'
      ? 'time'
      : 'part'
  })
  const setView = useCallback((m: 'part' | 'time') => {
    setViewMode(m)
    try {
      window.localStorage.setItem('roshera.timeline.view', m)
    } catch {
      // localStorage unavailable (private mode / SSR) — in-memory only.
    }
  }, [])
  // Live solid-id → display-name map, from the same scene store the browser
  // tree renders (`obj.name` via `analyticalGeometry.solidId`). Keeps lane
  // labels byte-identical to the tree for live parts — including custom
  // names from create/rename. Memoized on the objects Map reference (the
  // store replaces it on every scene mutation).
  const sceneObjects = useSceneStore((s) => s.objects)
  const liveNames = useMemo(() => {
    const m = new Map<string, string>()
    for (const [, obj] of sceneObjects) {
      const sid = obj.analyticalGeometry?.solidId
      if (sid !== undefined && obj.name) m.set(`solid:${sid}`, obj.name)
    }
    return m
  }, [sceneObjects])
  // Collapse the event body to just the controls row, reclaiming the dock's
  // vertical space for the viewport. Remembered across sessions.
  const [bodyCollapsed, setBodyCollapsed] = useState<boolean>(() => {
    if (typeof window === 'undefined') return false
    return window.localStorage.getItem('roshera.timeline.collapsed') === '1'
  })
  const toggleCollapsed = useCallback(() => {
    setBodyCollapsed((prev) => {
      const next = !prev
      try {
        window.localStorage.setItem('roshera.timeline.collapsed', next ? '1' : '0')
      } catch {
        // localStorage unavailable — in-memory only.
      }
      return next
    })
  }, [])
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
      // Clear events immediately so the strip + active lane don't
      // briefly render the previous branch's history while the new
      // fetch is in flight. fetchHistory will repopulate within ~50ms.
      setEvents([])
      setActiveBranchId(branchId)
    } catch (err) {
      console.error('[timeline] set-active-branch threw:', err)
    }
  }, [activeBranchId])
  const [loading, setLoading] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const [menu, setMenu] = useState<EventContextMenuState | null>(null)
  // Branch-naming picker: opens when the user clicks ⑂. Pre-populated
  // with three suggestions from `/api/branches/name-suggestions` plus
  // a free-text fallback. Closed via Esc, click-outside, or Cancel.
  const [branchPickerOpen, setBranchPickerOpen] = useState(false)
  const [nameSuggestions, setNameSuggestions] = useState<string[]>([])
  const [nameInput, setNameInput] = useState('')
  const [creatingBranch, setCreatingBranch] = useState(false)
  const branchPickerRef = useRef<HTMLDivElement | null>(null)
  const branchInputRef = useRef<HTMLInputElement | null>(null)
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

  const closeBranchPicker = useCallback(() => {
    setBranchPickerOpen(false)
    setNameSuggestions([])
    setNameInput('')
    setCreatingBranch(false)
  }, [])

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
      let branchList: BranchView[] | null = null
      if (branchResp.ok) {
        const data = await branchResp.json()
        if (Array.isArray(data)) {
          branchList = data as BranchView[]
          setBranches(branchList)
        }
      }
      if (histResp.ok) {
        const data = await histResp.json()
        const raw: EventSummary[] = Array.isArray(data)
          ? data
          : data.events && Array.isArray(data.events)
            ? data.events
            : []
        // Filter out events inherited from the parent branch. The
        // history endpoint returns the full lineage (events with
        // sequence ≤ fork_point.event_index were copied from the
        // parent at fork time); the strip should show only events
        // this branch added post-fork. `main` (parent === null)
        // shows everything.
        const active = branchList?.find((b) => b.id === activeBranchId)
        const isRoot = !active || active.parent == null
        const forkIdx = active?.fork_point?.event_index ?? 0
        const filtered = isRoot
          ? raw
          : raw.filter((e) => e.sequence_number > forkIdx)
        setEvents(filtered)
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

  // Dismiss the branch picker on Esc or click-outside. The popover
  // ref is null while closed, which short-circuits the contains-check.
  useEffect(() => {
    if (!branchPickerOpen) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        closeBranchPicker()
      }
    }
    const onPointer = (e: MouseEvent) => {
      const root = branchPickerRef.current
      if (root && !root.contains(e.target as Node)) {
        closeBranchPicker()
      }
    }
    // Defer the pointer listener by one frame so the click that
    // opened the picker doesn't immediately close it.
    const id = window.setTimeout(() => {
      document.addEventListener('mousedown', onPointer)
    }, 0)
    document.addEventListener('keydown', onKey)
    // Focus the input once mounted.
    branchInputRef.current?.focus()
    branchInputRef.current?.select()
    return () => {
      window.clearTimeout(id)
      document.removeEventListener('mousedown', onPointer)
      document.removeEventListener('keydown', onKey)
    }
  }, [branchPickerOpen, closeBranchPicker])

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
    // Open the picker. We always fetch fresh suggestions because the
    // backend's pool is pseudo-randomized per request — a stale set
    // from minutes ago would feel un-alive.
    if (branchPickerOpen) {
      closeBranchPicker()
      return
    }
    setBranchPickerOpen(true)
    setCreatingBranch(false)
    setNameSuggestions([])
    setNameInput('')
    try {
      const resp = await fetch('/api/branches/name-suggestions?count=3')
      if (resp.ok) {
        const data = (await resp.json()) as { names?: string[] }
        const names = Array.isArray(data.names) ? data.names : []
        setNameSuggestions(names)
        // Leave `nameInput` empty so the user still has to actively
        // pick a chip or type a name — pre-filling the input made
        // the first suggestion look like a branch already existed.
      }
    } catch {
      // Backend not running — picker still works as a free-text input.
    }
  }

  const submitBranchCreate = async () => {
    const name = nameInput.trim()
    if (!name) return
    // Hit `/api/branches` (timeline-backed) rather than the legacy
    // `/api/timeline/branch/create` route — the latter writes into a
    // separate `branch_manager` store that `GET /api/branches` (used
    // by the overview minimap) does not read from, so its branches
    // never show up in the UI.
    setCreatingBranch(true)
    try {
      const resp = await fetch('/api/branches', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name,
          parent: activeBranchId,
          description: 'manual branch from timeline strip',
        }),
      })
      if (!resp.ok) {
        console.error('[timeline] branch create failed:', resp.status)
        setCreatingBranch(false)
        return
      }
      // Creating a branch does NOT make it the recording target; the
      // backend keeps the old active branch until `/api/branches/active`
      // is hit. Without this swap, the very next primitive the user
      // adds would still land on the parent branch — the exact bug the
      // user just hit. Pull the new branch's id out of the response
      // and activate it before refreshing history.
      const created = (await resp.json()) as { id?: string }
      if (created.id) {
        const activeResp = await fetch('/api/branches/active', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ branch_id: created.id }),
        })
        if (activeResp.ok) {
          setEvents([])
          setActiveBranchId(created.id)
        } else {
          console.error('[timeline] activate-after-create failed:', activeResp.status)
        }
      }
      closeBranchPicker()
      fetchHistory()
    } catch (err) {
      console.error('[timeline] branch threw:', err)
      setCreatingBranch(false)
    }
  }

  const handleMerge = async () => {
    if (activeBranchId === MAIN_BRANCH_ID) return
    const sourceName = activeBranch?.name || 'this branch'
    const ok = window.confirm(
      `Merge "${sourceName}" into main?\n\nMain will fast-forward to include every event added on this branch.`,
    )
    if (!ok) return
    try {
      const resp = await fetch(
        `/api/branches/${encodeURIComponent(activeBranchId)}/merge`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ target: 'main', strategy: 'fast-forward' }),
        },
      )
      if (!resp.ok) {
        const body = await resp.text().catch(() => '')
        console.error('[timeline] merge failed:', resp.status, body)
        window.alert(
          `Merge failed (${resp.status}). The backend may have rejected a non-fast-forward merge — try squashing instead.`,
        )
        return
      }
      const result = (await resp.json()) as {
        success: boolean
        merged_into: string
        conflicts: string[]
      }
      if (!result.success) {
        window.alert(
          `Merge produced conflicts:\n\n${result.conflicts.join('\n') || 'unknown conflict'}`,
        )
        return
      }
      // Switch the active recording target back to main so the next
      // primitive lands on the merged trunk, not the now-merged source.
      const activeResp = await fetch('/api/branches/active', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ branch_id: MAIN_BRANCH_ID }),
      })
      if (activeResp.ok) {
        setEvents([])
        setActiveBranchId(MAIN_BRANCH_ID)
      }
      fetchHistory()
    } catch (err) {
      console.error('[timeline] merge threw:', err)
    }
  }

  const activeBranch = branches.find((b) => b.id === activeBranchId)
  const activeBranchLabel = activeBranch
    ? activeBranch.name || (activeBranch.id === MAIN_BRANCH_ID ? 'main' : '…')
    : 'main'
  // Match the minimap's filter — abandoned branches are hidden
  // there, so the "+N" indicator should not count them either.
  const visibleBranchCount = branches.filter(
    (b) => b.state !== 'abandoned' && b.state !== 'merged',
  ).length
  const otherBranchCount = Math.max(0, visibleBranchCount - 1)

  return (
    <div className="relative font-mono flex flex-col border-t border-border bg-card shrink-0">
      {branchPickerOpen && (
        <div
          ref={branchPickerRef}
          className="absolute left-3 bottom-full mb-1 z-50 flex flex-col gap-2 px-3 py-2 rounded-md border border-border bg-card shadow-lg text-[12px]"
          style={{ minWidth: 280 }}
        >
          <div className="text-foreground/70">
            new branch from{' '}
            <span className="text-foreground/90">{activeBranchLabel}</span>
          </div>
          {nameSuggestions.length > 0 && (
            <div className="flex items-center gap-1 flex-wrap">
              {nameSuggestions.map((s) => (
                <button
                  key={s}
                  type="button"
                  onClick={() => {
                    setNameInput(s)
                    branchInputRef.current?.focus()
                  }}
                  className={
                    'px-2 py-0.5 rounded border transition-colors ' +
                    (nameInput === s
                      ? 'border-[#2ecc71] text-foreground bg-[#2ecc71]/15'
                      : 'border-border text-foreground/80 hover:bg-accent/40')
                  }
                >
                  {s}
                </button>
              ))}
            </div>
          )}
          <div className="flex items-center gap-1.5">
            <input
              ref={branchInputRef}
              type="text"
              value={nameInput}
              onChange={(e) => setNameInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void submitBranchCreate()
                }
              }}
              placeholder="branch name"
              className="flex-1 min-w-0 px-2 py-1 rounded border border-border bg-background text-foreground placeholder:text-foreground/40 outline-none focus:border-[#2ecc71]"
            />
            <button
              type="button"
              onClick={() => void submitBranchCreate()}
              disabled={creatingBranch || !nameInput.trim()}
              className="px-2 py-1 rounded text-foreground bg-[#2ecc71]/20 hover:bg-[#2ecc71]/30 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {creatingBranch ? 'creating…' : 'create'}
            </button>
            <button
              type="button"
              onClick={closeBranchPicker}
              className="px-2 py-1 rounded text-foreground/60 hover:bg-accent/40"
            >
              cancel
            </button>
          </div>
        </div>
      )}
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

      {/* Controls row: title + glyph actions, grouping toggle, branch chip */}
      <div className="flex items-center gap-2 px-3 py-1.5">
        <div className="flex items-center gap-0.5 shrink-0 text-[13px]">
          <button
            type="button"
            onClick={toggleCollapsed}
            title={bodyCollapsed ? 'Expand timeline' : 'Collapse timeline'}
            aria-label={bodyCollapsed ? 'Expand timeline' : 'Collapse timeline'}
            aria-expanded={!bodyCollapsed}
            className="text-foreground/55 hover:text-foreground transition-colors text-[11px] leading-none px-0.5"
          >
            {bodyCollapsed ? '▸' : '▾'}
          </button>
          <span className="text-foreground/80 mr-1">timeline</span>
          <HeaderButton onClick={handleUndo} title="Undo (Ctrl+Z)" ariaLabel="Undo">↶</HeaderButton>
          <HeaderButton onClick={handleRedo} title="Redo (Ctrl+Shift+Z)" ariaLabel="Redo">↷</HeaderButton>
          <HeaderButton onClick={handleCheckpoint} title="Checkpoint" ariaLabel="Create checkpoint">◈</HeaderButton>
          <HeaderButton onClick={handleBranch} title="Branch" ariaLabel="Create branch">⑂</HeaderButton>
          <HeaderButton
            onClick={handleMerge}
            title={
              activeBranchId === MAIN_BRANCH_ID
                ? 'Merge into main (active branch is already main)'
                : `Merge ${activeBranchLabel} into main`
            }
            ariaLabel="Merge into main"
            disabled={activeBranchId === MAIN_BRANCH_ID}
          >⊕</HeaderButton>
        </div>

        <span className="text-muted-foreground/40 shrink-0 text-[13px]">│</span>

        {/* Grouping toggle: per-part swimlanes (default) vs flat by-time */}
        <div
          className="inline-flex shrink-0 rounded border border-border overflow-hidden text-[11px]"
          role="group"
          aria-label="Timeline grouping"
        >
          <button
            type="button"
            onClick={() => setView('part')}
            aria-pressed={viewMode === 'part'}
            title="Group operations into per-part lanes"
            className={cn(
              'px-2 py-0.5 transition-colors',
              viewMode === 'part'
                ? 'bg-accent/40 text-foreground font-medium'
                : 'text-muted-foreground hover:text-foreground hover:bg-accent/30',
            )}
          >
            by&nbsp;part
          </button>
          <button
            type="button"
            onClick={() => setView('time')}
            aria-pressed={viewMode === 'time'}
            title="Show all parts in one chronological strip"
            className={cn(
              'px-2 py-0.5 transition-colors border-l border-border',
              viewMode === 'time'
                ? 'bg-accent/40 text-foreground font-medium'
                : 'text-muted-foreground hover:text-foreground hover:bg-accent/30',
            )}
          >
            by&nbsp;time
          </button>
        </div>

        <div className="flex-1" />

        {/* Branch chip + expand/collapse toggle for the branch graph. */}
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

      {/* Event body: per-part swimlanes (default) or the flat by-time strip.
          Collapsible to reclaim viewport space; height-capped with internal
          scroll so it never crowds the 3D view. */}
      {!bodyCollapsed && (
        <div className="border-t border-border/40">
          {viewMode === 'part' ? (
            <Swimlanes
              events={events}
              liveNames={liveNames}
              now={now}
              onContextMenu={handleEventContextMenu}
              loading={loading}
            />
          ) : (
            <FlatStrip
              events={events}
              now={now}
              onContextMenu={handleEventContextMenu}
              loading={loading}
            />
          )}
        </div>
      )}

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
              } else {
                // Truncate can cascade-abandon child branches whose
                // fork point landed in the discarded range. If the
                // user was sitting on one of those branches, keeping
                // them there leaves the recorder writing to a dead
                // branch. Snap back to main so subsequent ops have a
                // valid home and the strip reflects "the new world".
                if (activeBranchId !== MAIN_BRANCH_ID) {
                  try {
                    const activeResp = await fetch('/api/branches/active', {
                      method: 'POST',
                      headers: { 'Content-Type': 'application/json' },
                      body: JSON.stringify({ branch_id: MAIN_BRANCH_ID }),
                    })
                    if (activeResp.ok) {
                      setEvents([])
                      setActiveBranchId(MAIN_BRANCH_ID)
                    }
                  } catch {
                    // Best-effort; the next fetchHistory will refresh.
                  }
                }
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
