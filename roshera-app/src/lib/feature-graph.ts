/**
 * The timeline Map as one feature graph (Varun 2026-11-18: "one look at map
 * should tell everything ... how it was made .. what was agent / what was
 * human ... can it be used to rebuild the part").
 *
 * The data comes from the kernel: `GET /api/timeline/feature-graph/{branch}`
 * (features, what each was built on, sizes, the recipe, a summary) and
 * `POST /api/timeline/rebuild-check/{branch}` (the recipe replayed into a
 * fresh model and measured against the live part). This module is the pure
 * layer the Map draws from: the layout, the summary line, who made each
 * feature, the rebuild result in words. No kernel ids reach any text here.
 */
import dagre from 'dagre'

export interface FeatureAuthorDetail {
  kind: string
  name?: string
  model?: string
}

/** One feature (a chapter of the history) as the Map shows it. */
export interface MapFeature {
  anchor: number
  kind?: string
  name?: string | null
  display_name?: string
  sizes?: string[]
  author: 'user' | 'agent' | 'system' | string
  author_detail: FeatureAuthorDetail
  intent?: string | null
  sound?: boolean
  refusals?: { sequence: number; kind: string; status: string; reason?: string }[]
}

/** Feature `to` was built on feature `from`; `via` says what in words. */
export interface MapEdge {
  from: number
  to: number
  via: string
}

export interface MapSummary {
  features: number
  by_agent: number
  by_user: number
  steps_rebuildable: number
  steps_total: number
  set_aside: number
  branch?: string
}

export interface FeatureGraph {
  features: MapFeature[]
  edges: MapEdge[]
  summary: MapSummary
  recipe: string[]
}

export interface PartMeasure {
  volume?: number
  area?: number
  bbox_min?: [number, number, number]
  bbox_max?: [number, number, number]
  faces?: number
  edges?: number
  vertices?: number
  sound?: boolean
}

export interface RebuildCheck {
  identical: boolean
  parts: { name: string; live?: PartMeasure | null; rebuilt?: PartMeasure | null; differences: string[] }[]
  steps_replayed: number
  steps_set_aside?: number
  elapsed_ms?: number
}

/** Size of a feature node on the canvas (picture + four lines of text). */
export const NODE_WIDTH = 200
export const NODE_HEIGHT = 190

/**
 * Where each feature sits: left to right in build order. Wired features rank
 * by what they were built on; ties and unwired features keep the order they
 * were made in (anchor).
 */
export function layoutFeatureGraph(
  features: Pick<MapFeature, 'anchor'>[],
  edges: MapEdge[],
): Map<number, { x: number; y: number }> {
  const g = new dagre.graphlib.Graph()
  g.setGraph({ rankdir: 'LR', ranksep: 60, nodesep: 30, marginx: 16, marginy: 16 })
  g.setDefaultEdgeLabel(() => ({}))
  const ordered = [...features].sort((a, b) => a.anchor - b.anchor)
  for (const f of ordered) g.setNode(String(f.anchor), { width: NODE_WIDTH, height: NODE_HEIGHT })
  const known = new Set(ordered.map((f) => f.anchor))
  const wired = new Set<number>()
  for (const e of edges) {
    if (!known.has(e.from) || !known.has(e.to)) continue
    g.setEdge(String(e.from), String(e.to))
    wired.add(e.from)
    wired.add(e.to)
  }
  // Unwired features follow the build order: an invisible chain through them
  // keeps a feature made later to the right of one made earlier.
  for (let i = 1; i < ordered.length; i++) {
    const a = ordered[i - 1].anchor
    const b = ordered[i].anchor
    if (!g.hasEdge(String(a), String(b)) && !(wired.has(a) && wired.has(b))) {
      g.setEdge(String(a), String(b), { weight: 0, minlen: 1 })
    }
  }
  dagre.layout(g)
  const at = new Map<number, { x: number; y: number }>()
  for (const f of ordered) {
    const n = g.node(String(f.anchor))
    at.set(f.anchor, { x: n.x - NODE_WIDTH / 2, y: n.y - NODE_HEIGHT / 2 })
  }
  return at
}

/** "6 features · 4 ✦ agent · 2 by people · rebuilds ✓ 35/51 steps" */
export function summaryLine(s: MapSummary): string {
  const plural = s.features === 1 ? 'feature' : 'features'
  return (
    `${s.features} ${plural} · ${s.by_agent} ✦ agent · ${s.by_user} by people · ` +
    `rebuilds ✓ ${s.steps_rebuildable}/${s.steps_total} steps`
  )
}

/** The agent's ✦ with its model on hover; a person's initials with their name. */
export function authorMark(f: Pick<MapFeature, 'author' | 'author_detail'>): { glyph: string; title: string } {
  if (f.author === 'agent') {
    return { glyph: '✦', title: `Made by the agent${f.author_detail.model ? ` (${f.author_detail.model})` : ''}` }
  }
  if (f.author === 'user') {
    const name = f.author_detail.name?.trim() || 'a person'
    const initials = name
      .split(/\s+/)
      .filter(Boolean)
      .map((w) => w[0]?.toUpperCase() ?? '')
      .join('')
      .slice(0, 2)
    return { glyph: initials || '•', title: `Made by ${name}` }
  }
  return { glyph: '·', title: 'Made by the system' }
}

/** The rebuild result in a sentence: identical only when every measure matched. */
export function rebuildLine(r: RebuildCheck): string {
  if (r.identical) {
    const faces = r.parts.reduce((n, p) => n + (p.live?.faces ?? 0), 0)
    return (
      `Rebuilt from the recipe: identical ✓ — volume, area, box and ${faces} faces match ` +
      `(${r.steps_replayed} steps replayed)`
    )
  }
  const diffs = r.parts
    .filter((p) => p.differences.length > 0)
    .map((p) => `${p.name}: ${p.differences.join(', ')}`)
  return `Rebuilt from the recipe: differs ✗ — ${diffs.join('; ')}`
}
