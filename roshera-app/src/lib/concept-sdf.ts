/**
 * CONCEPT SDF -- the field a concept recipe defines, its bounds, and the
 * volume / mass ESTIMATE the concept card prints.
 * =====================================================================
 * One description, two evaluators: `prepareRecipe` turns a validated recipe
 * (concept-recipe.ts) into world-frame primitives once; this module evaluates
 * them in TypeScript (numbers, points of view, tests) and concept-glsl.ts
 * emits the SAME primitives and the SAME formulas into the fragment shader
 * that draws the sketch. Change a formula here and change it there.
 *
 * Distances are in mm, negative inside material. The primitives are the
 * exact Euclidean fields of their shapes (Inigo Quilez's closed forms for
 * the box, capped cylinder, capped cone and triangle); union / subtract /
 * intersect are min / max(a, -b) / max, and a union blend is the circular
 * "round union" (Mercury hg_sdf fOpUnionRound): exactly a radius-r fillet
 * where two planes meet at 90 degrees, a close sketch of one elsewhere -- it
 * is NOT the kernel's rolling-ball fillet, which is why a concept's volume
 * with a blend is an estimate twice over.
 *
 * The volume is estimated by sampling the field at cell centres over the
 * recipe's bounds and counting each cell as filled by clamp(0.5 - d/h, 0, 1)
 * (h = cell edge): the linear partial-volume rule, exact for a face parallel
 * to the cells and within a fraction of a percent for the shapes a concept
 * uses (pinned in test/concept-sdf.test.mjs). It is labelled an estimate
 * everywhere it is shown; only a built part's mass properties are measured.
 */

import type { Recipe, RecipeShape } from './concept-recipe.ts'

export type Vec3 = [number, number, number]

export interface Bounds {
  min: Vec3
  max: Vec3
}

// ── vector helpers ────────────────────────────────────────────────────

export const add = (a: Vec3, b: Vec3): Vec3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
export const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
export const scale = (a: Vec3, k: number): Vec3 => [a[0] * k, a[1] * k, a[2] * k]
export const dot = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
export const cross = (a: Vec3, b: Vec3): Vec3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
]
export const length = (a: Vec3): number => Math.sqrt(dot(a, a))
export const normalize = (a: Vec3): Vec3 => scale(a, 1 / length(a))
const clamp = (x: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, x))

// ── prepared primitives (world frame, shared with concept-glsl.ts) ────

export type Prepared =
  /** Box / plate: middle `c`, orthonormal frame u v n, half sizes h; a plate
   *  rounds the four edges parallel to n with radius r. */
  | { kind: 'box'; c: Vec3; u: Vec3; v: Vec3; n: Vec3; h: Vec3 }
  | { kind: 'plate'; c: Vec3; u: Vec3; v: Vec3; n: Vec3; h: Vec3; r: number }
  /** Capped cylinder: base centre a, unit axis, radius, length. */
  | { kind: 'cylinder'; a: Vec3; axis: Vec3; r: number; len: number }
  /** Capped cone: base centre a (radius ra) to top centre b (radius rb). */
  | { kind: 'cone'; a: Vec3; b: Vec3; ra: number; rb: number }
  /** Triangle prism: vertices (0,0), (p1x,0), p2 in the (u,v) frame at o,
   *  extruded +-ht along n. */
  | { kind: 'rib'; o: Vec3; u: Vec3; v: Vec3; n: Vec3; p1x: number; p2: [number, number]; ht: number }
  /** Stadium prism: start-face centre c, long axis u, across v, depth along
   *  n from 0 to `depth`; straight half-length hl, end radius r. */
  | { kind: 'slot'; c: Vec3; u: Vec3; v: Vec3; n: Vec3; hl: number; r: number; depth: number }

export interface PreparedStep {
  op: 'union' | 'subtract' | 'intersect'
  blend: number
  prim: Prepared
}

export function preparePrimitive(s: RecipeShape): Prepared {
  switch (s.kind) {
    case 'box':
    case 'plate': {
      const u = normalize(s.u_axis)
      const v = normalize(s.v_axis)
      const n = normalize(cross(u, v))
      const c = add(s.center, scale(n, s.height / 2))
      const h: Vec3 = [s.width / 2, s.depth / 2, s.height / 2]
      return s.kind === 'box' ? { kind: 'box', c, u, v, n, h } : { kind: 'plate', c, u, v, n, h, r: s.corner_radius }
    }
    case 'cylinder':
      return { kind: 'cylinder', a: s.center, axis: normalize(s.axis), r: s.radius, len: s.height }
    case 'cone': {
      const axis = normalize(s.axis)
      return { kind: 'cone', a: s.center, b: add(s.center, scale(axis, s.height)), ra: s.base_radius, rb: s.top_radius }
    }
    case 'rib': {
      const u = normalize(s.leg_a)
      const v = normalize(sub(s.leg_b, scale(u, dot(s.leg_b, u))))
      const n = cross(u, v)
      return {
        kind: 'rib',
        o: s.origin,
        u,
        v,
        n,
        p1x: length(s.leg_a),
        p2: [dot(s.leg_b, u), dot(s.leg_b, v)],
        ht: s.thickness / 2,
      }
    }
    case 'slot': {
      const u = normalize(s.dir)
      const n = normalize(s.normal)
      const v = cross(n, u)
      return { kind: 'slot', c: s.center, u, v, n, hl: (s.length - s.width) / 2, r: s.width / 2, depth: s.depth }
    }
  }
}

export function prepareRecipe(recipe: Recipe): PreparedStep[] {
  return recipe.steps.map((s) => ({ op: s.op, blend: s.blend ?? 0, prim: preparePrimitive(s.shape) }))
}

// ── primitive fields (mirror GLSL_SDF_LIBRARY in concept-glsl.ts) ─────

/** Exact field of a rectangle-box extrusion: 2D distance d2, half height hz. */
function extrude(d2: number, z: number, hz: number): number {
  const wx = d2
  const wy = Math.abs(z) - hz
  return Math.min(Math.max(wx, wy), 0) + Math.hypot(Math.max(wx, 0), Math.max(wy, 0))
}

/** 2D triangle field (Quilez sdTriangle). */
function sdTriangle2(px: number, py: number, p1x: number, p2: [number, number]): number {
  const p0: [number, number] = [0, 0]
  const p1: [number, number] = [p1x, 0]
  const e0 = [p1[0] - p0[0], p1[1] - p0[1]]
  const e1 = [p2[0] - p1[0], p2[1] - p1[1]]
  const e2 = [p0[0] - p2[0], p0[1] - p2[1]]
  const v0 = [px - p0[0], py - p0[1]]
  const v1 = [px - p1[0], py - p1[1]]
  const v2 = [px - p2[0], py - p2[1]]
  const proj = (v: number[], e: number[]) => {
    const t = clamp((v[0] * e[0] + v[1] * e[1]) / (e[0] * e[0] + e[1] * e[1]), 0, 1)
    return [v[0] - e[0] * t, v[1] - e[1] * t]
  }
  const pq0 = proj(v0, e0)
  const pq1 = proj(v1, e1)
  const pq2 = proj(v2, e2)
  const s = Math.sign(e0[0] * e2[1] - e0[1] * e2[0])
  const d0 = [pq0[0] * pq0[0] + pq0[1] * pq0[1], s * (v0[0] * e0[1] - v0[1] * e0[0])]
  const d1 = [pq1[0] * pq1[0] + pq1[1] * pq1[1], s * (v1[0] * e1[1] - v1[1] * e1[0])]
  const d2 = [pq2[0] * pq2[0] + pq2[1] * pq2[1], s * (v2[0] * e2[1] - v2[1] * e2[0])]
  const dx = Math.min(d0[0], d1[0], d2[0])
  const dy = Math.min(d0[1], d1[1], d2[1])
  return -Math.sqrt(dx) * Math.sign(dy)
}

export function primitiveSdf(q: Prepared, p: Vec3): number {
  switch (q.kind) {
    case 'box': {
      const d = sub(p, q.c)
      const x = Math.abs(dot(d, q.u)) - q.h[0]
      const y = Math.abs(dot(d, q.v)) - q.h[1]
      const z = Math.abs(dot(d, q.n)) - q.h[2]
      return Math.hypot(Math.max(x, 0), Math.max(y, 0), Math.max(z, 0)) + Math.min(Math.max(x, y, z), 0)
    }
    case 'plate': {
      const d = sub(p, q.c)
      const x = Math.abs(dot(d, q.u)) - (q.h[0] - q.r)
      const y = Math.abs(dot(d, q.v)) - (q.h[1] - q.r)
      const d2 = Math.hypot(Math.max(x, 0), Math.max(y, 0)) + Math.min(Math.max(x, y), 0) - q.r
      return extrude(d2, dot(d, q.n), q.h[2])
    }
    case 'cylinder': {
      const d = sub(p, q.a)
      const t = dot(d, q.axis)
      const radial = length(sub(d, scale(q.axis, t)))
      const dx = radial - q.r
      const dy = Math.abs(t - q.len / 2) - q.len / 2
      return Math.min(Math.max(dx, dy), 0) + Math.hypot(Math.max(dx, 0), Math.max(dy, 0))
    }
    case 'cone': {
      // Quilez sdCappedCone(p, a, b, ra, rb).
      const ba = sub(q.b, q.a)
      const pa = sub(p, q.a)
      const rba = q.rb - q.ra
      const baba = dot(ba, ba)
      const papa = dot(pa, pa)
      const paba = dot(pa, ba) / baba
      const x = Math.sqrt(Math.max(0, papa - paba * paba * baba))
      const cax = Math.max(0, x - (paba < 0.5 ? q.ra : q.rb))
      const cay = Math.abs(paba - 0.5) - 0.5
      const k = rba * rba + baba
      const f = clamp((rba * (x - q.ra) + paba * baba) / k, 0, 1)
      const cbx = x - q.ra - f * rba
      const cby = paba - f
      const s = cbx < 0 && cay < 0 ? -1 : 1
      return s * Math.sqrt(Math.min(cax * cax + cay * cay * baba, cbx * cbx + cby * cby * baba))
    }
    case 'rib': {
      const d = sub(p, q.o)
      const d2 = sdTriangle2(dot(d, q.u), dot(d, q.v), q.p1x, q.p2)
      return extrude(d2, dot(d, q.n), q.ht)
    }
    case 'slot': {
      const d = sub(p, q.c)
      const x = Math.max(Math.abs(dot(d, q.u)) - q.hl, 0)
      const y = dot(d, q.v)
      const d2 = Math.hypot(x, y) - q.r
      return extrude(d2, dot(d, q.n) - q.depth / 2, q.depth / 2)
    }
  }
}

/** Round union (hg_sdf fOpUnionRound). */
export function unionRound(a: number, b: number, r: number): number {
  return Math.max(r, Math.min(a, b)) - Math.hypot(Math.max(r - a, 0), Math.max(r - b, 0))
}

export function combine(op: PreparedStep['op'], d: number, s: number, blend: number): number {
  switch (op) {
    case 'union':
      return blend > 0 ? unionRound(d, s, blend) : Math.min(d, s)
    case 'subtract':
      return Math.max(d, -s)
    case 'intersect':
      return Math.max(d, s)
  }
}

export type Sdf = (p: Vec3) => number

export function fieldOf(steps: readonly PreparedStep[]): Sdf {
  return (p) => {
    let d = primitiveSdf(steps[0].prim, p)
    for (let i = 1; i < steps.length; i++) d = combine(steps[i].op, d, primitiveSdf(steps[i].prim, p), steps[i].blend)
    return d
  }
}

export function compileSdf(recipe: Recipe): Sdf {
  return fieldOf(prepareRecipe(recipe))
}

/** Outward unit normal by central differences. */
export function normalAt(f: Sdf, p: Vec3, eps: number): Vec3 {
  const g: Vec3 = [
    f([p[0] + eps, p[1], p[2]]) - f([p[0] - eps, p[1], p[2]]),
    f([p[0], p[1] + eps, p[2]]) - f([p[0], p[1] - eps, p[2]]),
    f([p[0], p[1], p[2] + eps]) - f([p[0], p[1], p[2] - eps]),
  ]
  const l = length(g)
  return l > 0 ? scale(g, 1 / l) : [0, 0, 1]
}

// ── bounds ────────────────────────────────────────────────────────────

function boundsOfPoints(points: Vec3[], pad: Vec3 = [0, 0, 0]): Bounds {
  const min: Vec3 = [Infinity, Infinity, Infinity]
  const max: Vec3 = [-Infinity, -Infinity, -Infinity]
  for (const p of points)
    for (let i = 0; i < 3; i++) {
      min[i] = Math.min(min[i], p[i] - pad[i])
      max[i] = Math.max(max[i], p[i] + pad[i])
    }
  return { min, max }
}

/** The per-axis half extent of a disc of radius r normal to unit `axis`. */
const discPad = (axis: Vec3, r: number): Vec3 =>
  [0, 1, 2].map((i) => r * Math.sqrt(Math.max(0, 1 - axis[i] * axis[i]))) as Vec3

function unionBounds(a: Bounds, b: Bounds): Bounds {
  return {
    min: [0, 1, 2].map((i) => Math.min(a.min[i], b.min[i])) as Vec3,
    max: [0, 1, 2].map((i) => Math.max(a.max[i], b.max[i])) as Vec3,
  }
}

export function primitiveBounds(q: Prepared): Bounds {
  switch (q.kind) {
    case 'box':
    case 'plate': {
      const corners: Vec3[] = []
      for (const su of [-1, 1])
        for (const sv of [-1, 1])
          for (const sn of [-1, 1])
            corners.push(add(q.c, add(scale(q.u, su * q.h[0]), add(scale(q.v, sv * q.h[1]), scale(q.n, sn * q.h[2])))))
      return boundsOfPoints(corners)
    }
    case 'cylinder':
      return boundsOfPoints([q.a, add(q.a, scale(q.axis, q.len))], discPad(q.axis, q.r))
    case 'cone': {
      const axis = normalize(sub(q.b, q.a))
      return unionBounds(boundsOfPoints([q.a], discPad(axis, q.ra)), boundsOfPoints([q.b], discPad(axis, q.rb)))
    }
    case 'rib': {
      const tri: Vec3[] = [
        q.o,
        add(q.o, scale(q.u, q.p1x)),
        add(q.o, add(scale(q.u, q.p2[0]), scale(q.v, q.p2[1]))),
      ]
      return boundsOfPoints(tri.flatMap((t) => [add(t, scale(q.n, q.ht)), add(t, scale(q.n, -q.ht))]))
    }
    case 'slot': {
      const ends = [add(q.c, scale(q.u, q.hl)), add(q.c, scale(q.u, -q.hl))]
      return boundsOfPoints(
        ends.flatMap((e) => [e, add(e, scale(q.n, q.depth))]),
        discPad(q.n, q.r),
      )
    }
  }
}

export function boundsOfSteps(steps: readonly PreparedStep[]): Bounds {
  let b = primitiveBounds(steps[0].prim)
  for (let i = 1; i < steps.length; i++) {
    const s = steps[i]
    const pb = primitiveBounds(s.prim)
    if (s.op === 'union') {
      b = unionBounds(b, pb)
      if (s.blend > 0) b = { min: b.min.map((x) => x - s.blend) as Vec3, max: b.max.map((x) => x + s.blend) as Vec3 }
    } else if (s.op === 'intersect') {
      b = {
        min: [0, 1, 2].map((k) => Math.max(b.min[k], pb.min[k])) as Vec3,
        max: [0, 1, 2].map((k) => Math.min(b.max[k], pb.max[k])) as Vec3,
      }
    }
  }
  return b
}

export function recipeBounds(recipe: Recipe): Bounds {
  return boundsOfSteps(prepareRecipe(recipe))
}

// ── sampling grid + volume ────────────────────────────────────────────

/** Cells along the bounds' longest side. */
export const VOLUME_CELLS = 64

export interface Grid {
  origin: Vec3
  h: number
  n: [number, number, number]
}

/** A cell grid over `b`, padded by two cells, `cells` along the longest side. */
export function gridOver(b: Bounds, cells: number): Grid {
  const size = [0, 1, 2].map((i) => Math.max(0, b.max[i] - b.min[i]))
  const longest = Math.max(...size, 1e-9)
  const h = longest / cells
  const pad = 2 * h
  const origin = b.min.map((x) => x - pad) as Vec3
  const n = size.map((s) => Math.max(1, Math.ceil((s + 2 * pad) / h))) as [number, number, number]
  return { origin, h, n }
}

export interface VolumeEstimate {
  volume_mm3: number
  /** The grid's cell edge, mm -- the estimate's resolution. */
  cell_mm: number
}

export function estimateVolumeOfField(f: Sdf, b: Bounds, cells = VOLUME_CELLS): VolumeEstimate {
  if (!(b.max[0] > b.min[0] && b.max[1] > b.min[1] && b.max[2] > b.min[2])) return { volume_mm3: 0, cell_mm: 0 }
  const g = gridOver(b, cells)
  const { h } = g
  let filled = 0
  const p: Vec3 = [0, 0, 0]
  for (let i = 0; i < g.n[0]; i++) {
    p[0] = g.origin[0] + (i + 0.5) * h
    for (let j = 0; j < g.n[1]; j++) {
      p[1] = g.origin[1] + (j + 0.5) * h
      for (let k = 0; k < g.n[2]; k++) {
        p[2] = g.origin[2] + (k + 0.5) * h
        filled += clamp(0.5 - f(p) / h, 0, 1)
      }
    }
  }
  return { volume_mm3: filled * h * h * h, cell_mm: h }
}

export function estimateVolume(recipe: Recipe, cells = VOLUME_CELLS): VolumeEstimate {
  const steps = prepareRecipe(recipe)
  return estimateVolumeOfField(fieldOf(steps), boundsOfSteps(steps), cells)
}

/** grams = mm^3 x (kg/m^3) x 1e-9 m^3/mm^3 x 1e3 g/kg. */
export function massGrams(volumeMm3: number, densityKgM3: number): number {
  return volumeMm3 * densityKgM3 * 1e-6
}
