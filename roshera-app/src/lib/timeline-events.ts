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
