/**
 * Timeline map view — the timeline as a route, not an odometer (see vault
 * `Research/2026-07-31-ui-pass-spec.md` §3).
 *
 * A SECOND view alongside the existing `Timeline.tsx` strip (scrubbing
 * still lives there; this is for reading structure). Lazy-loaded from
 * `Timeline.tsx` — `@xyflow/react` never enters the initial bundle
 * unless the user actually opens this panel. Layout is a hand-rolled
 * wrapped grid (each branch's groups flow left→right and wrap into
 * rows, like text); dagre was dropped when the one-rank-per-group
 * chain proved unreadable at real history sizes.
 *
 * ── Honesty constraint (read before touching the grouping logic) ──────
 * The agent does not yet declare intent before executing (that work is
 * queued — see the spec). So a node here is NOT "bolt circle, 8×⌀18";
 * it is the real thing the data supports today: a CONTIGUOUS run of
 * operations of the SAME kind. Eight `Cyl` cuts in a row collapse into
 * one "Cyl ×8" card — honest about being a grouping, not a guessed name.
 *
 * Grouping by `affected_parts` (the first attempt here) was tried and
 * reverted after checking it against the live durability document: this
 * kernel mints a FRESH solid id on every mutating op — including a
 * boolean's *result* — so "same part" almost never holds across two
 * consecutive events even when a human would call them one continuous
 * piece of work. Grouping by operation kind is the signal that actually
 * collapses a real CSG build (13 `Cyl` creates in a row really do render
 * as one card, verified live). When intents land on the timeline, this
 * is where a real name replaces the `${kind} ×N` heading; nothing below
 * infers one early.
 *
 * Certificate coloring (spec's "terrain = certificate state") is NOT
 * implemented: `EventCertificate` (timeline-engine/src/event_certificate.rs)
 * has no production call site yet — every event's certificate is
 * `null` with an honest `certificate_absent_reason`. Coloring nodes by
 * a field that is uniformly absent would either paint everything the
 * same non-color (pointless) or invite someone to read meaning into a
 * placeholder. The panel says so explicitly instead of pretending.
 *
 * ── Branches (Varun: "show me branched timeline too") ──────────────────
 * Branches are the centrepiece, not a footnote: each branch renders as its
 * own LANE (a tinted band spanning the full width, with its name + state
 * pinned at the left), fork points drop as elbows from the parent lane,
 * and merged/abandoned lanes read distinctly (dashed, dimmed). Only real
 * `state`/`fork_point` data drives this — an empty branch list renders an
 * honest single `main` lane, never a fabricated fork.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  Handle,
  Position,
  type Node,
  type Edge,
  type NodeProps,
  type ReactFlowInstance,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { X } from 'lucide-react'
import { useThemeStore } from '@/stores/theme-store'
import {
  type EventSummary,
  type CheckpointSummary,
  type DurabilityStatus,
  checkpointCovering,
  durabilityNotice,
  shortLabel,
  symbolForOperation,
  formatTimestamp,
} from '@/lib/timeline-events'
import { DurabilityChip } from './TimelineDecisions'

// ─── Minimal branch shape this view needs (mirrors `BranchView` in
// Timeline.tsx — duplicated rather than imported so this module stays
// independently lazy-loadable and never pulls the strip's internals). ──

export interface GraphBranch {
  id: string
  name: string
  parent: string | null
  state: string
  event_count: number
  events_since_fork?: number
  created_at: string
  fork_point?: { branch_id: string; event_index: number; timestamp: string }
}

const MAIN_BRANCH_ID = '00000000-0000-0000-0000-000000000000'

// ─── Grouping: contiguous same-operation-kind runs (the honest grouping) ──

interface EventGroup {
  /** Common `shortLabel` of every event in this run, e.g. "Cyl". */
  key: string
  events: EventSummary[]
  /** The DECLARED intent covering this run — a checkpoint name, read
   *  off the real checkpoint list, never inferred. `undefined` when no
   *  checkpoint covers the run (the common case today: checkpoints are
   *  volatile and usually absent). Attached only on root-branch runs —
   *  checkpoint `event_range`s index the main timeline's sequences, so
   *  applying them to a child branch's post-fork sequences would
   *  mislabel spans that merely share numbers. */
  intent?: string
}

/** Group events into contiguous runs of the same operation kind — a run
 *  breaks the moment the kind changes, so 13 `Cyl` creates in a row
 *  become one "Cyl ×13" card and a `Cyl` between two `Bool`s honestly
 *  stays its own single-op card. */
function groupContiguous(events: EventSummary[]): EventGroup[] {
  const groups: EventGroup[] = []
  for (const ev of events) {
    const key = shortLabel(ev.operation_type)
    const last = groups[groups.length - 1]
    if (last && last.key === key) {
      last.events.push(ev)
    } else {
      groups.push({ key, events: [ev] })
    }
  }
  return groups
}

/** The part this run left behind — read straight off the LAST event's
 *  `affected_parts` (its real, recorded output), never inferred across
 *  the run. `null` for non-geometry events (sketch/drawing/session). */
function resultPartLabel(ev: EventSummary, liveNames: Map<string, string>): string | null {
  const parts = ev.affected_parts
  if (!parts || parts.length === 0) return null
  const key = parts[0]
  const live = liveNames.get(key)
  if (live) return live
  const m = key.match(/^solid:(.+)$/)
  return m ? `solid_${m[1]}` : key
}

// ─── Branch colour (the lane's identity — active / abandoned / merged / other) ──

function strokeColorFor(isActive: boolean, state: string): string {
  if (isActive) return '#2ecc71'
  if (state === 'abandoned') return 'rgba(251,146,60,0.9)'
  if (state === 'merged') return '#94a3b8'
  return '#7c8aa5'
}

function laneFillFor(isActive: boolean, state: string): string {
  if (isActive) return 'rgba(46,204,113,0.09)'
  if (state === 'abandoned') return 'rgba(251,146,60,0.08)'
  if (state === 'merged') return 'rgba(148,163,184,0.06)'
  return 'rgba(124,138,165,0.07)'
}

// ─── Operation family — the "one look and it's clear" vocabulary ────────
// Seeded from the existing glyph vocabulary (`symbolForOperation`/
// `shortLabel` in `lib/timeline-events.ts`) rather than inventing new
// icons: a boolean is still `⊕`, a cylinder still `⊟`. What's new here is
// SHAPE + COLOUR per family, so the eye sorts nodes before it reads them.

type OpFamily = 'create' | 'boolean' | 'blend' | 'sketch' | 'transform' | 'delete' | 'other'

function familyOf(key: string): OpFamily {
  switch (key) {
    case 'Box': case 'Sph': case 'Cyl': case 'Con': case 'Tor': case 'Pt': case 'Lin':
    case 'Cir': case 'Rec': case 'Pln': case 'Ext': case 'Rev': case 'Swp': case 'Lft':
      return 'create'
    case 'Bool': case 'Un': case 'Int': case 'Df':
      return 'boolean'
    case 'Fil': case 'Cha':
      return 'blend'
    case 'Tr':
      return 'transform'
    case 'Del':
      return 'delete'
    default:
      // Sketch/drawing kinds fall through the fixed switch above as their
      // raw lowercase-prefix fallback ("sket", "draw") — matched here
      // rather than listed above so a future real kind never silently
      // collapses into "other" without a glance at this function.
      if (key === 'sket' || key === 'draw') return 'sketch'
      return 'other'
  }
}

const FAMILY_COLOR: Record<OpFamily, string> = {
  create: '#2563eb',
  boolean: '#7c3aed',
  blend: '#d97706',
  sketch: '#0891b2',
  transform: '#db2777',
  delete: '#dc2626',
  other: '#64748b',
}

/** Boolean → hexagon (union of shapes); transform → parallelogram (a
 *  push/shift); blend handled via `borderRadius` instead (a "smoothed"
 *  pill reads better than a clipped hex at this size); everything else
 *  stays a plain rounded card. `clip-path` only changes the outer
 *  silhouette — it never clips the text inside, so padding is widened
 *  for the two shaped families to keep labels off the taper. */
function familyShapeStyle(family: OpFamily): React.CSSProperties {
  switch (family) {
    case 'boolean':
      return { clipPath: 'polygon(9% 0%, 91% 0%, 100% 50%, 91% 100%, 9% 100%, 0% 50%)' }
    case 'transform':
      return { clipPath: 'polygon(7% 0%, 100% 0%, 93% 100%, 0% 100%)' }
    case 'blend':
      return { borderRadius: 22 }
    default:
      return {}
  }
}

function familyExtraPaddingX(family: OpFamily): string {
  return family === 'boolean' || family === 'transform' ? 'px-4' : 'px-2.5'
}

// ─── Custom node: a real card, not a dot ─────────────────────────────

interface GraphNodeData extends Record<string, unknown> {
  branch: GraphBranch
  group: EventGroup
  isActiveBranch: boolean
  expanded: boolean
  liveNames: Map<string, string>
  onToggle: (id: string) => void
}

const NODE_WIDTH = 190
const COLLAPSED_HEIGHT = 54
const EXPANDED_HEIGHT = 188
/** Extra card height when a declared-intent overline is present, so
 *  dagre reserves real room for the third line instead of letting the
 *  card visually overflow its lane slot. */
const INTENT_LINE_HEIGHT = 15

function groupHeight(group: EventGroup, expanded: boolean): number {
  const base = expanded ? EXPANDED_HEIGHT : COLLAPSED_HEIGHT
  return base + (group.intent ? INTENT_LINE_HEIGHT : 0)
}

/** Everything that used to be printed on the card and now lives in the
 *  hover tooltip instead — branch, time range, ids. Progressive
 *  disclosure: the card is for scanning, the title/expansion is for
 *  reading (Varun: "make it polished — maximum 2 lines"). */
function nodeTitle(branch: GraphBranch, group: EventGroup, resultPart: string | null): string {
  const first = group.events[0]
  const last = group.events[group.events.length - 1]
  const lines = [
    group.intent ? `declared intent: ${group.intent}` : '',
    `${branch.name || 'main'} · ${group.key} ×${group.events.length}`,
    first && last ? `${formatTimestamp(first.timestamp)} – ${formatTimestamp(last.timestamp)}` : '',
    resultPart ? `result: ${resultPart}` : '',
  ]
  return lines.filter(Boolean).join('\n')
}

function IntentNode({ id, data }: NodeProps<Node<GraphNodeData>>) {
  const { branch, group, isActiveBranch, expanded, liveNames, onToggle } = data
  const branchStroke = strokeColorFor(isActiveBranch, branch.state)
  const family = familyOf(group.key)
  const familyColor = FAMILY_COLOR[family]
  const first = group.events[0]
  const last = group.events[group.events.length - 1]
  const resultPart = last ? resultPartLabel(last, liveNames) : null
  // Line 2: the ONE detail that matters — the result reference when the
  // op produced one, otherwise the count. Never both.
  const detailLine = resultPart ?? `${group.events.length} op${group.events.length === 1 ? '' : 's'}`

  return (
    <div
      title={nodeTitle(branch, group, resultPart)}
      className="shadow-sm text-foreground"
      style={{
        width: NODE_WIDTH,
        minHeight: COLLAPSED_HEIGHT,
        background: 'var(--card)',
        borderWidth: isActiveBranch ? 1.5 : 1,
        borderStyle: branch.state === 'merged' ? 'dashed' : 'solid',
        borderColor: branchStroke,
        borderRadius: 8,
        opacity: branch.state === 'merged' ? 0.6 : 1,
        borderLeftWidth: 5,
        borderLeftColor: familyColor,
        ...familyShapeStyle(family),
      }}
    >
      <Handle type="target" position={Position.Left} style={{ background: branchStroke, opacity: 0.6 }} />
      <div className={`${familyExtraPaddingX(family)} py-1.5`}>
        {/* Line 0 — the DECLARED intent, when a real checkpoint covers
            this run. This is the "named intent replaces the guessed
            heading" moment promised in the module comment: the name is
            read off the checkpoint list, never inferred. Neutral text —
            colour stays reserved for state. */}
        {group.intent && (
          <div className="flex items-center gap-1 text-[10px] leading-tight text-foreground/80 mb-0.5 min-w-0">
            <span aria-hidden className="shrink-0 text-foreground/60">◈</span>
            <span className="truncate font-medium">{group.intent}</span>
          </div>
        )}
        {/* Line 1 — what it is: glyph + the honest grouping label. */}
        <div className="flex items-center justify-between gap-1 min-w-0">
          <span className="flex items-center gap-1.5 min-w-0 text-[12.5px] font-medium truncate">
            <span aria-hidden style={{ color: familyColor }} className="text-[14px] leading-none shrink-0">
              {symbolForOperation(first?.operation_type ?? '')}
            </span>
            <span className="truncate">
              {group.key}
              {group.events.length > 1 && (
                <span className="text-muted-foreground font-normal"> ×{group.events.length}</span>
              )}
            </span>
          </span>
          <button
            type="button"
            onClick={() => onToggle(id)}
            className="nodrag shrink-0 text-muted-foreground hover:text-foreground text-[10px] px-1"
            title={expanded ? 'Fold to summary' : 'Unfold to the raw operations'}
            aria-label={expanded ? 'Fold group' : 'Unfold group'}
          >
            {expanded ? '▾' : '▸'}
          </button>
        </div>
        {/* Line 2 — the one detail that matters: result ref OR count. */}
        <div className="text-[10.5px] text-muted-foreground/80 truncate mt-0.5">
          {resultPart ? `→ ${detailLine}` : detailLine}
        </div>
        {expanded && (
          <div className="mt-1.5 pt-1.5 border-t border-border/40 max-h-[128px] overflow-y-auto space-y-0.5 nodrag nowheel">
            {group.events.map((ev) => (
              <div key={ev.id} className="flex items-center gap-1.5 text-[10px] leading-tight">
                <span className="shrink-0">{symbolForOperation(ev.operation_type)}</span>
                <span className="truncate">{shortLabel(ev.operation_type)}</span>
                <span className="ml-auto text-muted-foreground/60 shrink-0">
                  {formatTimestamp(ev.timestamp)}
                </span>
              </div>
            ))}
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Right} style={{ background: branchStroke, opacity: 0.6 }} />
    </div>
  )
}

// ─── Lane background: the branch AS the road, not a footnote ────────────

interface LaneNodeData extends Record<string, unknown> {
  label: string
  state: string
  isActive: boolean
  width: number
  height: number
}

function LaneNode({ data }: NodeProps<Node<LaneNodeData>>) {
  const stroke = strokeColorFor(data.isActive, data.state)
  const fill = laneFillFor(data.isActive, data.state)
  return (
    <div
      className="pointer-events-none rounded-md"
      style={{
        width: data.width,
        height: data.height,
        background: fill,
        borderLeft: `3px solid ${stroke}`,
        borderTop: data.state === 'abandoned' ? `1px dashed ${stroke}` : undefined,
        borderBottom: data.state === 'abandoned' ? `1px dashed ${stroke}` : undefined,
      }}
    >
      <div
        className="inline-flex items-center gap-1.5 ml-2 mt-1.5 px-2 py-0.5 rounded"
        style={{ background: 'var(--card)', border: `1px solid ${stroke}` }}
      >
        <span aria-hidden className="w-1.5 h-1.5 rounded-full shrink-0" style={{ backgroundColor: stroke }} />
        <span className="text-[11px] font-semibold whitespace-nowrap" style={{ color: stroke }}>
          {data.label}
        </span>
        {data.isActive && (
          <span className="text-[9px] uppercase tracking-wide text-muted-foreground">recording</span>
        )}
        {data.state === 'merged' && (
          <span className="text-[9px] uppercase tracking-wide text-muted-foreground">merged</span>
        )}
        {data.state === 'abandoned' && (
          <span className="text-[9px] uppercase tracking-wide" style={{ color: stroke }}>abandoned</span>
        )}
      </div>
    </div>
  )
}

// ─── Stub node: an honestly-empty branch — a fork with nothing recorded
// on it yet. `bolt-circle-8x` forked from main and has 0 events right
// now; that is a real, meaningful state ("a road not yet travelled"),
// not a placeholder to hide — the honesty constraint cuts both ways: no
// inventing an op that didn't happen, but also no hiding a branch that
// genuinely exists just because it's quiet so far.
const STUB_WIDTH = 168
const STUB_HEIGHT = 40

interface StubNodeData extends Record<string, unknown> {
  branch: GraphBranch
  isActive: boolean
}

function StubNode({ data }: NodeProps<Node<StubNodeData>>) {
  const stroke = strokeColorFor(data.isActive, data.branch.state)
  return (
    <div
      title={`${data.branch.name} — forked from main, no operations recorded on it yet`}
      className="flex items-center justify-center text-[10.5px] italic text-muted-foreground/70"
      style={{
        width: STUB_WIDTH,
        height: STUB_HEIGHT,
        borderRadius: 8,
        border: `1px dashed ${stroke}`,
        background: 'var(--card)',
      }}
    >
      <Handle type="target" position={Position.Left} style={{ background: stroke, opacity: 0.6 }} />
      no ops yet
    </div>
  )
}

const nodeTypes = { intent: IntentNode, lane: LaneNode, stub: StubNode }

// ─── Layout: dagre lays out the DAG left-to-right (the "road") ──────

const LANE_PAD_Y = 40
const LANE_PAD_X = 48
// ── Wrapped-grid layout constants ───────────────────────────────────
// A branch's groups flow left→right and WRAP into rows, like text —
// not one infinite straight line. The previous dagre chain put every
// group on its own rank, so a real 99-op history rendered as a single
// line of boxes running off-screen (Varun, live 2026-08-01: "i only
// see one straight line.. multiple boxes ... unable to make out what
// is happening"). A wrapped path needs no layout solver, so dagre is
// gone from this chunk entirely.
const WRAP_COLS = 6
const GAP_X = 64
const GAP_Y = 44
/** Vertical clearance between one branch's band and the next. */
const BAND_GAP = LANE_PAD_Y * 2 + 36

function buildGraph(
  branchGroups: { branch: GraphBranch; groups: EventGroup[] }[],
  activeBranchId: string,
  expandedIds: Set<string>,
  liveNames: Map<string, string>,
  onToggle: (id: string) => void,
): { nodes: Node<GraphNodeData | LaneNodeData | StubNodeData>[]; edges: Edge[] } {
  // ── Wrapped-grid layout + per-branch Y bounds (for the lane bands) ──
  const contentNodes: Node<GraphNodeData | StubNodeData>[] = []
  const placed = new Set<string>()
  let globalMinX = Infinity
  let globalMaxX = -Infinity
  const laneBounds = new Map<string, { minY: number; maxY: number }>()

  const noteExtent = (branchId: string, left: number, top: number, w: number, h: number) => {
    globalMinX = Math.min(globalMinX, left)
    globalMaxX = Math.max(globalMaxX, left + w)
    const bounds = laneBounds.get(branchId)
    if (bounds) {
      bounds.minY = Math.min(bounds.minY, top)
      bounds.maxY = Math.max(bounds.maxY, top + h)
    } else {
      laneBounds.set(branchId, { minY: top, maxY: top + h })
    }
  }

  let yCursor = 0
  for (const { branch, groups } of branchGroups) {
    // A non-root branch with ZERO events still gets a node — a real fork
    // with nothing recorded on it yet is honest structure, not something
    // to hide (see StubNode's comment).
    if (groups.length === 0) {
      if (!branch.parent) continue
      const id = `${branch.id}:0`
      contentNodes.push({
        id,
        type: 'stub',
        position: { x: 0, y: yCursor },
        draggable: false,
        data: { branch, isActive: branch.id === activeBranchId },
      })
      placed.add(id)
      noteExtent(branch.id, 0, yCursor, STUB_WIDTH, STUB_HEIGHT)
      yCursor += STUB_HEIGHT + BAND_GAP
      continue
    }

    // Row heights first (a row is as tall as its tallest card — an
    // expanded card grows its whole row, not just itself).
    const rowHeights: number[] = []
    groups.forEach((group, i) => {
      const row = Math.floor(i / WRAP_COLS)
      const h = groupHeight(group, expandedIds.has(`${branch.id}:${i}`))
      rowHeights[row] = Math.max(rowHeights[row] ?? 0, h)
    })
    const rowTop = (row: number): number =>
      yCursor + rowHeights.slice(0, row).reduce((a, b) => a + b, 0) + row * GAP_Y

    groups.forEach((group, i) => {
      const id = `${branch.id}:${i}`
      const col = i % WRAP_COLS
      const row = Math.floor(i / WRAP_COLS)
      const h = groupHeight(group, expandedIds.has(id))
      const left = col * (NODE_WIDTH + GAP_X)
      const top = rowTop(row) + (rowHeights[row] - h) / 2
      contentNodes.push({
        id,
        type: 'intent',
        position: { x: left, y: top },
        data: {
          branch,
          group,
          isActiveBranch: branch.id === activeBranchId,
          expanded: expandedIds.has(id),
          liveNames,
          onToggle,
        },
      })
      placed.add(id)
      noteExtent(branch.id, left, top, NODE_WIDTH, h)
    })

    const lastRow = rowHeights.length - 1
    yCursor = rowTop(lastRow) + rowHeights[lastRow] + BAND_GAP
  }

  // Fork edges: parent's group covering the fork point → child's first
  // group (or its stub, when the child has recorded nothing yet).
  const forkEdges: { source: string; target: string; branch: GraphBranch }[] = []
  for (const { branch } of branchGroups) {
    if (!branch.parent) continue
    const parentEntry = branchGroups.find((bg) => bg.branch.id === branch.parent)
    if (!parentEntry || parentEntry.groups.length === 0) continue
    const forkIdx = branch.fork_point?.event_index ?? 0
    let sourceGroupIdx = 0
    parentEntry.groups.forEach((grp, i) => {
      if (grp.events.some((e) => e.sequence_number <= forkIdx)) sourceGroupIdx = i
    })
    const sourceId = `${parentEntry.branch.id}:${sourceGroupIdx}`
    const targetId = `${branch.id}:0`
    if (placed.has(sourceId) && placed.has(targetId)) {
      forkEdges.push({ source: sourceId, target: targetId, branch })
    }
  }

  const laneNodes: Node<LaneNodeData>[] = []
  if (isFinite(globalMinX)) {
    for (const { branch } of branchGroups) {
      const bounds = laneBounds.get(branch.id)
      if (!bounds) continue
      laneNodes.push({
        id: `lane:${branch.id}`,
        type: 'lane',
        position: { x: globalMinX - LANE_PAD_X, y: bounds.minY - LANE_PAD_Y },
        draggable: false,
        selectable: false,
        focusable: false,
        zIndex: -1,
        data: {
          label: branch.name || (branch.id === MAIN_BRANCH_ID ? 'main' : branch.id.slice(0, 8)),
          state: branch.state,
          isActive: branch.id === activeBranchId,
          width: globalMaxX - globalMinX + LANE_PAD_X * 2,
          height: bounds.maxY - bounds.minY + LANE_PAD_Y * 2,
        },
      })
    }
  }

  const edges: Edge[] = []
  for (const { branch, groups } of branchGroups) {
    const color = strokeColorFor(branch.id === activeBranchId, branch.state)
    for (let i = 1; i < groups.length; i++) {
      // A wrap edge (row end → next row start) travels right-to-left;
      // smoothstep routes it around the cards instead of through them.
      const isWrap = i % WRAP_COLS === 0
      edges.push({
        id: `seq:${branch.id}:${i - 1}->${i}`,
        source: `${branch.id}:${i - 1}`,
        target: `${branch.id}:${i}`,
        type: 'smoothstep',
        style: {
          stroke: color,
          strokeWidth: branch.id === activeBranchId ? 1.6 : 1,
          strokeOpacity: isWrap ? 0.55 : 1,
          strokeDasharray: branch.state === 'merged' ? '4 3' : undefined,
        },
      })
    }
  }
  for (const fe of forkEdges) {
    edges.push({
      id: `fork:${fe.source}->${fe.target}`,
      source: fe.source,
      target: fe.target,
      type: 'smoothstep',
      label: fe.branch.name || undefined,
      labelStyle: { fontSize: 10, fill: 'var(--foreground)' },
      style: {
        stroke: strokeColorFor(fe.branch.id === activeBranchId, fe.branch.state),
        strokeDasharray: '3 3',
        strokeWidth: 1,
      },
    })
  }

  // Lanes FIRST so they paint behind the intent/stub cards (React Flow
  // stacks by array order; `zIndex: -1` above reinforces it).
  return { nodes: [...laneNodes, ...contentNodes], edges }
}

// ─── Panel ────────────────────────────────────────────────────────────

interface FetchedBranch {
  branch: GraphBranch
  events: EventSummary[]
  /** True when the fetch itself failed (non-2xx or network) — zero
   *  events from a REFUSED fetch is not the same fact as a genuinely
   *  empty branch, and the empty state must not conflate them (this
   *  panel rendered "no operations recorded yet" over a rate-limited
   *  429 with 100 real ops behind it — caught live 2026-08-01). */
  failed: boolean
}

async function fetchBranchEvents(branch: GraphBranch): Promise<FetchedBranch> {
  try {
    const resp = await fetch(`/api/timeline/history/${branch.id}`)
    if (!resp.ok) return { branch, events: [], failed: true }
    const data = await resp.json()
    const raw: EventSummary[] = Array.isArray(data) ? data : (data.events ?? [])
    const isRoot = branch.parent == null
    const forkIdx = branch.fork_point?.event_index ?? 0
    const events = isRoot ? raw : raw.filter((e) => e.sequence_number > forkIdx)
    return { branch, events, failed: false }
  } catch {
    return { branch, events: [], failed: true }
  }
}

export default function TimelineGraph({
  branches,
  activeBranchId,
  liveNames,
  checkpoints,
  durability,
  onClose,
}: {
  branches: GraphBranch[]
  activeBranchId: string
  liveNames: Map<string, string>
  /** Named design states (real checkpoints; usually empty — volatile). */
  checkpoints: CheckpointSummary[]
  /** Durability boot outcome — quarantine is disclosed in the header. */
  durability: DurabilityStatus | null
  onClose: () => void
}) {
  const [fetched, setFetched] = useState<FetchedBranch[]>([])
  const [loading, setLoading] = useState(true)
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set())
  const [rfInstance, setRfInstance] = useState<ReactFlowInstance | null>(null)
  const canvasRef = useRef<HTMLDivElement | null>(null)
  // Floor for how far the user can zoom out — set from the fitted zoom
  // once the graph is measured, so "zoom out" can never land on an empty
  // field (Varun: "cant be completely zoomed out to be able to see
  // nothing"). Generous default until that measurement lands.
  const [minZoom, setMinZoom] = useState(0.5)
  const theme = useThemeStore((s) => s.theme)
  // The honesty paragraph is real and stays, but not as seven lines above
  // the thing the user came to look at (Varun: "it takes 10 secs to read
  // what it does" — the same defect as the provider dialog). Collapsed by
  // default; the ⓘ expands it in place.
  const [detailsOpen, setDetailsOpen] = useState(false)

  // The parent's `branches` state can legitimately be EMPTY when the
  // map opens (the /api/branches poll may be rate-limited or still in
  // flight after a remount) — but `main` always exists, so fall back to
  // it rather than mapping nothing over a document with real history.
  const effectiveBranches = useMemo<GraphBranch[]>(
    () =>
      branches.length > 0
        ? branches
        : [{
            id: MAIN_BRANCH_ID,
            name: 'main',
            parent: null,
            state: 'active',
            event_count: 0,
            created_at: '',
          }],
    [branches],
  )

  const [loadEpoch, setLoadEpoch] = useState(0)
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    Promise.all(effectiveBranches.map(fetchBranchEvents)).then((results) => {
      if (!cancelled) {
        setFetched(results)
        setLoading(false)
      }
    })
    return () => {
      cancelled = true
    }
    // `branches` is refreshed by the parent's 5s poll; re-fetching this
    // panel's per-branch histories on every poll tick would defeat the
    // "only fetch when the map is actually open" point of lazy-loading
    // it in the first place. One-shot per mount, plus explicit retries
    // via `loadEpoch` (the failed-fetch state's retry button).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadEpoch])

  const toggleExpand = (id: string) => {
    setExpandedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const branchGroups = useMemo(
    () =>
      fetched.map(({ branch, events }) => {
        const groups = groupContiguous(events)
        // Attach declared intents — root branch only (checkpoint ranges
        // index main-timeline sequences; a child branch's post-fork
        // sequence numbers merely coincide with them). A run gets the
        // name only when EVERY event in it falls inside the covering
        // checkpoint's range: a straddling run stays unlabeled rather
        // than borrowing a name that only half-applies.
        if (branch.parent == null && checkpoints.length > 0) {
          for (const grp of groups) {
            const first = grp.events[0]
            if (!first) continue
            const cp = checkpointCovering(checkpoints, first.sequence_number)
            if (
              cp &&
              grp.events.every(
                (e) =>
                  e.sequence_number >= cp.event_range[0] &&
                  e.sequence_number <= cp.event_range[1],
              )
            ) {
              grp.intent = cp.name
            }
          }
        }
        return { branch, groups }
      }),
    [fetched, checkpoints],
  )

  const { nodes, edges } = useMemo(
    () => buildGraph(branchGroups, activeBranchId, expandedIds, liveNames, toggleExpand),
    [branchGroups, activeBranchId, expandedIds, liveNames],
  )

  useEffect(() => {
    if (rfInstance && !loading && nodes.length > 0) {
      // The wrapped-grid layout keeps the graph's aspect ratio close to
      // the dialog's, so React Flow's own fitView does the right thing.
      // (A hand-rolled height-fit used to live here to work around the
      // one-long-line layout rendering as a sliver — that layout is gone.)
      void rfInstance.fitView({ padding: 0.08, maxZoom: 1.25, duration: 200 })
      // Read back the zoom fitView landed on and use it as the zoom-out
      // floor — "zoom out" must never land on an empty field.
      const t = window.setTimeout(() => {
        setMinZoom(Math.min(0.4, rfInstance.getViewport().zoom * 0.75))
      }, 260)
      return () => window.clearTimeout(t)
    }
    // Fit once when data first lands; deliberately not re-fitting on
    // every expand/collapse (that would yank the view under the user's
    // cursor mid-interaction).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rfInstance, loading])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])

  const totalOps = fetched.reduce((sum, f) => sum + f.events.length, 0)
  const branchCount = branchGroups.length
  const bgColor = theme === 'dark' ? '#0b0d12' : '#eef1f6'
  const dotColor = theme === 'dark' ? '#2a2f3d' : '#c7cede'

  return (
    <>
      <div className="fixed inset-0 z-40 bg-black/25" onClick={onClose} aria-hidden />
      <div
        role="dialog"
        aria-label="Timeline map"
        className="fixed inset-6 z-50 flex flex-col rounded-lg border border-border bg-card shadow-2xl overflow-hidden font-mono"
      >
        <div className="flex items-start justify-between gap-3 px-4 py-2 border-b border-border shrink-0">
          <div className="min-w-0 flex items-center gap-2">
            <span className="text-[13px] font-medium text-foreground shrink-0">Timeline — map view</span>
            <span className="text-[11px] text-muted-foreground/70 truncate">
              {checkpoints.length > 0
                ? `Grouped by operation kind — ${checkpoints.length} declared intent${
                    checkpoints.length === 1 ? '' : 's'
                  } attached (◈) where a checkpoint covers a run.`
                : 'Grouped by operation kind — no declared intents on this document right now.'}
            </span>
            {(() => {
              const notice = durabilityNotice(durability)
              return notice ? <DurabilityChip notice={notice} /> : null
            })()}
            <button
              type="button"
              onClick={() => setDetailsOpen((v) => !v)}
              title="What this grouping is and isn't (click to expand)"
              aria-label="More about this grouping"
              aria-expanded={detailsOpen}
              className="shrink-0 w-4 h-4 rounded-full text-[10px] leading-none flex items-center justify-center border border-muted-foreground/40 text-muted-foreground/70 hover:text-foreground hover:border-foreground/60"
            >
              i
            </button>
          </div>
          <button
            type="button"
            onClick={onClose}
            title="Close map view"
            aria-label="Close map view"
            className="shrink-0 p-1 rounded text-muted-foreground hover:text-foreground hover:bg-accent/40"
          >
            <X size={16} />
          </button>
        </div>
        {detailsOpen && (
          <div className="px-4 py-2 border-b border-border shrink-0 bg-accent/10 text-[11px] text-muted-foreground/80 max-w-[85ch]">
            Grouped by contiguous runs of the same operation kind (e.g. 13 cylinder cuts in a
            row become one "Cyl ×13" card) — the real structure the event log supports today.
            Named intents (e.g. "bolt circle") will attach here once the agent declares them
            before executing, instead of being guessed after the fact.
            {' '}Certificates aren't attached to any operation yet (no producer wired), so
            nodes don't claim a proven/provisional/refused color — that would imply a
            distinction the data doesn't have.
            {' '}Each branch renders as its own lane — {branchCount === 0
              ? 'none exist on this document yet.'
              : `${branchCount} lane${branchCount === 1 ? '' : 's'} shown, real fork points only.`}
          </div>
        )}

        <div ref={canvasRef} className="flex-1 min-h-0 relative" style={{ background: bgColor }}>
          {loading ? (
            <div className="absolute inset-0 flex items-center justify-center text-[12px] text-muted-foreground/60">
              ⋯ loading branch histories
            </div>
          ) : totalOps === 0 && fetched.some((f) => f.failed) ? (
            // A refused fetch is NOT an empty document — say which it
            // was, and offer the retry in place instead of making the
            // user close and reopen the dialog.
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-[12px] text-amber-800 dark:text-amber-300">
              <span>history fetch refused (rate-limited or backend error) — the log was not read</span>
              <button
                type="button"
                onClick={() => setLoadEpoch((n) => n + 1)}
                className="px-2 py-1 rounded border border-amber-500/40 hover:bg-amber-500/10"
              >
                retry
              </button>
            </div>
          ) : totalOps === 0 ? (
            <div className="absolute inset-0 flex items-center justify-center text-[12px] text-muted-foreground/60">
              {durability?.state === 'failed'
                ? '∅ event log unreadable at boot — nothing to map'
                : '∅ no operations recorded yet'}
            </div>
          ) : (
            <ReactFlow
              nodes={nodes}
              edges={edges}
              nodeTypes={nodeTypes}
              onInit={setRfInstance}
              minZoom={minZoom}
              maxZoom={1.75}
              // Trackpad-first navigation (Varun, live: "unable to
              // traverse using the mousepad .. double finger swipes to
              // move left right up down"): React Flow's default treats
              // wheel/two-finger scroll as ZOOM, so swiping did nothing
              // useful. panOnScroll makes two-finger swipes pan in all
              // directions; pinch (ctrl+wheel) still zooms.
              panOnScroll
              zoomOnScroll={false}
              zoomOnPinch
              proOptions={{ hideAttribution: true }}
            >
              {/* xyflow's Controls/MiniMap chrome ships light-theme colors
                  hardcoded in its own stylesheet — it doesn't read this
                  app's theme. Without this override the zoom buttons sit
                  as a stark white box on the dark canvas (contrast bug
                  caught by viewing dark mode, not by typechecking). */}
              <style>{`
                .react-flow__controls-button {
                  background: ${theme === 'dark' ? '#1a1d26' : '#ffffff'} !important;
                  border-bottom-color: ${theme === 'dark' ? '#2a2f3d' : '#e5e7eb'} !important;
                  fill: ${theme === 'dark' ? '#c9cedb' : '#333333'} !important;
                }
                .react-flow__controls-button:hover {
                  background: ${theme === 'dark' ? '#262a36' : '#f4f4f5'} !important;
                }
                .react-flow__controls {
                  border: 1px solid ${theme === 'dark' ? '#2a2f3d' : '#e5e7eb'};
                  border-radius: 6px;
                  overflow: hidden;
                }
              `}</style>
              <Background gap={18} size={1.2} color={dotColor} />
              <Controls showInteractive={false} />
              <MiniMap
                pannable
                zoomable
                style={{ opacity: 0.9 }}
                maskColor={theme === 'dark' ? 'rgba(11,13,18,0.7)' : 'rgba(238,241,246,0.7)'}
                nodeColor={(n) => {
                  const d = n.data as Partial<GraphNodeData & LaneNodeData>
                  if (n.type === 'lane') return laneFillFor(!!d.isActive, String(d.state ?? ''))
                  if (d.branch) return strokeColorFor(!!d.isActiveBranch, d.branch.state)
                  return '#64748b'
                }}
              />
            </ReactFlow>
          )}
        </div>

        <div className="flex items-center gap-3 px-4 py-1.5 border-t border-border text-[10px] text-muted-foreground/70 shrink-0 flex-wrap">
          <span className="flex items-center gap-1">
            <span className="inline-block w-2 h-2 rounded-full" style={{ backgroundColor: '#2ecc71' }} /> active branch
          </span>
          <span className="flex items-center gap-1">
            <span className="inline-block w-2 h-2 rounded-full" style={{ backgroundColor: '#7c8aa5' }} /> other branch
          </span>
          <span className="flex items-center gap-1">
            <span className="inline-block w-2 h-2 rounded-full" style={{ backgroundColor: 'rgba(251,146,60,0.9)' }} /> abandoned
          </span>
          <span className="flex items-center gap-1 opacity-70">
            <span className="inline-block w-2 h-2 rounded-full" style={{ backgroundColor: '#94a3b8' }} /> merged
          </span>
          <span className="mx-1 text-muted-foreground/30">│</span>
          {(Object.keys(FAMILY_COLOR) as OpFamily[])
            .filter((f) => f !== 'other')
            .map((f) => (
              <span key={f} className="flex items-center gap-1">
                <span
                  className="inline-block w-2 h-2 rounded-[2px]"
                  style={{ backgroundColor: FAMILY_COLOR[f] }}
                />
                {f}
              </span>
            ))}
          <span className="ml-auto">click ▸ to unfold a group's raw operations</span>
        </div>
      </div>
    </>
  )
}
