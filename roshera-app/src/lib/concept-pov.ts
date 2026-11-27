/**
 * CONCEPT POINTS OF VIEW -- what each POV shows, as numbers.
 * ==========================================================
 * The concept card can show a sketch from a point of view (concept board
 * spec, 2026-11-27). Each one is a HEURISTIC and is labelled "estimate"
 * wherever it appears; none is a kernel verdict. The rules live here, with
 * tests (test/concept-pov.test.mjs); the fragment shader (concept-glsl.ts)
 * draws the same rules on the sketch.
 *
 * LOAD PATH -- material near the straight segment from each load point to
 * each support point carries the load; the rest is idle. A point's weight is
 * exp(-(d / sigma)^2) for its distance d to the nearest such segment, sigma
 * a fixed share of the part's size; weight >= 0.5 counts as "on the path".
 * Not a stress field: it ignores stiffness, section and direction.
 *
 * MAKER, 3-AXIS MILLING FROM +-Z -- a surface point is reachable from +Z
 * when a straight tool coming down the Z axis meets no material on its way
 * to that point (the field is marched from just off the surface along +Z),
 * and likewise from -Z. The downward face at the lowest Z is the STOCK face
 * (it sits in the vise, faced or saw-cut) and is not counted. One setup when
 * everything else is reachable from +Z; two when some of it needs -Z; no
 * count when some of it is reachable from neither (another setup or 5-axis).
 * A real tool has a radius and a length; this one is a line.
 *
 * MAKER, FDM -- a surface whose outward normal points more than 45 degrees
 * past vertical downward (n . -Z > cos 45) prints as an overhang; the face
 * on the bed (downward, at the lowest Z) does not.
 */

import type { ConceptLoad, ConceptSupport, Recipe } from './concept-recipe.ts'
import {
  boundsOfSteps,
  dot,
  fieldOf,
  gridOver,
  length,
  normalAt,
  prepareRecipe,
  scale,
  sub,
  add,
  type Bounds,
  type Sdf,
  type Vec3,
} from './concept-sdf.ts'

const clamp01 = (x: number) => Math.min(1, Math.max(0, x))

// ── load path ─────────────────────────────────────────────────────────

/** sigma = this share of the bounds' diagonal. */
export const LOAD_PATH_SIGMA_SHARE = 0.08
/** A point is on the load path at or above this weight. */
export const LOAD_PATH_ON = 0.5

export function loadPathSigma(b: Bounds): number {
  return LOAD_PATH_SIGMA_SHARE * length(sub(b.max, b.min))
}

function segmentDistance(p: Vec3, a: Vec3, b: Vec3): number {
  const ab = sub(b, a)
  const l2 = dot(ab, ab)
  const t = l2 > 0 ? clamp01(dot(sub(p, a), ab) / l2) : 0
  return length(sub(p, add(a, scale(ab, t))))
}

/** 1 on a load-to-support segment, falling as exp(-(d/sigma)^2). */
export function loadPathWeight(
  p: Vec3,
  loads: readonly Pick<ConceptLoad, 'at'>[],
  supports: readonly Pick<ConceptSupport, 'at'>[],
  sigma: number,
): number {
  let best = Infinity
  for (const l of loads) for (const s of supports) best = Math.min(best, segmentDistance(p, l.at, s.at))
  if (!Number.isFinite(best) || sigma <= 0) return 0
  return Math.exp(-(best / sigma) * (best / sigma))
}

export interface LoadPathStats {
  /** Share of the concept's material within the load path (estimate). */
  on_path_fraction: number
  sigma_mm: number
}

export function loadPathStats(
  recipe: Recipe,
  loads: readonly Pick<ConceptLoad, 'at'>[],
  supports: readonly Pick<ConceptSupport, 'at'>[],
  cells = 48,
): LoadPathStats {
  const steps = prepareRecipe(recipe)
  const f = fieldOf(steps)
  const b = boundsOfSteps(steps)
  const sigma = loadPathSigma(b)
  const g = gridOver(b, cells)
  let solid = 0
  let on = 0
  for (let i = 0; i < g.n[0]; i++)
    for (let j = 0; j < g.n[1]; j++)
      for (let k = 0; k < g.n[2]; k++) {
        const p: Vec3 = [g.origin[0] + (i + 0.5) * g.h, g.origin[1] + (j + 0.5) * g.h, g.origin[2] + (k + 0.5) * g.h]
        const occ = clamp01(0.5 - f(p) / g.h)
        if (occ === 0) continue
        solid += occ
        if (loadPathWeight(p, loads, supports, sigma) >= LOAD_PATH_ON) on += occ
      }
  return { on_path_fraction: solid > 0 ? on / solid : 0, sigma_mm: sigma }
}

// ── surface sampling ──────────────────────────────────────────────────

export interface SurfaceSample {
  p: Vec3
  /** Outward unit normal. */
  n: Vec3
  /** The surface area this sample stands for, mm^2. */
  area: number
}

export interface SurfaceSampling {
  samples: SurfaceSample[]
  cell_mm: number
  /** Lowest Z of the surface: where the stock face / the bed is. */
  zmin: number
  bounds: Bounds
  field: Sdf
}

/**
 * Cell centres just OUTSIDE the surface (0 <= d < w), projected onto it
 * along the field's gradient. Outside a solid the field is the exact
 * distance, so one step lands on the nearest surface point (inside, near an
 * edge, the gradient is ambiguous and a projection can stall). A band of
 * width w holds w / h^3 centres per unit area whatever the face's
 * orientation, so each sample stands for h^3 / w of surface; convex edges
 * are slightly over-counted (by about (pi/4) w per unit edge length).
 * Shares, not absolute areas, are what the card reports.
 */
export function surfaceSamples(recipe: Recipe, cells = 48): SurfaceSampling {
  const steps = prepareRecipe(recipe)
  const f = fieldOf(steps)
  const bounds = boundsOfSteps(steps)
  const g = gridOver(bounds, cells)
  const h = g.h
  // One cell wide: a face parallel to the cells then holds exactly one layer
  // of centres (a narrower band can miss such a face altogether).
  const band = h
  const area = (h * h * h) / band
  const eps = h * 0.05
  const samples: SurfaceSample[] = []
  let zmin = Infinity
  for (let i = 0; i < g.n[0]; i++)
    for (let j = 0; j < g.n[1]; j++)
      for (let k = 0; k < g.n[2]; k++) {
        let p: Vec3 = [g.origin[0] + (i + 0.5) * h, g.origin[1] + (j + 0.5) * h, g.origin[2] + (k + 0.5) * h]
        const d0 = f(p)
        if (d0 < 0 || d0 >= band) continue
        // Outside an exact field one step lands on the surface; a subtract
        // or a blend makes the field a bound, so step until it converges and
        // drop a sample that does not (it would sit on a phantom surface,
        // such as a cutting tool's cap in the air).
        for (let it = 0; it < 8; it++) {
          const d = f(p)
          if (Math.abs(d) < 1e-6 * h) break
          p = sub(p, scale(normalAt(f, p, eps), d))
        }
        if (Math.abs(f(p)) > 0.02 * h) continue
        const n = normalAt(f, p, eps)
        samples.push({ p, n, area })
        zmin = Math.min(zmin, p[2])
      }
  return { samples, cell_mm: h, zmin, bounds, field: f }
}

// ── maker: milling reach and FDM overhang ─────────────────────────────

/** Whether a straight tool along -dir*Z (coming from the +Z side when
 *  dir = +1) reaches surface point p with outward normal n: march the field
 *  from just off the surface along dir*Z to the bounds and see whether it
 *  meets material. */
/** How far off the surface a reach test starts: a thousandth of the part. */
export function reachOffset(b: Bounds): number {
  return Math.max(1e-3 * length(sub(b.max, b.min)), 1e-4)
}

/**
 * A sample in a concave corner: just off its own face, another face is
 * already nearer than the offset. A line tool can never settle such a point
 * (it touches the neighbouring face), and corner lines have no area, so the
 * maker shares leave these samples out rather than count sampling noise.
 */
export function isConcaveCorner(f: Sdf, b: Bounds, p: Vec3, n: Vec3): boolean {
  const off = reachOffset(b)
  return f(add(p, scale(n, off))) < 0.75 * off
}

export function reachableAlongZ(f: Sdf, b: Bounds, p: Vec3, n: Vec3, dir: 1 | -1): boolean {
  const off = reachOffset(b)
  const hit = off / 2
  const q: Vec3 = add(p, scale(n, off))
  const end = dir > 0 ? b.max[2] + off : b.min[2] - off
  let t = 0
  const span = Math.abs(end - q[2])
  for (let iter = 0; iter < 512 && t <= span; iter++) {
    const d = f([q[0], q[1], q[2] + dir * t])
    if (d < hit) return false
    t += Math.max(d, hit)
  }
  return true
}

/** FDM: more than 45 degrees past vertical, downward. */
export const OVERHANG_COS = Math.cos(Math.PI / 4)

/** Whether the point is on the stock face / the bed: facing down (an edge
 *  rounding onto it included) at the lowest Z. */
export function isBedFace(n: Vec3, z: number, zmin: number, band: number): boolean {
  return n[2] < 0 && z - zmin < band
}

export function isOverhang(n: Vec3, z: number, zmin: number, band: number): boolean {
  if (isBedFace(n, z, zmin, band)) return false
  return -n[2] > OVERHANG_COS + 1e-9
}

export interface MakerStats {
  mill: {
    /** Area shares (of the surface less the stock face). */
    top: number
    bottom_only: number
    neither: number
  }
  /** 1 or 2 straight-tool setups along Z; null when some surface is
   *  reachable from neither side. */
  setups: 1 | 2 | null
  fdm: {
    /** Area share printing as an overhang. */
    overhang: number
  }
  cell_mm: number
  /** Lowest Z of the surface (where the stock face / the bed is). */
  zmin: number
}

/** Shares below this are sampling noise, not a feature. */
export const MAKER_NOISE = 0.005

export function makerStats(recipe: Recipe, cells = 48): MakerStats {
  const s = surfaceSamples(recipe, cells)
  const bandZ = s.cell_mm * 0.5
  let total = 0
  let millArea = 0
  let top = 0
  let bottomOnly = 0
  let neither = 0
  let overhang = 0
  for (const q of s.samples) {
    total += q.area
    if (isOverhang(q.n, q.p[2], s.zmin, bandZ)) overhang += q.area
    if (isBedFace(q.n, q.p[2], s.zmin, bandZ)) continue
    if (isConcaveCorner(s.field, s.bounds, q.p, q.n)) continue
    millArea += q.area
    if (reachableAlongZ(s.field, s.bounds, q.p, q.n, 1)) top += q.area
    else if (reachableAlongZ(s.field, s.bounds, q.p, q.n, -1)) bottomOnly += q.area
    else neither += q.area
  }
  const share = (x: number, of: number) => (of > 0 ? x / of : 0)
  const mill = { top: share(top, millArea), bottom_only: share(bottomOnly, millArea), neither: share(neither, millArea) }
  const setups: MakerStats['setups'] = mill.neither > MAKER_NOISE ? null : mill.bottom_only > MAKER_NOISE ? 2 : 1
  return { mill, setups, fdm: { overhang: share(overhang, total) }, cell_mm: s.cell_mm, zmin: s.zmin }
}
