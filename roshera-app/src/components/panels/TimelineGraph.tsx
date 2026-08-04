/**
 * Timeline map view — the timeline as a route, not an odometer (see vault
 * `Research/2026-07-31-ui-pass-spec.md` §3).
 *
 * A SECOND view alongside the existing `Timeline.tsx` strip (scrubbing
 * still lives there; this is for reading structure). Lazy-loaded from
 * `Timeline.tsx` — `@xyflow/react` never enters the initial bundle
 * unless the user actually opens this panel.
 *
 * ── What connects two cards ───────────────────────────────────────────
 * RECORDED LINEAGE, and nothing else. Every node is one operation; every
 * arrow means the entity named on it actually travelled from the
 * operation that produced it into the operation that consumed it. The
 * data comes from `GET /api/timeline/lineage/{branch}`, which projects
 * `timeline_engine::LineageGraph` — the kernel's own entity DAG — onto
 * events. A boolean therefore shows a JOIN (its two operands converging),
 * and `box → fillet → chamfer` on one solid shows as one CHAIN.
 *
 * An operation that recorded no entity refs at all arrives with
 * `linked: false` and is drawn dashed, in its own band, unattached. That
 * is the honest statement about it. It is NOT chained to whatever
 * happened to run next.
 *
 * ── Why grouping by operation kind is gone (read before re-adding it) ─
 * This panel used to group contiguous runs of the same `operation_type`
 * into one "Cyl ×8" card. That is adjacency, not lineage: eight unrelated
 * cylinder cuts became one tidy card while a genuine feature chain
 * scattered across cards. It was retired (Varun, 2026-08-03: one source of
 * truth for "what belongs together"). If eight unrelated cuts now render
 * as eight separate nodes, that is the correct picture — they were never
 * one feature.
 *
 * The older justification for it — "this kernel mints a FRESH solid id on
 * every mutating op, including a boolean's result, so 'same part' almost
 * never holds" — was FALSE and is corrected here for the record: only
 * `boolean` mints a fresh id (and retires both operands, `boolean.rs:689`);
 * `fillet`, `chamfer` and `transform` go through `solids.get_mut` and
 * PRESERVE the id. That preserved id is exactly what makes an
 * identity-preserving chain a chain, and the backend's continuation edge
 * is what surfaces it (the entity-level `x → x` self-edge is suppressed
 * upstream, where it would be a lie).
 *
 * ── Certificates ──────────────────────────────────────────────────────
 * Nodes are NOT coloured by certificate state. Kernel ops do now carry an
 * `EventCertificate` (recorder_bridge attaches `from_recorded_solid` at
 * record time), but the lineage endpoint does not serve it, so this view
 * has no certificate data to colour with and does not imply one. Colour
 * here means operation family and branch state, nothing more.
 *
 * ── Branches (Varun: "show me branched timeline too") ──────────────────
 * Each branch renders as its own LANE (a tinted band spanning the full
 * width, name + state pinned at the left), fork points drop as elbows from
 * the parent lane, and merged/abandoned lanes read distinctly (dashed,
 * dimmed). Only real `state`/`fork_point` data drives this — an empty
 * branch list renders an honest single `main` lane, never a fabricated
 * fork.
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  Handle,
  Position,
  MarkerType,
  type Node,
  type Edge,
  type NodeProps,
  type ReactFlowInstance,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { X } from 'lucide-react'
import { useThemeStore } from '@/stores/theme-store'
import {
  type CheckpointSummary,
  type DurabilityStatus,
  type LineageMap,
  type LineageMapNode,
  type LineageMapEdge,
  type LineageOutcome,
  type LineageRefusal,
  checkpointCovering,
  durabilityNotice,
  entityLabel,
  fetchLineageMap,
  resultRef,
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

/** A retire edge ends a lineage rather than continuing one — the one
 *  place a non-branch colour is allowed on an edge. */
const RETIRE_COLOR = '#dc2626'

// ─── Operation family — the "one look and it's clear" vocabulary ────────
// Seeded from the existing glyph vocabulary (`symbolForOperation` /
// `shortLabel` in `lib/timeline-events.ts`) rather than inventing new
// icons: a boolean is still `⊕`, a cylinder still `⊟`. What's added here
// is SHAPE + COLOUR per family, so the eye sorts nodes before it reads.

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

/** Boolean → hexagon (a union of shapes); transform → parallelogram (a
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

// ─── Node geometry ────────────────────────────────────────────────────

const NODE_WIDTH = 176
const NODE_HEIGHT = 48
/** Extra card height when a declared-intent overline is present, so the
 *  layout reserves real room for the third line instead of letting the
 *  card visually overflow its rank slot. */
const INTENT_LINE_HEIGHT = 15

// ─── Custom node: one operation, one card ────────────────────────────

interface OpNodeData extends Record<string, unknown> {
  branch: GraphBranch
  node: LineageMapNode
  isActiveBranch: boolean
  liveNames: Map<string, string>
  /** The DECLARED intent covering this operation — a checkpoint name read
   *  off the real checkpoint list, never inferred. `undefined` when no
   *  checkpoint covers it. Root-branch only: checkpoint `event_range`s
   *  index the main timeline's sequences, so applying them to a child
   *  branch's post-fork sequences would mislabel spans that merely share
   *  numbers. */
  intent?: string
  /** In-edges this lane could not draw because the producer sits before
   *  this branch's fork point (it lives on the parent's lane). Disclosed
   *  on the card so a node with an off-lane parent is never mistaken for
   *  a constructive root. */
  hiddenInputs: number
}

/** Card height. Every operation is one card, so the only variable is
 *  whether a declared intent adds an overline. */
function nodeHeight(intent?: string): number {
  return NODE_HEIGHT + (intent ? INTENT_LINE_HEIGHT : 0)
}

/** Everything that would clutter the card and now lives in the hover
 *  tooltip instead — branch, time, the full ref lists. Progressive
 *  disclosure: the card is for scanning (Varun: "maximum 2 lines"). */
function nodeTitle(data: OpNodeData): string {
  const { branch, node, intent } = data
  const lines = [
    intent ? `declared intent: ${intent}` : '',
    `${branch.name || 'main'} · #${node.sequence_number} · ${node.operation_type}`,
    `${formatTimestamp(node.timestamp)} · ${node.author}`,
    node.inputs.length ? `consumed: ${node.inputs.join(', ')}` : '',
    node.outputs.length ? `produced: ${node.outputs.join(', ')}` : '',
    node.deleted.length ? `deleted: ${node.deleted.join(', ')}` : '',
    node.linked ? '' : 'no entity refs recorded on this event — nothing derives from it and it derives from nothing',
    data.hiddenInputs > 0
      ? `${data.hiddenInputs} input${data.hiddenInputs === 1 ? '' : 's'} produced before this branch forked (shown on the parent lane)`
      : '',
  ]
  return lines.filter(Boolean).join('\n')
}

function OpNode({ data }: NodeProps<Node<OpNodeData>>) {
  const { branch, node, isActiveBranch, liveNames, intent, hiddenInputs } = data
  const branchStroke = strokeColorFor(isActiveBranch, branch.state)
  const key = shortLabel(node.operation_type)
  const family = familyOf(key)
  const familyColor = FAMILY_COLOR[family]
  const result = resultRef(node)
  const unlinked = !node.linked

  // Line 2 — the ONE detail that matters, in priority order: what this
  // operation left behind, what it retired, or (for an event with no
  // refs at all) the fact that nothing was recorded.
  const detail = unlinked
    ? 'no lineage recorded'
    : result
      ? `→ ${entityLabel(result, liveNames)}`
      : node.deleted.length > 0
        ? `✕ ${node.deleted.map((r) => entityLabel(r, liveNames)).join(', ')}`
        : `${node.inputs.length} in`

  return (
    <div
      title={nodeTitle(data)}
      className="shadow-sm text-foreground"
      style={{
        width: NODE_WIDTH,
        minHeight: NODE_HEIGHT,
        background: 'var(--card)',
        borderWidth: isActiveBranch ? 1.5 : 1,
        borderStyle: unlinked || branch.state === 'merged' ? 'dashed' : 'solid',
        borderColor: unlinked ? 'var(--muted-foreground)' : branchStroke,
        borderRadius: 8,
        opacity: branch.state === 'merged' ? 0.6 : unlinked ? 0.72 : 1,
        borderLeftWidth: unlinked ? 1 : 5,
        borderLeftColor: unlinked ? 'var(--muted-foreground)' : familyColor,
        ...(unlinked ? {} : familyShapeStyle(family)),
      }}
    >
      <Handle type="target" position={Position.Left} style={{ background: branchStroke, opacity: unlinked ? 0 : 0.6 }} />
      <div className={`${unlinked ? 'px-2.5' : familyExtraPaddingX(family)} py-1.5`}>
        {/* Line 0 — the DECLARED intent, when a real checkpoint covers this
            operation. Read off the checkpoint list, never inferred.
            Neutral text — colour stays reserved for state. */}
        {intent && (
          <div className="flex items-center gap-1 text-[10px] leading-tight text-foreground/80 mb-0.5 min-w-0">
            <span aria-hidden className="shrink-0 text-foreground/60">◈</span>
            <span className="truncate font-medium">{intent}</span>
          </div>
        )}
        {/* Line 1 — what it is: glyph + kernel kind. */}
        <div className="flex items-center justify-between gap-1 min-w-0">
          <span className="flex items-center gap-1.5 min-w-0 text-[12.5px] font-medium truncate">
            <span
              aria-hidden
              style={{ color: unlinked ? 'var(--muted-foreground)' : familyColor }}
              className="text-[14px] leading-none shrink-0"
            >
              {symbolForOperation(node.operation_type)}
            </span>
            <span className="truncate">{key}</span>
          </span>
          <span className="shrink-0 text-[9.5px] text-muted-foreground/60">#{node.sequence_number}</span>
        </div>
        {/* Line 2 — result, retirement, or the honest absence. */}
        <div
          className={`text-[10.5px] truncate mt-0.5 ${
            unlinked ? 'italic text-muted-foreground/70' : 'text-muted-foreground/80'
          }`}
        >
          {detail}
        </div>
        {hiddenInputs > 0 && (
          <div className="text-[9.5px] text-muted-foreground/60 truncate">
            ⇠ {hiddenInputs} from before the fork
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Right} style={{ background: branchStroke, opacity: unlinked ? 0 : 0.6 }} />
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
  /** Operations on this lane that recorded no lineage at all. Shown on
   *  the lane tag so the count is readable without a mouse. */
  unlinked: number
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
        {data.unlinked > 0 && (
          <span className="text-[9px] uppercase tracking-wide text-muted-foreground/80">
            {data.unlinked} unlinked
          </span>
        )}
      </div>
    </div>
  )
}

// ─── Stub node: an honestly-empty branch — a fork with nothing recorded
// on it yet. A real fork with 0 events is a meaningful state ("a road not
// yet travelled"), not a placeholder to hide: the honesty constraint cuts
// both ways — no inventing an op that didn't happen, but also no hiding a
// branch that genuinely exists just because it's quiet so far.
const STUB_WIDTH = 168
const STUB_HEIGHT = 40

interface StubNodeData extends Record<string, unknown> {
  branch: GraphBranch
  isActive: boolean
  /** Set when the branch's lineage could not be read at all, so an empty
   *  lane is never passed off as an empty branch. */
  reason: string | null
}

function StubNode({ data }: NodeProps<Node<StubNodeData>>) {
  const stroke = data.reason ? 'rgba(251,146,60,0.9)' : strokeColorFor(data.isActive, data.branch.state)
  return (
    <div
      title={
        data.reason
          ? `${data.branch.name} — lineage not read: ${data.reason}`
          : `${data.branch.name} — forked, no operations recorded on it yet`
      }
      className="flex items-center justify-center px-2 text-[10.5px] italic text-muted-foreground/70"
      style={{
        width: STUB_WIDTH,
        height: STUB_HEIGHT,
        borderRadius: 8,
        border: `1px dashed ${stroke}`,
        background: 'var(--card)',
      }}
    >
      <Handle type="target" position={Position.Left} style={{ background: stroke, opacity: 0.6 }} />
      <span className="truncate">{data.reason ? 'lineage not read' : 'no ops yet'}</span>
    </div>
  )
}

const nodeTypes = { op: OpNode, lane: LaneNode, stub: StubNode }

// ─── Layout: rank = longest path through the lineage DAG ───────────────
//
// X is the node's RANK (its longest path from a root), Y its position
// within that rank, so every lineage arrow runs left→right and same-rank
// siblings stack vertically. That is what makes a join legible: a
// boolean's two operands sit one above the other and converge on it.
//
// This is NOT the reverted dagre chain. That failed because one rank per
// GROUP put a 99-op history on one infinite line (Varun, live 2026-08-01:
// "i only see one straight line.. unable to make out what is happening").
// Here a rank holds every node at the same depth — 13 cylinder creates are
// 13 nodes in ONE column, not 13 columns — so a real CSG build renders a
// few columns wide. Deep chains additionally WRAP every `RANK_WRAP`
// columns, like text, so nothing runs off-screen.

const LANE_PAD_Y = 40
const LANE_PAD_X = 48
const RANK_WRAP = 7
const GAP_X = 56
const GAP_Y = 14
/** Vertical clearance between wrapped rank blocks of the same branch. */
const BLOCK_GAP_Y = 40
/** Clearance before the unlinked band at the foot of a lane. */
const UNLINKED_GAP = 34
/** Vertical clearance between one branch's band and the next. */
const BAND_GAP = LANE_PAD_Y * 2 + 36

interface Placement {
  x: number
  y: number
  h: number
}

/**
 * Place one branch's nodes. Linked nodes are ranked by longest path;
 * nodes with no recorded lineage go in their own band at the foot of the
 * lane, so "unlinked" stays readable instead of piling in with the real
 * constructive roots at rank 0.
 */
function layoutBranch(
  nodes: LineageMapNode[],
  edges: LineageMapEdge[],
  heightOf: (n: LineageMapNode) => number,
): { pos: Map<string, Placement>; width: number; height: number } {
  const pos = new Map<string, Placement>()
  const present = new Set(nodes.map((n) => n.id))
  const linked = nodes.filter((n) => n.linked).sort((a, b) => a.sequence_number - b.sequence_number)
  const unlinked = nodes.filter((n) => !n.linked).sort((a, b) => a.sequence_number - b.sequence_number)

  const incoming = new Map<string, string[]>()
  for (const e of edges) {
    if (!present.has(e.from) || !present.has(e.to)) continue
    const list = incoming.get(e.to)
    if (list) list.push(e.from)
    else incoming.set(e.to, [e.from])
  }

  // Longest-path rank. Every lineage edge runs from a lower sequence to a
  // higher one (a producer always precedes its consumer in the log), so
  // ascending sequence order is a valid topological order and one pass
  // suffices.
  const rank = new Map<string, number>()
  const columns: LineageMapNode[][] = []
  for (const n of linked) {
    let r = 0
    for (const src of incoming.get(n.id) ?? []) r = Math.max(r, (rank.get(src) ?? 0) + 1)
    rank.set(n.id, r)
    if (!columns[r]) columns[r] = []
    columns[r].push(n)
  }

  const stackHeight = (col: LineageMapNode[]): number =>
    col.reduce((sum, n) => sum + heightOf(n), 0) + GAP_Y * Math.max(0, col.length - 1)

  let width = 0
  let y = 0
  const blocks = Math.ceil(columns.length / RANK_WRAP)
  for (let b = 0; b < blocks; b++) {
    const inBlock = columns.slice(b * RANK_WRAP, (b + 1) * RANK_WRAP)
    const blockH = inBlock.reduce((max, col) => Math.max(max, stackHeight(col ?? [])), 0)
    inBlock.forEach((col, i) => {
      if (!col || col.length === 0) return
      const x = i * (NODE_WIDTH + GAP_X)
      let cy = y + (blockH - stackHeight(col)) / 2
      for (const n of col) {
        const h = heightOf(n)
        pos.set(n.id, { x, y: cy, h })
        cy += h + GAP_Y
      }
      width = Math.max(width, x + NODE_WIDTH)
    })
    y += blockH + BLOCK_GAP_Y
  }
  if (blocks > 0) y -= BLOCK_GAP_Y

  if (unlinked.length > 0) {
    if (blocks > 0) y += UNLINKED_GAP
    let rowTop = y
    let rowH = 0
    unlinked.forEach((n, i) => {
      const col = i % RANK_WRAP
      if (col === 0 && i > 0) {
        rowTop += rowH + GAP_Y
        rowH = 0
      }
      const h = heightOf(n)
      rowH = Math.max(rowH, h)
      const x = col * (NODE_WIDTH + GAP_X)
      pos.set(n.id, { x, y: rowTop, h })
      width = Math.max(width, x + NODE_WIDTH)
    })
    y = rowTop + rowH
  }

  return { pos, width, height: y }
}

// ─── Per-branch prepared data ─────────────────────────────────────────

interface BranchLineage {
  branch: GraphBranch
  /** Nodes shown on this lane (post-fork only for a child branch). */
  nodes: LineageMapNode[]
  /** Edges with both endpoints on this lane. */
  edges: LineageMapEdge[]
  /** node id → in-edges dropped because the producer sits before the fork. */
  hiddenInputs: Map<string, number>
  /** Declared intent per node id (root branch only). */
  intents: Map<string, string>
  /** Non-null when the endpoint refused, or could not be read. */
  refusal: LineageRefusal | null
  unreachable: string | null
  window: LineageMap['window'] | null
  entityCount: number
}

function prepare(
  branch: GraphBranch,
  outcome: LineageOutcome,
  checkpoints: CheckpointSummary[],
): BranchLineage {
  const empty: BranchLineage = {
    branch,
    nodes: [],
    edges: [],
    hiddenInputs: new Map(),
    intents: new Map(),
    refusal: null,
    unreachable: null,
    window: null,
    entityCount: 0,
  }
  if (outcome.state === 'refused') return { ...empty, refusal: outcome.refusal }
  if (outcome.state === 'unreachable') return { ...empty, unreachable: outcome.reason }

  const { map } = outcome
  // A child branch's history includes its parent's events; those are
  // already drawn on the parent's lane, so this lane shows only what was
  // recorded AFTER the fork.
  const isRoot = branch.parent == null
  const forkIdx = branch.fork_point?.event_index ?? 0
  const nodes = isRoot ? map.nodes : map.nodes.filter((n) => n.sequence_number > forkIdx)
  const present = new Set(nodes.map((n) => n.id))

  // Edges leaving the lane are dropped EXPLICITLY and counted — React
  // Flow silently discards a dangling edge, which would make a chain
  // appear broken with no explanation.
  const edges: LineageMapEdge[] = []
  const hiddenInputs = new Map<string, number>()
  for (const e of map.edges) {
    const hasFrom = present.has(e.from)
    const hasTo = present.has(e.to)
    if (hasFrom && hasTo) edges.push(e)
    else if (hasTo) hiddenInputs.set(e.to, (hiddenInputs.get(e.to) ?? 0) + 1)
  }

  const intents = new Map<string, string>()
  if (isRoot && checkpoints.length > 0) {
    for (const n of nodes) {
      const cp = checkpointCovering(checkpoints, n.sequence_number)
      if (cp) intents.set(n.id, cp.name)
    }
  }

  return {
    branch,
    nodes,
    edges,
    hiddenInputs,
    intents,
    refusal: null,
    unreachable: null,
    window: map.window,
    entityCount: map.entity_count,
  }
}

// ─── Graph assembly ───────────────────────────────────────────────────

/** React Flow node id. Event UUIDs are unique per event, but a branch
 *  whose `fork_point` is absent falls back to showing its inherited
 *  history, which would put the SAME event on two lanes and collide.
 *  Scoping the id to the lane makes that impossible rather than unlikely. */
function nid(branchId: string, eventId: string): string {
  return `${branchId}:${eventId}`
}

function buildGraph(
  lanes: BranchLineage[],
  activeBranchId: string,
  liveNames: Map<string, string>,
): { nodes: Node<OpNodeData | LaneNodeData | StubNodeData>[]; edges: Edge[] } {
  const contentNodes: Node<OpNodeData | StubNodeData>[] = []
  const flowEdges: Edge[] = []
  const placed = new Set<string>()
  const laneBounds = new Map<string, { minY: number; maxY: number }>()
  /** First node id on each lane — the fork elbow's landing point. */
  const laneEntry = new Map<string, string>()
  let globalMaxX = 0
  let yCursor = 0

  for (const lane of lanes) {
    const { branch } = lane
    const isActive = branch.id === activeBranchId

    if (lane.nodes.length === 0) {
      // A root branch with nothing on it renders no lane at all (the
      // panel-level empty state speaks instead); a FORK with nothing on
      // it is real structure and gets a stub.
      if (!branch.parent && !lane.refusal && !lane.unreachable) continue
      const id = `stub:${branch.id}`
      contentNodes.push({
        id,
        type: 'stub',
        position: { x: 0, y: yCursor },
        draggable: false,
        data: {
          branch,
          isActive,
          reason: lane.refusal ? lane.refusal.reason : lane.unreachable,
        },
      })
      placed.add(id)
      laneEntry.set(branch.id, id)
      laneBounds.set(branch.id, { minY: yCursor, maxY: yCursor + STUB_HEIGHT })
      globalMaxX = Math.max(globalMaxX, STUB_WIDTH)
      yCursor += STUB_HEIGHT + BAND_GAP
      continue
    }

    const heightOf = (n: LineageMapNode) => nodeHeight(lane.intents.get(n.id))
    const { pos, width, height } = layoutBranch(lane.nodes, lane.edges, heightOf)

    for (const n of lane.nodes) {
      const p = pos.get(n.id)
      if (!p) continue
      contentNodes.push({
        id: nid(branch.id, n.id),
        type: 'op',
        position: { x: p.x, y: yCursor + p.y },
        data: {
          branch,
          node: n,
          isActiveBranch: isActive,
          liveNames,
          intent: lane.intents.get(n.id),
          hiddenInputs: lane.hiddenInputs.get(n.id) ?? 0,
        },
      })
      placed.add(nid(branch.id, n.id))
    }
    // Lane entry = the earliest node, for the fork elbow.
    const first = [...lane.nodes].sort((a, b) => a.sequence_number - b.sequence_number)[0]
    if (first) laneEntry.set(branch.id, nid(branch.id, first.id))

    const color = strokeColorFor(isActive, branch.state)
    for (const e of lane.edges) {
      const isRetire = e.kind === 'retire'
      const stroke = isRetire ? RETIRE_COLOR : color
      flowEdges.push({
        id: `${e.kind}:${branch.id}:${e.from}->${e.to}:${e.via}`,
        source: nid(branch.id, e.from),
        target: nid(branch.id, e.to),
        type: 'smoothstep',
        // `via` is the operand's own name — shown where it disambiguates
        // (retirements always; joins are labelled below once in-degrees
        // are known) and suppressed on plain chains, where an edge label
        // per node would be a hairball rather than information.
        label: isRetire ? `✕ ${e.via}` : undefined,
        labelStyle: { fontSize: 9, fill: 'var(--foreground)' },
        labelBgStyle: { fill: 'var(--card)', fillOpacity: 0.85 },
        data: { via: e.via, kind: e.kind },
        markerEnd: { type: MarkerType.ArrowClosed, width: 12, height: 12, color: stroke },
        style: {
          stroke,
          strokeWidth: isActive ? 1.6 : 1.1,
          strokeDasharray: isRetire ? '4 3' : branch.state === 'merged' ? '4 3' : undefined,
        },
      })
    }

    laneBounds.set(branch.id, { minY: yCursor, maxY: yCursor + height })
    globalMaxX = Math.max(globalMaxX, width)
    yCursor += height + BAND_GAP
  }

  // A JOIN is the structure worth naming: when two or more lineages
  // converge on one operation, each arrow is labelled with the entity it
  // carried, so "which operand is which" is readable without a click.
  const inDegree = new Map<string, number>()
  for (const e of flowEdges) inDegree.set(e.target, (inDegree.get(e.target) ?? 0) + 1)
  for (const e of flowEdges) {
    if (e.label === undefined && (inDegree.get(e.target) ?? 0) > 1) {
      const via = (e.data as { via?: string } | undefined)?.via
      if (via) e.label = via
    }
  }

  // Fork elbows: the parent lane's node covering the fork point → the
  // child lane's first node. Real `fork_point` data only.
  for (const lane of lanes) {
    const { branch } = lane
    if (!branch.parent) continue
    const parent = lanes.find((l) => l.branch.id === branch.parent)
    const targetId = laneEntry.get(branch.id)
    if (!parent || !targetId) continue
    const forkIdx = branch.fork_point?.event_index ?? 0
    const candidates = parent.nodes.filter((n) => n.sequence_number <= forkIdx)
    const source = candidates.length > 0 ? candidates[candidates.length - 1] : parent.nodes[0]
    const sourceId = source ? nid(parent.branch.id, source.id) : null
    if (!sourceId || !placed.has(sourceId) || !placed.has(targetId)) continue
    flowEdges.push({
      id: `fork:${sourceId}->${targetId}`,
      source: sourceId,
      target: targetId,
      type: 'smoothstep',
      label: branch.name || undefined,
      labelStyle: { fontSize: 10, fill: 'var(--foreground)' },
      style: {
        stroke: strokeColorFor(branch.id === activeBranchId, branch.state),
        strokeDasharray: '3 3',
        strokeWidth: 1,
      },
    })
  }

  const laneNodes: Node<LaneNodeData>[] = []
  for (const lane of lanes) {
    const bounds = laneBounds.get(lane.branch.id)
    if (!bounds) continue
    laneNodes.push({
      id: `lane:${lane.branch.id}`,
      type: 'lane',
      position: { x: -LANE_PAD_X, y: bounds.minY - LANE_PAD_Y },
      draggable: false,
      selectable: false,
      focusable: false,
      zIndex: -1,
      data: {
        label:
          lane.branch.name || (lane.branch.id === MAIN_BRANCH_ID ? 'main' : lane.branch.id.slice(0, 8)),
        state: lane.branch.state,
        isActive: lane.branch.id === activeBranchId,
        width: globalMaxX + LANE_PAD_X * 2,
        height: bounds.maxY - bounds.minY + LANE_PAD_Y * 2,
        unlinked: lane.nodes.filter((n) => !n.linked).length,
      },
    })
  }

  // Lanes FIRST so they paint behind the operation cards (React Flow
  // stacks by array order; `zIndex: -1` above reinforces it).
  return { nodes: [...laneNodes, ...contentNodes], edges: flowEdges }
}

// ─── Panel ────────────────────────────────────────────────────────────

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
  /** Named design states (real, durable checkpoints; empty until an
   *  intent is declared). */
  checkpoints: CheckpointSummary[]
  /** Durability boot outcome — quarantine is disclosed in the header. */
  durability: DurabilityStatus | null
  onClose: () => void
}) {
  const [fetched, setFetched] = useState<{ branch: GraphBranch; outcome: LineageOutcome }[]>([])
  const [loading, setLoading] = useState(true)
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
  // what it does"). Collapsed by default; the ⓘ expands it in place.
  const [detailsOpen, setDetailsOpen] = useState(false)

  // The parent's `branches` state can legitimately be EMPTY when the map
  // opens (the /api/branches poll may be rate-limited or still in flight
  // after a remount) — but `main` always exists, so fall back to it
  // rather than mapping nothing over a document with real history.
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
    Promise.all(
      effectiveBranches.map(async (branch) => ({
        branch,
        outcome: await fetchLineageMap(branch.id),
      })),
    ).then((results) => {
      if (!cancelled) {
        setFetched(results)
        setLoading(false)
      }
    })
    return () => {
      cancelled = true
    }
    // `branches` is refreshed by the parent's 5s poll; re-fetching this
    // panel's per-branch lineage on every poll tick would defeat the
    // "only fetch when the map is actually open" point of lazy-loading it
    // in the first place. One-shot per mount, plus explicit retries via
    // `loadEpoch` (the failed-fetch state's retry button).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadEpoch])

  const lanes = useMemo(
    () => fetched.map(({ branch, outcome }) => prepare(branch, outcome, checkpoints)),
    [fetched, checkpoints],
  )

  const { nodes, edges } = useMemo(
    () => buildGraph(lanes, activeBranchId, liveNames),
    [lanes, activeBranchId, liveNames],
  )

  useEffect(() => {
    if (rfInstance && !loading && nodes.length > 0) {
      void rfInstance.fitView({ padding: 0.08, maxZoom: 1.25, duration: 200 })
      // Read back the zoom fitView landed on and use it as the zoom-out
      // floor — "zoom out" must never land on an empty field.
      const t = window.setTimeout(() => {
        setMinZoom(Math.min(0.4, rfInstance.getViewport().zoom * 0.75))
      }, 260)
      return () => window.clearTimeout(t)
    }
    // Fit once when data first lands; deliberately not re-fitting on every
    // interaction (that would yank the view under the user's cursor).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rfInstance, loading])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [onClose])

  const totalOps = lanes.reduce((sum, l) => sum + l.nodes.length, 0)
  const totalLinks = lanes.reduce((sum, l) => sum + l.edges.length, 0)
  const totalUnlinked = lanes.reduce((sum, l) => sum + l.nodes.filter((n) => !n.linked).length, 0)
  // Distinct entities the kernel's own DAG saw — the size of the graph
  // BEHIND this event view (a solid, its faces, its edges).
  const totalEntities = lanes.reduce((sum, l) => sum + l.entityCount, 0)
  const refusals = lanes.filter((l) => l.refusal !== null)
  const unreachable = lanes.filter((l) => l.unreachable !== null)
  const truncated = lanes.some((l) => l.window?.truncated)
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
              {totalOps === 0
                ? 'Linked by recorded lineage.'
                : `Linked by recorded lineage — ${totalOps} op${totalOps === 1 ? '' : 's'}, ` +
                  `${totalLinks} link${totalLinks === 1 ? '' : 's'}, ` +
                  `${totalEntities} entit${totalEntities === 1 ? 'y' : 'ies'}` +
                  (totalUnlinked > 0 ? `, ${totalUnlinked} unlinked` : '')}
            </span>
            {truncated && (
              <span
                title="The lineage window filled up — producers outside it are not represented, so some nodes may look like roots that are not."
                className="shrink-0 px-1.5 py-0.5 rounded text-[9.5px] uppercase tracking-wide border border-amber-500/40 text-amber-700 dark:text-amber-300"
              >
                partial window
              </span>
            )}
            {(() => {
              const notice = durabilityNotice(durability)
              return notice ? <DurabilityChip notice={notice} /> : null
            })()}
            <button
              type="button"
              onClick={() => setDetailsOpen((v) => !v)}
              title="What connects two cards here (click to expand)"
              aria-label="More about this map"
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
            Every node is one operation. An arrow means the entity named on it actually flowed
            from one operation into the next — the input→output lineage the kernel recorded,
            projected by <code>timeline_engine::LineageGraph</code>. A boolean shows its two
            operands converging; a fillet and chamfer on the same solid stay one chain, because
            the kernel preserves that solid's id.
            {' '}An operation that recorded no entity refs at all is drawn dashed in the band at
            the foot of its lane and says “no lineage recorded”. It is never chained to its
            neighbour: sharing an operation kind, or a moment in time, is adjacency, not lineage.
            {' '}Nodes carry no certificate colour — kernel ops do record certificates now, but
            this endpoint does not serve them, and colouring by data this view hasn't read would
            imply a distinction it cannot back.
            {' '}Each branch renders as its own lane — {lanes.length === 0
              ? 'none exist on this document yet.'
              : `${lanes.length} lane${lanes.length === 1 ? '' : 's'} shown, real fork points only.`}
          </div>
        )}
        {refusals.length > 0 && (
          <div className="px-4 py-2 border-b border-border shrink-0 bg-amber-500/10 text-[11px] text-amber-800 dark:text-amber-300">
            {refusals.map((l) => (
              <div key={l.branch.id} className="truncate">
                <span className="font-semibold">{l.branch.name || 'main'}: lineage refused</span>
                {' — '}
                {l.refusal?.reason}
                {l.refusal && l.refusal.entities.length > 0 && (
                  <span> ({l.refusal.entities.join(', ')})</span>
                )}
              </div>
            ))}
          </div>
        )}

        <div ref={canvasRef} className="flex-1 min-h-0 relative" style={{ background: bgColor }}>
          {loading ? (
            <div className="absolute inset-0 flex items-center justify-center text-[12px] text-muted-foreground/60">
              ⋯ reading recorded lineage
            </div>
          ) : totalOps === 0 && unreachable.length > 0 ? (
            // A failed read is NOT an empty document — say which it was,
            // and offer the retry in place.
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 text-[12px] text-amber-800 dark:text-amber-300">
              <span>lineage not read ({unreachable[0].unreachable}) — the log was not consulted</span>
              <button
                type="button"
                onClick={() => setLoadEpoch((n) => n + 1)}
                className="px-2 py-1 rounded border border-amber-500/40 hover:bg-amber-500/10"
              >
                retry
              </button>
            </div>
          ) : totalOps === 0 && refusals.length > 0 ? (
            <div className="absolute inset-0 flex items-center justify-center px-8 text-center text-[12px] text-amber-800 dark:text-amber-300">
              no graph is drawn: the recorded lineage was refused (see above) — an empty map would
              be a false answer
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
                  const d = n.data as Partial<OpNodeData & LaneNodeData>
                  if (n.type === 'lane') return laneFillFor(!!d.isActive, String(d.state ?? ''))
                  if (d.node && !d.node.linked) return '#94a3b8'
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
          <span className="flex items-center gap-1 opacity-70">
            <span className="inline-block w-2 h-2 rounded-full" style={{ backgroundColor: '#94a3b8' }} /> merged
          </span>
          <span className="mx-1 text-muted-foreground/30">│</span>
          <span className="flex items-center gap-1">
            <span
              className="inline-block w-4 h-0"
              style={{ borderTop: `1.5px solid ${RETIRE_COLOR}` }}
            />
            retires (✕)
          </span>
          <span className="flex items-center gap-1">
            <span
              className="inline-block w-3 h-2 rounded-[2px]"
              style={{ border: '1px dashed var(--muted-foreground)' }}
            />
            no lineage recorded
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
          <span className="ml-auto">left → right is derivation; an arrow's label is the entity that flowed</span>
        </div>
      </div>
    </>
  )
}
