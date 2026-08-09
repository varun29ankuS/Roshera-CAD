/**
 * Shared event-rendering helpers consumed by both the bottom Timeline
 * strip (`components/panels/Timeline.tsx`) and the top-left Feature
 * Tree (`components/panels/FeatureTree.tsx`).
 *
 * Pure module — no React imports. Mirrors the wire shape of
 * `GET /api/timeline/history/{branch_id}` (`EventSummary` in
 * `roshera-backend/api-server/src/handlers/timeline.rs`).
 */

// ─── Types matching backend GET /api/timeline/history/{branch_id} ──

export interface EventSummary {
  id: string
  sequence_number: number
  timestamp: string // ISO 8601
  operation_type: string // clean kernel kind, e.g. "create_box_3d"
  /** Full structured operation as tagged JSON (backend-emitted). */
  operation?: unknown
  author: string // clean display name
  /** Backend-emitted classification: "user" | "ai" | "system". */
  author_kind?: AuthorKind
  /** Top-level solid parts this event produced or modified, as namespaced
   *  ids ("solid:2", …). Excludes consumed operands (they're inputs) and
   *  sub-entities (face/edge). Empty for non-geometry events (drawing,
   *  mould, checkpoint). Drives the per-part swimlane grouping; absent on
   *  responses from a backend that predates the field. */
  affected_parts?: string[]
}

export type AuthorKind = 'user' | 'ai' | 'system'

// ─── Named design states (GET /api/timeline/checkpoints) ───────────
//
// A checkpoint is a DECLARED intent — the agent (or user) naming what a
// span of raw operations was FOR ("bolt circle, 8×⌀18"). This is the
// layer that turns the event log into a map of decisions. Checkpoints
// are durable: the backend persists each one on creation and restores
// them at boot (api-server/src/durability.rs, persist_checkpoint /
// restore_checkpoints), so an [] from the endpoint means nobody has
// declared an intent on this document — a first-class, true state.
// Renderers must show that emptiness as such ("no declared intents"),
// never hide it and never invent names to fill the gap.

export interface CheckpointSummary {
  id: string
  name: string
  description: string
  /** `[first, last]` event sequence numbers this decision covers. */
  event_range: [number, number]
  /**
   * The branch this checkpoint was created against — `"main"` for the
   * trunk, otherwise the branch UUID (`CheckpointSummary::branch_id`,
   * populated via `branch_ref_string` in `handlers/timeline.rs`).
   * `event_range` indexes into THIS branch's sequence numbers:
   * sequence numbers are per DOCUMENT, not per branch, so a branch
   * that forked from a shared prefix genuinely carries the same
   * numbers a sibling branch (or the trunk) does. `checkpointCovering`
   * below is the only reader and requires this field to avoid
   * attributing one branch's declared decision to another branch's
   * identically-numbered event.
   *
   * Optional here only because the wire type can't force it: the
   * current backend always sends it (pre-field persisted checkpoints
   * default to `"main"` at deserialize time —
   * `timeline-engine/src/types.rs:174-179` — so even old rows arrive
   * populated). An absent value here means this response came from a
   * backend build that predates the field entirely (frontend/backend
   * version skew), not stale data. See `checkpointCovering` for how
   * that case is handled — it is NOT treated as "matches every
   * branch".
   */
  branch_id?: string
  /**
   * `[first, last]` events this decision actually AUTHORED, as opposed to
   * `event_range`, which is a RESTORE MARKER covering everything on the
   * branch up to this point (`Timeline::create_checkpoint`'s documented
   * contract: replaying `[0, last]` reproduces the state).
   *
   * Because the restore marker always starts at 0 on a branch that starts
   * at 0, successive checkpoints NEST rather than partition — the real
   * flange document reports `[0,8] [0,17] [0,36] [0,45] [0,47] [0,72]`.
   * Matching on that, last-covering-wins, credits every early operation to
   * the most RECENT decision: the bolt-circle work gets labelled with the
   * raised-face declaration. A confidently wrong intent label is worse
   * than none, so `covers` is preferred wherever the backend sends it.
   *
   * Optional only because the wire type cannot force it — a response
   * without it came from a backend build predating the field, not from
   * stale data. `checkpointCovering` falls back to `event_range` in that
   * case, which is the pre-existing (wrong-but-familiar) behaviour rather
   * than a silent blank.
   */
  covers?: [number, number]
  author: string
  timestamp: string // ISO 8601 (RFC 3339) — NOT epoch ms
  tags: string[]
}

/** `#4–#17`, collapsing a single-event range to `#4`. */
export function formatEventRange(range: [number, number]): string {
  const [a, b] = range
  return a === b ? `#${a}` : `#${a}–#${b}`
}

// ─── Checkpoint-name quality floor ──────────────────────────────────
//
// The ◈ button posts straight to `POST /api/timeline/checkpoint`, which
// bypasses the MCP intent gate — but the route itself now holds the
// floor too (`checkpoint_name_refusal` in api-server/src/handlers/
// timeline.rs returns a typed 422), so this client copy exists to
// refuse locally, before a round trip. It mirrors
// `GENERIC_CHECKPOINT_NAME` in `roshera-mcp/src/gates.ts` and the Rust
// floor. The three copies cannot share a constant across packages, but
// they are NOT hand-synced on trust:
// `regex_copies_agree_across_the_three_packages` (timeline.rs,
// `checkpoint_name_gate_tests`) embeds this file's source and fails
// `cargo test -p api-server` if this pattern's text drifts from the
// other two. The rule: a generic word, an ordinal, or both — "step 3",
// "cp 2", "checkpoint", "7" — is a sequence position, not an intent.
// Any real intent phrase ("bolt circle 8 x D18 on D160 B.C.", even the
// terse "cut cylinders") passes; quality beyond this floor stays
// judgment, not schema.

const GENERIC_CHECKPOINT_NAME =
  /^(?:(?:step|op|operation|cut|feature|part|checkpoint|chkpt|cp|test|wip|tmp|temp|misc)[\s\-_#:.]*)?\d*$/i

// Refuse a bare clock/date reading (with or without a generic-word
// prefix) — "Checkpoint 9:59:36 PM", "10:05", "2026-08-01". That is
// exactly the named-nothing string this UI's own button used to mint
// from the system clock; it slips the generic regex above (whose tail
// accepts only a plain ordinal) while carrying less than "step 3"
// (every row already shows its time). This rule was found here first,
// then adopted by the MCP gate and the REST floor — all three now
// carry it, kept identical by the parity test named above.
const CLOCK_CHECKPOINT_NAME =
  /^(?:(?:step|op|operation|checkpoint|chkpt|cp)[\s\-_#:.]*)?\d{1,4}([:\-/.]\d{1,2}){1,2}(\s*(am|pm))?$/i

/**
 * Why `name` is not an acceptable checkpoint name, or `null` when it
 * is. The message says what a good name looks like, with a concrete
 * example — same shape as the MCP gate's refusal.
 */
export function checkpointNameRefusal(name: string): string | null {
  const trimmed = name.trim()
  if (trimmed.length === 0) {
    return 'A checkpoint is a declared intent — name the feature you are about to build.'
  }
  if (GENERIC_CHECKPOINT_NAME.test(trimmed) || CLOCK_CHECKPOINT_NAME.test(trimmed)) {
    return (
      `'${trimmed}' names a sequence position, not a design intent. ` +
      "Name what a drawing would name — the feature, its governing dimensions, " +
      "and where it sits: e.g. 'bolt circle 8 x D18 on D160 B.C.' or " +
      "'M8 clearance holes, close fit, 4x base corners'."
    )
  }
  return null
}

// The all-zero UUID (`Uuid::nil()`) every branch-listing endpoint
// (`GET /api/branches`, `BranchView::id`) spells the trunk as. A
// checkpoint spells the SAME branch differently — the literal string
// `"main"` — because `branch_id` round-trips through endpoints
// (`POST /api/branches/select`, …) that accept `"main"` as the
// well-known label. The two spellings must be reconciled before any
// equality check; `canonicalBranchId` is that reconciliation, kept
// private because `checkpointCovering` is the only caller that needs
// it. (`Timeline.tsx` and `TimelineGraph.tsx` each independently
// define this same literal for their own branch bookkeeping — see
// their `MAIN_BRANCH_ID` — this module stays consistent with both by
// construction, not by importing across them.)
const MAIN_BRANCH_ID = '00000000-0000-0000-0000-000000000000'

function canonicalBranchId(id: string): string {
  return id === 'main' ? MAIN_BRANCH_ID : id
}

/**
 * The declared intent covering event `sequence` **on `branchId`**, or
 * `null` when nobody named this span of history on that branch.
 *
 * `branchId` is required, not optional. Sequence numbers are per
 * DOCUMENT, not per branch — a branch that forked from a shared prefix
 * genuinely carries the same numbers a sibling branch (or the trunk)
 * does. Matching by `event_range` alone — this function's original
 * behaviour — would therefore attribute one branch's declared decision
 * to another branch's identically-numbered event: exactly the failure
 * `timeline_engine::Checkpoint`'s own doc comment
 * (`timeline-engine/src/types.rs:169-179`) names. Every checkpoint is
 * filtered to `branchId` FIRST; only among the survivors does the
 * range comparison run.
 *
 * The one assumption this function still makes: a checkpoint whose
 * `branch_id` the payload omitted is treated as belonging to `"main"`,
 * mirroring the backend's own default for pre-field persisted rows
 * (`BranchId::main`) — chosen deliberately over "matches every branch",
 * which would silently resurrect range-only matching for exactly the
 * payloads most likely to be old and cross-branch. Every OTHER match is
 * a real checkpoint, on its recorded branch, covering the recorded
 * sequence — nothing else here is guessed.
 *
 * Ranges may overlap within a branch (a later, more specific
 * declaration over an earlier broad one) — the LAST covering
 * checkpoint in list order wins, matching "most recently declared
 * intent".
 */
export function checkpointCovering(
  checkpoints: CheckpointSummary[],
  sequence: number,
  branchId: string,
): CheckpointSummary | null {
  const target = canonicalBranchId(branchId)
  let found: CheckpointSummary | null = null
  for (const cp of checkpoints) {
    if (canonicalBranchId(cp.branch_id ?? 'main') !== target) continue
    // `covers` is the authored span and PARTITIONS the branch, so at most
    // one checkpoint matches and "last wins" never has to arbitrate. The
    // `event_range` fallback is the restore marker, which nests — there
    // last-wins picks the most RECENT decision, which is the miscrediting
    // this field exists to end. See the doc on `covers`.
    const [a, b] = cp.covers ?? cp.event_range
    // An empty span (a > b, a decision that authored nothing) matches
    // nothing, which is the intended reading.
    if (sequence >= a && sequence <= b) found = cp
  }
  return found
}

// ─── Lineage map (GET /api/timeline/lineage/{branch_id}) ───────────
//
// The recorded answer to "what derives from what". Mirrors
// `LineageMapResponse` in `api-server/src/handlers/timeline.rs`, which
// projects `timeline_engine::LineageGraph` — the kernel's own entity DAG
// — onto events: one node per operation, one edge per entity that
// actually travelled between two operations.
//
// This is the ONLY grouping source the map view uses. It deliberately
// replaced client-side grouping by operation kind: contiguous runs of the
// same kind are adjacency, not lineage, and eight unrelated cylinder cuts
// collapsing into one tidy card was a pleasant lie. An event that recorded
// no refs arrives with `linked: false` and must render unattached — the
// honest statement — rather than being chained to whatever ran next.

export type LineageEdgeKind = 'flow' | 'retire'

export interface LineageMapNode {
  /** Event UUID — the id every edge endpoint refers to. */
  id: string
  sequence_number: number
  timestamp: string // ISO 8601 (RFC 3339)
  operation_type: string
  author: string
  author_kind?: AuthorKind
  /** Canonical `"kind:id"` refs consumed / produced / deleted. */
  inputs: string[]
  outputs: string[]
  deleted: string[]
  /** `false` exactly when the event recorded NO refs at all. Distinct
   *  from "no inputs": a `create_box_3d` is a constructive root —
   *  `linked: true` with an empty `inputs` — and renders differently. */
  linked: boolean
}

export interface LineageMapEdge {
  /** Producer event id. */
  from: string
  /** Consumer (or deleting) event id. */
  to: string
  /** The entity that travelled — what makes a join readable. */
  via: string
  kind: LineageEdgeKind
}

export interface LineageWindow {
  start: number
  limit: number
  returned: number
  /** The page filled up: producers outside the window are absent, so
   *  some nodes may look like roots that aren't. Always disclosed. */
  truncated: boolean
}

export interface LineageMap {
  branch: string
  nodes: LineageMapNode[]
  edges: LineageMapEdge[]
  entity_count: number
  window: LineageWindow
}

/** A typed refusal from the endpoint — `CycleDetected` (the recorded log
 *  contains an entity that is its own ancestor, so no honest graph
 *  exists) or `BranchNotFound`. Never rendered as an empty graph. */
export interface LineageRefusal {
  kind: string
  reason: string
  entities: string[]
}

export type LineageOutcome =
  | { state: 'graph'; map: LineageMap }
  | { state: 'refused'; refusal: LineageRefusal }
  /** The fetch itself never produced an answer (network / non-typed
   *  error). Zero nodes from a failed request is NOT the same fact as an
   *  empty branch, and the two must not share an empty state. */
  | { state: 'unreachable'; reason: string }

function asRefusal(payload: unknown): LineageRefusal | null {
  if (!payload || typeof payload !== 'object') return null
  const p = payload as Record<string, unknown>
  if (p.status !== 'LineageRefused') return null
  return {
    kind: typeof p.kind === 'string' ? p.kind : 'Unknown',
    reason: typeof p.reason === 'string' ? p.reason : 'no reason given',
    entities: Array.isArray(p.entities) ? p.entities.filter((e): e is string => typeof e === 'string') : [],
  }
}

function asMap(payload: unknown): LineageMap | null {
  if (!payload || typeof payload !== 'object') return null
  const p = payload as Record<string, unknown>
  if (!Array.isArray(p.nodes) || !Array.isArray(p.edges)) return null
  return p as unknown as LineageMap
}

/**
 * Read one branch's lineage map. Every outcome is typed: a graph, a
 * declared refusal, or an honest "could not read". Nothing is invented
 * to fill a gap.
 */
export async function fetchLineageMap(branchId: string, limit = 500): Promise<LineageOutcome> {
  try {
    const resp = await fetch(`/api/timeline/lineage/${branchId}?limit=${limit}`)
    const body: unknown = await resp.json().catch(() => null)
    if (!resp.ok) {
      const refusal = asRefusal(body)
      return refusal
        ? { state: 'refused', refusal }
        : { state: 'unreachable', reason: `HTTP ${resp.status}` }
    }
    const map = asMap(body)
    return map
      ? { state: 'graph', map }
      : { state: 'unreachable', reason: 'response was not a lineage map' }
  } catch (err) {
    return { state: 'unreachable', reason: err instanceof Error ? err.message : 'network error' }
  }
}

/**
 * The entity this operation left behind, preferring a top-level solid
 * over the sub-entities (fillet faces, edges) that ride along with it.
 * `null` when the operation produced nothing.
 */
export function resultRef(node: LineageMapNode): string | null {
  const solid = node.outputs.find((r) => r.startsWith('solid:'))
  return solid ?? node.outputs[0] ?? null
}

/** A live part name when the viewport knows one, else the canonical ref. */
export function entityLabel(ref: string, liveNames: Map<string, string>): string {
  return liveNames.get(ref) ?? ref
}

// ─── Durability boot outcome (GET /api/durability/status) ──────────
//
// Mirrors `DurabilityStatus` in `api-server/src/durability.rs` (serde
// tag = "state", snake_case). `quarantined` is the honest-refusal case:
// the kernel found an event it cannot faithfully replay, serves the
// clean prefix, and REFUSES the tail rather than approximating it.
// That withholding is by design and must be visible — a user looking at
// the timeline has to be able to see that history is being withheld,
// and why — but it is never an error to "fix" from the client side.

export type DurabilityStatus =
  | { state: 'disabled' }
  | { state: 'empty' }
  | { state: 'active'; events_replayed: number }
  | {
      state: 'quarantined'
      first_break_sequence: number
      first_break_kind: string
      reason: string
      events_served: number
      events_total: number
    }
  | { state: 'failed'; reason: string }

/**
 * Parse the `GET /api/durability/status` payload. The endpoint wraps the
 * typed union: `{ session_id, durability_enabled, quarantined,
 * status: DurabilityStatus }` (main.rs::durability_status_endpoint) —
 * verified live 2026-08-01; parsing the top level for a `state` tag
 * silently yields null. A bare union is also accepted so a future
 * unwrapped emitter keeps working. Anything else (or an unknown tag)
 * → null, never a guessed status.
 */
export function parseDurabilityStatus(payload: unknown): DurabilityStatus | null {
  const candidate =
    payload && typeof payload === 'object' && 'state' in payload
      ? payload
      : payload && typeof payload === 'object' && 'status' in payload
        ? (payload as { status: unknown }).status
        : null
  if (!candidate || typeof candidate !== 'object' || !('state' in candidate)) return null
  const state = (candidate as { state: unknown }).state
  const known = ['disabled', 'empty', 'active', 'quarantined', 'failed']
  if (typeof state !== 'string' || !known.includes(state)) return null
  return candidate as DurabilityStatus
}

export interface DurabilityNotice {
  /** amber = history withheld · red = log unreadable · neutral = persistence off */
  tone: 'withheld' | 'failed' | 'off'
  /** Short enough to sit in the strip and be read without a mouse. */
  label: string
  /** The full account, for hover. */
  detail: string
}

/**
 * `null` for the calm cases (full replay, empty log) — the strip stays
 * quiet when there is nothing to disclose. Non-null exactly when the
 * boot outcome withheld or lost something the user would otherwise
 * assume is there.
 */
export function durabilityNotice(
  status: DurabilityStatus | null | undefined,
): DurabilityNotice | null {
  if (!status) return null
  switch (status.state) {
    case 'quarantined':
      return {
        tone: 'withheld',
        label: `history: ${status.events_served}/${status.events_total} served`,
        detail:
          `Tail withheld at boot — by design. Event #${status.first_break_sequence} ` +
          `(${status.first_break_kind}) cannot be faithfully replayed by this kernel: ` +
          `${status.reason}. The clean prefix (${status.events_served} of ` +
          `${status.events_total} events) is served; everything at and after the break ` +
          `is refused rather than approximated.`,
      }
    case 'failed':
      return {
        tone: 'failed',
        label: 'history unreadable',
        detail:
          `The event log could not be read at boot: ${status.reason}. ` +
          `Serving a blank model rather than pretending the document is empty.`,
      }
    case 'disabled':
      return {
        tone: 'off',
        label: 'persistence off',
        detail:
          'ROSHERA_DURABILITY=off — nothing recorded this session survives a restart.',
      }
    case 'active':
    case 'empty':
      return null
  }
}

// ─── Kernel kind → symbol/label (terminal aesthetic) ────────────────
//
// `operation_type` is the clean kernel command name emitted by the
// timeline-engine bridge — e.g. "create_box_3d", "extrude_face",
// "boolean_operation", "fillet_edges". The legacy debug-string format
// (`Generic { command_type: "create_box_3d", ... }`) is still tolerated
// as a fallback so old timelines on disk render correctly.

export function normalizeKind(op: string): string {
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

export function symbolForOperation(op: string): string {
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

export function shortLabel(op: string): string {
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

export function formatTimestamp(ts: string): string {
  const d = new Date(ts)
  if (isNaN(d.getTime())) return ts
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

// Relative human time: "now", "5s", "3m", "2h", "4d"
export function relativeTime(ts: string): string {
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

export function formatAuthor(author: string): string {
  // New backend already emits clean strings ("Varun", "Claude", "System").
  // Legacy Debug strings ("User { id: 1, name: Varun }") still parsed as
  // a fallback so persisted timelines render the right name.
  if (!author) return '?'
  const nameMatch = author.match(/name:\s*(\w+)/)
  if (nameMatch) return nameMatch[1]
  return author
}

export function authorKind(author: string, hint?: AuthorKind): AuthorKind {
  // Prefer the backend-supplied classification when present.
  if (hint === 'user' || hint === 'ai' || hint === 'system') return hint
  if (author === 'System') return 'system'
  if (author.includes('AIAgent') || author.includes('AI')) return 'ai'
  if (author.includes('User') || author.includes('name:')) return 'user'
  return 'system'
}

export function authorGlyph(kind: AuthorKind): string {
  switch (kind) {
    case 'user': return 'Ⓤ'
    case 'ai': return 'Ⓒ'
    case 'system': return '§'
  }
}

// Tailwind class for author-tinted text. User = primary, AI = amber, System = muted.
export function authorTextClass(kind: AuthorKind, isLatest: boolean): string {
  const base = (() => {
    switch (kind) {
      case 'user': return 'text-primary'
      case 'ai': return 'text-amber-400'
      case 'system': return 'text-muted-foreground'
    }
  })()
  return isLatest ? base : `${base}/70`
}
