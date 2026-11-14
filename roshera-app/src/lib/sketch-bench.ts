/**
 * The sketch workbench's drawing logic (Varun 2026-11-14: "the sketching
 * is severely underworked ... nothing compared to how state of art works").
 *
 * Pure functions, no React and no network:
 *  - `planShape`: what a tool's clicks BUILD. Real sketch geometry, as a CAD
 *    sketcher builds it -- shared corner points, lines between them, and the
 *    constraints added as you draw (a rectangle is held square, a level line
 *    is Horizontal) -- so a sketch can be dimensioned and edited afterwards.
 *    The plan is a list of steps the store sends in order; later steps name
 *    earlier ones by key (`k:<n>`) or an existing point by `id:<uuid>`.
 *  - `snap`, `inferAxis`: where a click lands.
 *  - `entityPaths`, `pickEntity`: what to draw, and what a click hits.
 */

import type { CSketchSummary, EntityRef, GeometricConstraint } from './csketch-api'

export type Vec2 = [number, number]

/** The draw tools, plus Select. */
export type BenchTool =
  | 'select'
  | 'line'
  | 'centerline'
  | 'rectangle'
  | 'center-rectangle'
  | 'circle'
  | 'arc3'
  | 'arc-center'
  | 'polygon'
  | 'slot'
  | 'spline'
  | 'point'

/** Clicks a tool needs; `chain` keeps going until Esc / Enter. */
export const TOOL_CLICKS: Record<BenchTool, number | 'chain'> = {
  select: 0,
  line: 'chain',
  centerline: 2,
  rectangle: 2,
  'center-rectangle': 2,
  circle: 2,
  arc3: 3,
  'arc-center': 3,
  polygon: 2,
  slot: 3,
  spline: 'chain',
  point: 1,
}

/** A click, snapped; `pointId` when it landed on an existing point. */
export interface Click {
  at: Vec2
  pointId?: string
}

/** A reference to a sketch point: a step's key, or `id:<existing uuid>`. */
export type Ref = string

export type PlanOp =
  | { op: 'point'; key: Ref; at: Vec2 }
  | { op: 'line'; key: Ref; a: Ref; b: Ref; construction?: boolean }
  | { op: 'circle'; key: Ref; c: Vec2; r: number }
  | { op: 'arc'; key: Ref; c: Vec2; r: number; start: number; end: number }
  | { op: 'spline'; key: Ref; points: Ref[]; degree: number }
  | {
      op: 'constrain'
      kind: GeometricConstraint
      refs: { kind: 'Point' | 'Line' | 'Arc' | 'Circle'; ref: Ref }[]
    }

/** Below this a size is no size (sketch units are millimetres). */
const EPS = 1e-6
/** A line within this angle of level / plumb is drawn level / plumb. */
const AXIS_SNAP_RAD = (3 * Math.PI) / 180
const TAU = 2 * Math.PI

const norm = (a: number) => ((a % TAU) + TAU) % TAU

/** Level / plumb inference for a line a -> b. */
export function inferAxis(a: Vec2, b: Vec2): 'Horizontal' | 'Vertical' | null {
  const dx = b[0] - a[0]
  const dy = b[1] - a[1]
  if (Math.hypot(dx, dy) < EPS) return null
  const ang = Math.abs(Math.atan2(dy, dx))
  if (ang < AXIS_SNAP_RAD || Math.PI - ang < AXIS_SNAP_RAD) return 'Horizontal'
  if (Math.abs(ang - Math.PI / 2) < AXIS_SNAP_RAD) return 'Vertical'
  return null
}

/** The arc from `a` to `b` through `m`, counter-clockwise from `start`
 *  to `end` (radians, start in [0, 2pi), end > start); null if collinear. */
export function arcThrough(a: Vec2, m: Vec2, b: Vec2): { c: Vec2; r: number; start: number; end: number } | null {
  const d = 2 * (a[0] * (m[1] - b[1]) + m[0] * (b[1] - a[1]) + b[0] * (a[1] - m[1]))
  if (Math.abs(d) < EPS) return null
  const a2 = a[0] * a[0] + a[1] * a[1]
  const m2 = m[0] * m[0] + m[1] * m[1]
  const b2 = b[0] * b[0] + b[1] * b[1]
  const cx = (a2 * (m[1] - b[1]) + m2 * (b[1] - a[1]) + b2 * (a[1] - m[1])) / d
  const cy = (a2 * (b[0] - m[0]) + m2 * (a[0] - b[0]) + b2 * (m[0] - a[0])) / d
  const r = Math.hypot(a[0] - cx, a[1] - cy)
  const angA = norm(Math.atan2(a[1] - cy, a[0] - cx))
  const angM = norm(Math.atan2(m[1] - cy, m[0] - cx))
  const angB = norm(Math.atan2(b[1] - cy, b[0] - cx))
  // Counter-clockwise from A, is M met before B? Then A -> B; else B -> A.
  const toM = norm(angM - angA)
  const toB = norm(angB - angA)
  const [s, e] = toM < toB ? [angA, angB] : [angB, angA]
  return { c: [cx, cy], r, start: s, end: s + norm(e - s) }
}

/** Builds plan steps with fresh keys. */
class Planner {
  ops: PlanOp[] = []
  private n = 0
  key() {
    return `k:${this.n++}`
  }
  point(c: Click): Ref {
    if (c.pointId) return `id:${c.pointId}`
    const key = this.key()
    this.ops.push({ op: 'point', key, at: c.at })
    return key
  }
  line(a: Ref, b: Ref, construction = false): Ref {
    const key = this.key()
    this.ops.push(construction ? { op: 'line', key, a, b, construction } : { op: 'line', key, a, b })
    return key
  }
  constrain(kind: GeometricConstraint, refs: { kind: 'Point' | 'Line' | 'Arc' | 'Circle'; ref: Ref }[]) {
    this.ops.push({ op: 'constrain', kind, refs })
  }
  /** A closed loop through `corners`; returns the line keys. */
  loop(corners: Vec2[]): Ref[] {
    const pts = corners.map((at) => this.point({ at }))
    return pts.map((p, i) => this.line(p, pts[(i + 1) % pts.length]))
  }
}

/** A rectangle's corners, counter-clockwise from (x0, y0). */
function rectCorners(x0: number, y0: number, x1: number, y1: number): Vec2[] {
  return [
    [x0, y0],
    [x1, y0],
    [x1, y1],
    [x0, y1],
  ]
}

/** What a tool's clicks build. Empty when the clicks make no shape. */
export function planShape(tool: BenchTool, clicks: Click[], opts: { sides?: number } = {}): PlanOp[] {
  const p = new Planner()
  const at = (i: number) => clicks[i]?.at
  switch (tool) {
    case 'point': {
      if (!clicks[0] || clicks[0].pointId) return []
      p.point(clicks[0])
      return p.ops
    }
    case 'line':
    case 'centerline': {
      const [c0, c1] = clicks
      if (!c0 || !c1) return []
      const axis = inferAxis(c0.at, c1.at)
      let end = c1
      // Drawn level / plumb, unless it ends ON an existing point.
      if (axis && !c1.pointId) {
        end = { at: axis === 'Horizontal' ? [c1.at[0], c0.at[1]] : [c0.at[0], c1.at[1]] }
      }
      if (Math.hypot(end.at[0] - c0.at[0], end.at[1] - c0.at[1]) < EPS) return []
      const a = p.point(c0)
      const b = p.point(end)
      const l = p.line(a, b, tool === 'centerline')
      if (axis) p.constrain(axis, [{ kind: 'Line', ref: l }])
      return p.ops
    }
    case 'rectangle':
    case 'center-rectangle': {
      const a = at(0)
      const b = at(1)
      if (!a || !b) return []
      const [x0, y0, x1, y1] =
        tool === 'rectangle'
          ? [a[0], a[1], b[0], b[1]]
          : [2 * a[0] - b[0], 2 * a[1] - b[1], b[0], b[1]]
      if (Math.abs(x1 - x0) < EPS || Math.abs(y1 - y0) < EPS) return []
      const [l0, l1, l2, l3] = p.loop(
        rectCorners(Math.min(x0, x1), Math.min(y0, y1), Math.max(x0, x1), Math.max(y0, y1)),
      )
      p.constrain('Horizontal', [{ kind: 'Line', ref: l0 }])
      p.constrain('Vertical', [{ kind: 'Line', ref: l1 }])
      p.constrain('Horizontal', [{ kind: 'Line', ref: l2 }])
      p.constrain('Vertical', [{ kind: 'Line', ref: l3 }])
      return p.ops
    }
    case 'circle': {
      const c = at(0)
      const e = at(1)
      if (!c || !e) return []
      const r = Math.hypot(e[0] - c[0], e[1] - c[1])
      if (r < EPS) return []
      p.ops.push({ op: 'circle', key: p.key(), c, r })
      return p.ops
    }
    case 'arc3': {
      // Start, end, then a point on the arc.
      const a = at(0)
      const b = at(1)
      const m = at(2)
      if (!a || !b || !m) return []
      const arc = arcThrough(a, m, b)
      if (!arc) return []
      p.ops.push({ op: 'arc', key: p.key(), ...arc })
      return p.ops
    }
    case 'arc-center': {
      // Centre, start (sets the radius), then the end direction (CCW).
      const c = at(0)
      const s = at(1)
      const e = at(2)
      if (!c || !s || !e) return []
      const r = Math.hypot(s[0] - c[0], s[1] - c[1])
      if (r < EPS) return []
      const start = norm(Math.atan2(s[1] - c[1], s[0] - c[0]))
      const sweep = norm(Math.atan2(e[1] - c[1], e[0] - c[0]) - start)
      if (sweep < EPS) return []
      p.ops.push({ op: 'arc', key: p.key(), c, r, start, end: start + sweep })
      return p.ops
    }
    case 'polygon': {
      const c = at(0)
      const v = at(1)
      const n = Math.max(3, Math.min(64, Math.round(opts.sides ?? 6)))
      if (!c || !v) return []
      const r = Math.hypot(v[0] - c[0], v[1] - c[1])
      if (r < EPS) return []
      const a0 = Math.atan2(v[1] - c[1], v[0] - c[0])
      const corners: Vec2[] = Array.from({ length: n }, (_, i) => [
        c[0] + r * Math.cos(a0 + (i * TAU) / n),
        c[1] + r * Math.sin(a0 + (i * TAU) / n),
      ])
      const lines = p.loop(corners)
      for (let i = 1; i < lines.length; i++) {
        p.constrain('Equal', [
          { kind: 'Line', ref: lines[0] },
          { kind: 'Line', ref: lines[i] },
        ])
      }
      return p.ops
    }
    case 'slot': {
      // The two end centres, then a point that sets the half-width.
      const a = at(0)
      const b = at(1)
      const w = at(2)
      if (!a || !b || !w) return []
      const len = Math.hypot(b[0] - a[0], b[1] - a[1])
      if (len < EPS) return []
      const ux = (b[0] - a[0]) / len
      const uy = (b[1] - a[1]) / len
      const nx = -uy
      const ny = ux
      const r = Math.abs((w[0] - a[0]) * nx + (w[1] - a[1]) * ny)
      if (r < EPS) return []
      const pa = p.point({ at: [a[0] + nx * r, a[1] + ny * r] })
      const pb = p.point({ at: [b[0] + nx * r, b[1] + ny * r] })
      const qb = p.point({ at: [b[0] - nx * r, b[1] - ny * r] })
      const qa = p.point({ at: [a[0] - nx * r, a[1] - ny * r] })
      const top = p.line(pa, pb)
      const bottom = p.line(qb, qa)
      const dir = Math.atan2(uy, ux)
      // b's end: from -90 deg to +90 deg about the slot axis; a's: the other half.
      const sb = norm(dir - Math.PI / 2)
      const sa = norm(dir + Math.PI / 2)
      p.ops.push({ op: 'arc', key: p.key(), c: b, r, start: sb, end: sb + Math.PI })
      p.ops.push({ op: 'arc', key: p.key(), c: a, r, start: sa, end: sa + Math.PI })
      p.constrain('Parallel', [
        { kind: 'Line', ref: top },
        { kind: 'Line', ref: bottom },
      ])
      return p.ops
    }
    case 'spline': {
      if (clicks.length < 2) return []
      const pts = clicks.map((c) => p.point(c))
      p.ops.push({ op: 'spline', key: p.key(), points: pts, degree: Math.min(3, pts.length - 1) })
      return p.ops
    }
    case 'select':
      return []
  }
}

// ── Snapping ─────────────────────────────────────────────────────────

export type SnapKind = 'endpoint' | 'midpoint' | 'center' | 'grid' | 'none'

export interface SnapTarget {
  at: Vec2
  kind: Exclude<SnapKind, 'grid' | 'none'>
  /** The point entity, when the target IS one (a click there reuses it). */
  pointId?: string
}

/** Where a cursor lands: the nearest target within `reach`, else the grid
 *  (`grid` = 0 turns grid snapping off). */
export function snap(cursor: Vec2, targets: SnapTarget[], opts: { reach: number; grid: number }): Click & { kind: SnapKind } {
  let best: SnapTarget | null = null
  let bestD = opts.reach
  for (const t of targets) {
    const d = Math.hypot(t.at[0] - cursor[0], t.at[1] - cursor[1])
    // Points first: an end point beats a midpoint at the same distance.
    if (d < bestD || (best && d === bestD && t.kind === 'endpoint')) {
      best = t
      bestD = d
    }
  }
  if (best) return { at: best.at, kind: best.kind, ...(best.pointId ? { pointId: best.pointId } : {}) }
  if (opts.grid > 0) {
    const g = opts.grid
    return { at: [Math.round(cursor[0] / g) * g, Math.round(cursor[1] / g) * g], kind: 'grid' }
  }
  return { at: cursor, kind: 'none' }
}

/** Snap targets of a sketch: points, line midpoints, circle / arc centres. */
export function snapTargets(s: CSketchSummary): SnapTarget[] {
  const out: SnapTarget[] = []
  for (const p of s.points) out.push({ at: [p.x, p.y], kind: 'endpoint', pointId: p.id })
  for (const l of s.lines) {
    if ('Segment' in l.geometry) {
      const { start, end } = l.geometry.Segment
      out.push({ at: [(start.x + end.x) / 2, (start.y + end.y) / 2], kind: 'midpoint' })
    }
  }
  for (const c of s.circles) out.push({ at: [c.cx, c.cy], kind: 'center' })
  for (const a of s.arcs ?? []) out.push({ at: [a.cx, a.cy], kind: 'center' })
  return out
}

// ── Drawing and picking ──────────────────────────────────────────────

export interface EntityPath {
  ref: EntityRef
  pts: Vec2[]
  construction: boolean
}

const SEGMENTS = 64

function arcPts(cx: number, cy: number, r: number, a0: number, a1: number): Vec2[] {
  const n = Math.max(8, Math.ceil((SEGMENTS * Math.abs(a1 - a0)) / TAU))
  return Array.from({ length: n + 1 }, (_, i) => {
    const t = a0 + ((a1 - a0) * i) / n
    return [cx + r * Math.cos(t), cy + r * Math.sin(t)] as Vec2
  })
}

/** de Boor evaluation of a (rational) B-spline at u. */
function deBoor(degree: number, knots: number[], cps: Vec2[], w: number[] | null, u: number): Vec2 {
  const n = cps.length - 1
  let k = degree
  while (k < n && u >= knots[k + 1]) k++
  const d: [number, number, number][] = []
  for (let j = 0; j <= degree; j++) {
    const i = k - degree + j
    const wi = w ? w[i] : 1
    d.push([cps[i][0] * wi, cps[i][1] * wi, wi])
  }
  for (let r = 1; r <= degree; r++) {
    for (let j = degree; j >= r; j--) {
      const i = k - degree + j
      const den = knots[i + degree - r + 1] - knots[i]
      const a = den === 0 ? 0 : (u - knots[i]) / den
      d[j] = [
        (1 - a) * d[j - 1][0] + a * d[j][0],
        (1 - a) * d[j - 1][1] + a * d[j][1],
        (1 - a) * d[j - 1][2] + a * d[j][2],
      ]
    }
  }
  const [x, y, ww] = d[degree]
  return [x / ww, y / ww]
}

/** A drawn path for every curve in the sketch (points are drawn as dots). */
export function entityPaths(s: CSketchSummary): EntityPath[] {
  const out: EntityPath[] = []
  for (const l of s.lines) {
    if (!('Segment' in l.geometry)) continue
    const { start, end } = l.geometry.Segment
    out.push({ ref: { Line: l.id }, pts: [[start.x, start.y], [end.x, end.y]], construction: l.is_construction })
  }
  for (const c of s.circles) {
    out.push({ ref: { Circle: c.id }, pts: arcPts(c.cx, c.cy, c.radius, 0, TAU), construction: c.is_construction })
  }
  for (const a of s.arcs ?? []) {
    const sweep = a.ccw ? norm(a.end_angle - a.start_angle) || TAU : -(norm(a.start_angle - a.end_angle) || TAU)
    out.push({
      ref: { Arc: a.id },
      pts: arcPts(a.cx, a.cy, a.radius, a.start_angle, a.start_angle + sweep),
      construction: a.is_construction,
    })
  }
  for (const r of s.rectangles ?? []) {
    const c = Math.cos(r.rotation)
    const sn = Math.sin(r.rotation)
    const corner = (u: number, v: number): Vec2 => [r.cx + u * c - v * sn, r.cy + u * sn + v * c]
    const w = r.width / 2
    const h = r.height / 2
    out.push({
      ref: { Rectangle: r.id },
      pts: [corner(-w, -h), corner(w, -h), corner(w, h), corner(-w, h), corner(-w, -h)],
      construction: r.is_construction,
    })
  }
  for (const e of s.ellipses ?? []) {
    const c = Math.cos(e.rotation)
    const sn = Math.sin(e.rotation)
    const pts: Vec2[] = Array.from({ length: SEGMENTS + 1 }, (_, i) => {
      const t = (i * TAU) / SEGMENTS
      const u = e.semi_major * Math.cos(t)
      const v = e.semi_minor * Math.sin(t)
      return [e.cx + u * c - v * sn, e.cy + u * sn + v * c]
    })
    out.push({ ref: { Ellipse: e.id }, pts, construction: e.is_construction })
  }
  for (const pl of s.polylines ?? []) {
    const pts = pl.vertices.map((v) => [v[0], v[1]] as Vec2)
    if (pl.closed && pts.length > 0) pts.push(pts[0])
    out.push({ ref: { Polyline: pl.id }, pts, construction: pl.is_construction })
  }
  for (const sp of s.splines ?? []) {
    const cps = sp.control_points.map((c) => [c[0], c[1]] as Vec2)
    const k = sp.knots
    if (cps.length < 2 || k.length !== cps.length + sp.degree + 1) continue
    const u0 = k[sp.degree]
    const u1 = k[cps.length]
    const pts = Array.from({ length: SEGMENTS + 1 }, (_, i) =>
      deBoor(sp.degree, k, cps, sp.weights, Math.min(u0 + ((u1 - u0) * i) / SEGMENTS, u1 - 1e-12)),
    )
    pts[SEGMENTS] = cps[cps.length - 1]
    out.push({ ref: { Spline: sp.id }, pts, construction: sp.is_construction })
  }
  return out
}

function segDist(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b[0] - a[0]
  const dy = b[1] - a[1]
  const L = dx * dx + dy * dy
  const t = L === 0 ? 0 : Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L))
  return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy))
}

/** What a click hits: a point first (within reach), else the nearest curve. */
export function pickEntity(cursor: Vec2, paths: EntityPath[], s: CSketchSummary, reach: number): EntityRef | null {
  let best: EntityRef | null = null
  let bestD = reach
  for (const p of s.points) {
    const d = Math.hypot(p.x - cursor[0], p.y - cursor[1])
    if (d <= bestD) {
      best = { Point: p.id }
      bestD = d
    }
  }
  if (best) return best
  for (const path of paths) {
    for (let i = 1; i < path.pts.length; i++) {
      const d = segDist(cursor, path.pts[i - 1], path.pts[i])
      if (d <= bestD) {
        best = path.ref
        bestD = d
      }
    }
  }
  return best
}

/** Two references to the same entity? */
export function sameRef(a: EntityRef, b: EntityRef): boolean {
  const [ka, va] = Object.entries(a)[0]
  const [kb, vb] = Object.entries(b)[0]
  return ka === kb && va === vb
}

/** A plan as drawable paths: the rubber band while drawing, and the shape
 *  shown at once while the server builds it. `known` places existing
 *  points (`id:<uuid>` references) by id. */
export function planPaths(plan: PlanOp[], known: Map<string, Vec2>): Vec2[][] {
  const at = new Map<string, Vec2>()
  for (const op of plan) if (op.op === 'point') at.set(op.key, op.at)
  const where = (ref: Ref): Vec2 | undefined => (ref.startsWith('id:') ? known.get(ref.slice(3)) : at.get(ref))
  const out: Vec2[][] = []
  for (const op of plan) {
    if (op.op === 'line') {
      const a = where(op.a)
      const b = where(op.b)
      if (a && b) out.push([a, b])
    } else if (op.op === 'circle') {
      out.push(arcPts(op.c[0], op.c[1], op.r, 0, TAU))
    } else if (op.op === 'arc') {
      out.push(arcPts(op.c[0], op.c[1], op.r, op.start, op.end))
    } else if (op.op === 'spline') {
      // The control polygon: the curve itself is the server's to evaluate.
      const pts = op.points.map(where).filter((p): p is Vec2 => p !== undefined)
      if (pts.length >= 2) out.push(pts)
    }
  }
  return out
}

/** The freedom chip: the solver's VERDICT (`status`), never the raw
 *  pre-constraint count; nothing for an empty sketch. */
export function freedomLabel(
  dof: { status: { kind: string; dofs?: number; conflicting_constraints?: number } } | null,
  empty: boolean,
): { text: string; tone: 'free' | 'done' | 'conflict' } | null {
  if (!dof || empty) return null
  const s = dof.status
  if (s.kind === 'fully_constrained') return { text: 'Fully defined', tone: 'done' }
  if (s.kind === 'over_constrained') return { text: `${s.conflicting_constraints ?? 0} conflicting`, tone: 'conflict' }
  return { text: `${s.dofs ?? 0} free`, tone: 'free' }
}
