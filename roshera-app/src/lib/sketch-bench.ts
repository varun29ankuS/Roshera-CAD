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
  // Modify tools: a click picks what they act on.
  | 'trim'
  | 'extend'
  | 'fillet'
  | 'chamfer'
  | 'mirror'
  | 'circular-pattern'
  | 'dimension'

/** Tools whose click PICKS an existing entity (or place) to act on. */
export const PICK_TOOLS: ReadonlySet<BenchTool> = new Set<BenchTool>([
  'trim',
  'extend',
  'fillet',
  'chamfer',
  'mirror',
  'circular-pattern',
])

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
  trim: 0,
  extend: 0,
  fillet: 0,
  chamfer: 0,
  mirror: 0,
  'circular-pattern': 0,
  dimension: 0,
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
  /** An arc ON its end points `a` -> `b` (counter-clockwise when `ccw`);
   *  centre and angles are what it draws as until the server answers. */
  | { op: 'arc'; key: Ref; a: Ref; b: Ref; c: Vec2; r: number; start: number; end: number; ccw: boolean; large: boolean }
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
  /** An arc on new end points at `start` / `end` (CCW, radians). */
  arcOnNewPoints(c: Vec2, r: number, start: number, end: number): Ref {
    const at = (t: number): Vec2 => [c[0] + r * Math.cos(t), c[1] + r * Math.sin(t)]
    const a = this.point({ at: at(start) })
    const b = this.point({ at: at(end) })
    return this.arcOn(a, b, c, r, start, end)
  }
  /** An arc on existing plan points `a` -> `b`, CCW from `start` to `end`. */
  arcOn(a: Ref, b: Ref, c: Vec2, r: number, start: number, end: number): Ref {
    const key = this.key()
    this.ops.push({ op: 'arc', key, a, b, c, r, start, end, ccw: true, large: end - start > Math.PI + 1e-9 })
    return key
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
      p.arcOnNewPoints(arc.c, arc.r, arc.start, arc.end)
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
      p.arcOnNewPoints(c, r, start, start + sweep)
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
      // The ends are half circles ON the sides' end points: b's runs CCW
      // from its lower corner to its upper; a's from upper to lower.
      const sb = norm(dir - Math.PI / 2)
      const sa = norm(dir + Math.PI / 2)
      p.arcOn(qb, pb, b, r, sb, sb + Math.PI)
      p.arcOn(pa, qa, a, r, sa, sa + Math.PI)
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
    case 'trim':
    case 'extend':
    case 'fillet':
    case 'chamfer':
    case 'mirror':
    case 'circular-pattern':
    case 'dimension':
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

// ── Constraints, dimensions, crossings, modify (slice 2) ──────────────

type Kind = 'Point' | 'Line' | 'Arc' | 'Circle' | 'Rectangle' | 'Ellipse' | 'Spline' | 'Polyline'
const kindOf = (r: EntityRef): Kind => Object.keys(r)[0] as Kind
const idOf = (r: EntityRef): string => Object.values(r)[0] as string
const isRound = (k: Kind) => k === 'Arc' || k === 'Circle'

/** A constraint the selection can take, with its entities in the order
 *  the solver reads them. */
export interface ConstraintOffer {
  kind: GeometricConstraint
  entities: EntityRef[]
}

/** The constraints that fit what is selected (Varun 2026-11-14: "there is
 *  no way to constraint two lines"). */
export function constraintsFor(sel: EntityRef[]): ConstraintOffer[] {
  const pts = sel.filter((r) => kindOf(r) === 'Point')
  const lines = sel.filter((r) => kindOf(r) === 'Line')
  const rounds = sel.filter((r) => isRound(kindOf(r)))
  const offer = (kind: GeometricConstraint, entities: EntityRef[]): ConstraintOffer => ({ kind, entities })
  if (sel.length === 1 && lines.length === 1) return [offer('Horizontal', sel), offer('Vertical', sel)]
  if (sel.length === 2) {
    if (lines.length === 2) return [offer('Parallel', lines), offer('Perpendicular', lines), offer('Equal', lines)]
    if (pts.length === 2) return [offer('Coincident', pts)]
    if (pts.length === 1 && lines.length === 1) {
      return [offer('PointOnCurve', [pts[0], lines[0]]), offer('Midpoint', [pts[0], lines[0]])]
    }
    if (pts.length === 1 && rounds.length === 1) return [offer('PointOnCurve', [pts[0], rounds[0]])]
    if (lines.length === 1 && rounds.length === 1) return [offer('Tangent', [rounds[0], lines[0]])]
    if (rounds.length === 2) return [offer('Concentric', rounds), offer('Equal', rounds), offer('Tangent', rounds)]
  }
  if (sel.length === 3 && pts.length === 2 && lines.length === 1) return [offer('Symmetric', [pts[0], pts[1], lines[0]])]
  return []
}

/** What Smart Dimension measures for a selection, and its size now. */
export interface DimensionOffer {
  kind: 'Length' | 'Distance' | 'Diameter' | 'Radius' | 'Angle'
  entities: EntityRef[]
  value: number
  unit: 'mm' | 'deg'
}

function segOf(s: CSketchSummary, id: string): [Vec2, Vec2] | null {
  const l = s.lines.find((x) => x.id === id)
  if (!l || !('Segment' in l.geometry)) return null
  const { start, end } = l.geometry.Segment
  return [
    [start.x, start.y],
    [end.x, end.y],
  ]
}

export function dimensionFor(sel: EntityRef[], s: CSketchSummary): DimensionOffer | null {
  if (sel.length === 1) {
    const r = sel[0]
    const k = kindOf(r)
    if (k === 'Line') {
      const g = segOf(s, idOf(r))
      if (!g) return null
      return { kind: 'Length', entities: sel, value: Math.hypot(g[1][0] - g[0][0], g[1][1] - g[0][1]), unit: 'mm' }
    }
    if (k === 'Circle') {
      const c = s.circles.find((x) => x.id === idOf(r))
      return c ? { kind: 'Diameter', entities: sel, value: 2 * c.radius, unit: 'mm' } : null
    }
    if (k === 'Arc') {
      const a = (s.arcs ?? []).find((x) => x.id === idOf(r))
      return a ? { kind: 'Radius', entities: sel, value: a.radius, unit: 'mm' } : null
    }
    return null
  }
  if (sel.length === 2) {
    const ks = sel.map(kindOf)
    if (ks[0] === 'Point' && ks[1] === 'Point') {
      const p = s.points.find((x) => x.id === idOf(sel[0]))
      const q = s.points.find((x) => x.id === idOf(sel[1]))
      return p && q ? { kind: 'Distance', entities: sel, value: Math.hypot(q.x - p.x, q.y - p.y), unit: 'mm' } : null
    }
    if (ks[0] === 'Line' && ks[1] === 'Line') {
      const a = segOf(s, idOf(sel[0]))
      const b = segOf(s, idOf(sel[1]))
      if (!a || !b) return null
      const ang = (g: [Vec2, Vec2]) => Math.atan2(g[1][1] - g[0][1], g[1][0] - g[0][0])
      let d = Math.abs(ang(a) - ang(b)) % Math.PI
      if (d > Math.PI / 2) d = Math.PI - d
      return { kind: 'Angle', entities: sel, value: (d * 180) / Math.PI, unit: 'deg' }
    }
    const pi = ks.indexOf('Point')
    const li = ks.indexOf('Line')
    if (pi >= 0 && li >= 0) {
      const p = s.points.find((x) => x.id === idOf(sel[pi]))
      const g = segOf(s, idOf(sel[li]))
      if (!p || !g) return null
      const dx = g[1][0] - g[0][0]
      const dy = g[1][1] - g[0][1]
      const dist = Math.abs((p.x - g[0][0]) * dy - (p.y - g[0][1]) * dx) / Math.hypot(dx, dy)
      return { kind: 'Distance', entities: [sel[pi], sel[li]], value: dist, unit: 'mm' }
    }
  }
  return null
}

// Crossings and overlaps

type SegCurve = { ref: EntityRef; kind: 'seg'; a: Vec2; b: Vec2 }
type CircCurve = { ref: EntityRef; kind: 'circ'; c: Vec2; r: number; a0: number; sweep: number }
type Curve = SegCurve | CircCurve

/** The PROFILE curves (construction geometry forms no profile). */
function curvesOf(s: CSketchSummary): Curve[] {
  const out: Curve[] = []
  for (const l of s.lines) {
    if (!('Segment' in l.geometry) || l.is_construction) continue
    const { start, end } = l.geometry.Segment
    out.push({ ref: { Line: l.id }, kind: 'seg', a: [start.x, start.y], b: [end.x, end.y] })
  }
  for (const c of s.circles) {
    if (!c.is_construction) out.push({ ref: { Circle: c.id }, kind: 'circ', c: [c.cx, c.cy], r: c.radius, a0: 0, sweep: TAU })
  }
  for (const a of s.arcs ?? []) {
    if (a.is_construction) continue
    const sweep = a.ccw ? norm(a.end_angle - a.start_angle) || TAU : norm(a.start_angle - a.end_angle) || TAU
    const a0 = a.ccw ? a.start_angle : a.end_angle
    out.push({ ref: { Arc: a.id }, kind: 'circ', c: [a.cx, a.cy], r: a.radius, a0: norm(a0), sweep })
  }
  return out
}

const TOL = 1e-6

function segParam(a: Vec2, b: Vec2, p: Vec2): number {
  const dx = b[0] - a[0]
  const dy = b[1] - a[1]
  return ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (dx * dx + dy * dy)
}

function onArc(c: CircCurve, p: Vec2): boolean {
  if (c.sweep >= TAU - 1e-9) return true
  return norm(Math.atan2(p[1] - c.c[1], p[0] - c.c[0]) - c.a0) <= c.sweep + 1e-9
}

function carrierHits(x: Curve, y: Curve): Vec2[] {
  if (x.kind === 'seg' && y.kind === 'seg') {
    const d1: Vec2 = [x.b[0] - x.a[0], x.b[1] - x.a[1]]
    const d2: Vec2 = [y.b[0] - y.a[0], y.b[1] - y.a[1]]
    const cr = d1[0] * d2[1] - d1[1] * d2[0]
    if (Math.abs(cr) < 1e-12) return []
    const t = ((y.a[0] - x.a[0]) * d2[1] - (y.a[1] - x.a[1]) * d2[0]) / cr
    return [[x.a[0] + t * d1[0], x.a[1] + t * d1[1]]]
  }
  if (x.kind === 'circ' && y.kind === 'seg') return carrierHits(y, x)
  if (x.kind === 'seg' && y.kind === 'circ') {
    const d: Vec2 = [x.b[0] - x.a[0], x.b[1] - x.a[1]]
    const f: Vec2 = [x.a[0] - y.c[0], x.a[1] - y.c[1]]
    const A = d[0] * d[0] + d[1] * d[1]
    const B = 2 * (f[0] * d[0] + f[1] * d[1])
    const C = f[0] * f[0] + f[1] * f[1] - y.r * y.r
    const disc = B * B - 4 * A * C
    if (disc < 0) return []
    const sq = Math.sqrt(disc)
    return [(-B - sq) / (2 * A), (-B + sq) / (2 * A)].map((t) => [x.a[0] + t * d[0], x.a[1] + t * d[1]] as Vec2)
  }
  if (x.kind === 'circ' && y.kind === 'circ') {
    const dx = y.c[0] - x.c[0]
    const dy = y.c[1] - x.c[1]
    const d = Math.hypot(dx, dy)
    if (d < 1e-12 || d > x.r + y.r || d < Math.abs(x.r - y.r)) return []
    const a = (x.r * x.r - y.r * y.r + d * d) / (2 * d)
    const h = Math.sqrt(Math.max(0, x.r * x.r - a * a))
    const mx = x.c[0] + (a * dx) / d
    const my = x.c[1] + (a * dy) / d
    return [
      [mx + (h * dy) / d, my - (h * dx) / d],
      [mx - (h * dy) / d, my + (h * dx) / d],
    ]
  }
  return []
}

function within(c: Curve, p: Vec2): boolean {
  if (c.kind === 'seg') {
    const t = segParam(c.a, c.b, p)
    return t > -TOL && t < 1 + TOL
  }
  return onArc(c, p)
}

function isEnd(c: Curve, p: Vec2): boolean {
  const near = (q: Vec2) => Math.hypot(p[0] - q[0], p[1] - q[1]) < 1e-6
  if (c.kind === 'seg') return near(c.a) || near(c.b)
  if (c.sweep >= TAU - 1e-9) return false
  const at = (ang: number): Vec2 => [c.c[0] + c.r * Math.cos(ang), c.c[1] + c.r * Math.sin(ang)]
  return near(at(c.a0)) || near(at(c.a0 + c.sweep))
}

/** Where two curves CROSS (not where they meet end to end): shown on the
 *  sketch, snappable, and where Trim cuts. */
export function crossings(s: CSketchSummary): { at: Vec2; refs: [EntityRef, EntityRef] }[] {
  const cs = curvesOf(s)
  const out: { at: Vec2; refs: [EntityRef, EntityRef] }[] = []
  for (let i = 0; i < cs.length; i++) {
    for (let j = i + 1; j < cs.length; j++) {
      for (const p of carrierHits(cs[i], cs[j])) {
        if (!within(cs[i], p) || !within(cs[j], p)) continue
        if (isEnd(cs[i], p) && isEnd(cs[j], p)) continue
        out.push({ at: p, refs: [cs[i].ref, cs[j].ref] })
      }
    }
  }
  return out
}

/** Curves lying ON each other (collinear overlapping lines, coincident
 *  circles / arcs) -- a profile cannot be closed through them. */
export function overlaps(s: CSketchSummary): [EntityRef, EntityRef][] {
  const cs = curvesOf(s)
  const out: [EntityRef, EntityRef][] = []
  for (let i = 0; i < cs.length; i++) {
    for (let j = i + 1; j < cs.length; j++) {
      const x = cs[i]
      const y = cs[j]
      if (x.kind === 'seg' && y.kind === 'seg') {
        const d: Vec2 = [x.b[0] - x.a[0], x.b[1] - x.a[1]]
        const L = Math.hypot(d[0], d[1])
        const off = (p: Vec2) => Math.abs((p[0] - x.a[0]) * d[1] - (p[1] - x.a[1]) * d[0]) / L
        if (off(y.a) > 1e-6 || off(y.b) > 1e-6) continue
        const t0 = segParam(x.a, x.b, y.a)
        const t1 = segParam(x.a, x.b, y.b)
        const lo = Math.max(0, Math.min(t0, t1))
        const hi = Math.min(1, Math.max(t0, t1))
        if ((hi - lo) * L > 1e-6) out.push([x.ref, y.ref])
      } else if (x.kind === 'circ' && y.kind === 'circ') {
        if (Math.hypot(x.c[0] - y.c[0], x.c[1] - y.c[1]) < 1e-6 && Math.abs(x.r - y.r) < 1e-6) out.push([x.ref, y.ref])
      }
    }
  }
  return out
}

/** The curve a line end runs into when extended forward, or null. */
export function extendBoundary(lineId: string, end: 'start' | 'end', s: CSketchSummary): EntityRef | null {
  const g = segOf(s, lineId)
  if (!g) return null
  const [from, tip] = end === 'end' ? [g[0], g[1]] : [g[1], g[0]]
  const ray: Curve = { ref: { Line: lineId }, kind: 'seg', a: from, b: tip }
  let best: EntityRef | null = null
  let bestT = Infinity
  for (const c of curvesOf(s)) {
    if (kindOf(c.ref) === 'Line' && idOf(c.ref) === lineId) continue
    for (const p of carrierHits(ray, c)) {
      const t = segParam(from, tip, p)
      if (t > 1 + 1e-9 && within(c, p) && t < bestT) {
        bestT = t
        best = c.ref
      }
    }
  }
  return best
}

/** A sketch corner: a point where exactly two lines meet (fillet / chamfer). */
export function isCorner(pointId: string, s: CSketchSummary): boolean {
  return s.lines.filter((l) => l.endpoints && (l.endpoints[0] === pointId || l.endpoints[1] === pointId)).length === 2
}

/** Which end of a line a click is nearer. */
export function nearerEnd(lineId: string, pick: Vec2, s: CSketchSummary): 'start' | 'end' | null {
  const g = segOf(s, lineId)
  if (!g) return null
  const d0 = Math.hypot(pick[0] - g[0][0], pick[1] - g[0][1])
  const d1 = Math.hypot(pick[0] - g[1][0], pick[1] - g[1][1])
  return d0 <= d1 ? 'start' : 'end'
}

/** Where a constraint's glyph or a dimension's value sits: the middle of
 *  its first curve, else its first point. */
export function anchorOf(entities: EntityRef[], s: CSketchSummary): Vec2 | null {
  for (const r of entities) {
    const id = idOf(r)
    switch (kindOf(r)) {
      case 'Line': {
        const g = segOf(s, id)
        if (g) return [(g[0][0] + g[1][0]) / 2, (g[0][1] + g[1][1]) / 2]
        break
      }
      case 'Circle': {
        const c = s.circles.find((x) => x.id === id)
        if (c) return [c.cx + c.radius * Math.SQRT1_2, c.cy + c.radius * Math.SQRT1_2]
        break
      }
      case 'Arc': {
        const a = (s.arcs ?? []).find((x) => x.id === id)
        if (a) {
          const mid = a.start_angle + norm(a.end_angle - a.start_angle) / 2
          return [a.cx + a.radius * Math.cos(mid), a.cy + a.radius * Math.sin(mid)]
        }
        break
      }
      default:
        break
    }
  }
  const p = entities.find((r) => kindOf(r) === 'Point')
  const pt = p && s.points.find((x) => x.id === idOf(p))
  return pt ? [pt.x, pt.y] : null
}

/** A constraint as a glyph on the sketch (how established sketchers mark them). */
export const CONSTRAINT_GLYPH: Partial<Record<string, string>> = {
  Horizontal: 'H',
  Vertical: 'V',
  Parallel: '∥',
  Perpendicular: '⊥',
  Equal: '=',
  Coincident: '•',
  Tangent: 'T',
  Concentric: '◎',
  PointOnCurve: '⌖',
  Midpoint: 'M',
  Symmetric: '⇋',
  Collinear: '≡',
}

/** Whether a constraint is marked on the sketch: not when every curve it
 *  touches is construction (pattern / mirror guides would bury the
 *  sketch in marks nobody placed). */
export function showsMark(entities: EntityRef[], s: CSketchSummary): boolean {
  const curves = entities.filter((r) => kindOf(r) !== 'Point')
  if (curves.length === 0) return true
  const construction = (r: EntityRef): boolean => {
    const id = idOf(r)
    switch (kindOf(r)) {
      case 'Line':
        return s.lines.find((x) => x.id === id)?.is_construction ?? false
      case 'Circle':
        return s.circles.find((x) => x.id === id)?.is_construction ?? false
      case 'Arc':
        return (s.arcs ?? []).find((x) => x.id === id)?.is_construction ?? false
      default:
        return false
    }
  }
  return !curves.every(construction)
}

/** Each spline's control polygon (dashed on the sketch): its control
 *  points are sketch points -- drag one and the curve reshapes. */
export function controlPolygons(s: CSketchSummary): Vec2[][] {
  return (s.splines ?? []).map((sp) => sp.control_points.map((p) => [p[0], p[1]] as Vec2))
}
