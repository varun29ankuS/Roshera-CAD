/**
 * The sign-in hero's aircraft: a stealth fighter concept drawn in the
 * browser (Varun 2026-11-18, after nTop's hero: an ink drawing with a
 * particle wake). This is the pure part -- the numbers and the wake seeding
 * -- so it can be checked like geometry; `SignInHero` draws it.
 *
 * Aircraft frame, metres: x from the nose (0) to the tail, y up, z span
 * (right wing +z). Flying surfaces use NACA 4-digit symmetric sections.
 */

/** NACA 4-digit half-thickness (closed trailing edge, -0.1036) at chord
 *  fraction `x` for thickness ratio `t`, as a fraction of chord. */
export function nacaThickness(t: number, x: number): number {
  return 5 * t * (0.2969 * Math.sqrt(x) - 0.126 * x - 0.3516 * x * x + 0.2843 * x ** 3 - 0.1036 * x ** 4)
}

/** A closed symmetric section loop, unit chord, cosine-spaced: [x, y][]. */
export function nacaSection(t: number, n = 20): [number, number][] {
  const up: [number, number][] = []
  const lo: [number, number][] = []
  for (let i = 0; i <= n; i++) {
    const x = 0.5 * (1 - Math.cos((Math.PI * i) / n))
    const y = nacaThickness(t, x)
    up.push([x, y])
    lo.push([x, -y])
  }
  return up.concat(lo.reverse().slice(1, -1))
}

export const LENGTH = 17.6

/** The wing: root at the body side, a crank where the trailing edge turns
 *  forward, and the tip. Leading edge swept 42 degrees throughout. */
export const WING = {
  sweepDeg: 42,
  rootZ: 1.85,
  crankZ: 3.7,
  tipZ: 5.85,
  rootLeX: 7.0,
  rootTeX: 14.6,
  crankTeX: 14.25,
  tipChord: 1.25,
  anhedralDeg: 3,
  thickness: { root: 0.05, crank: 0.045, tip: 0.035 },
} as const

export function wingLeadingEdgeX(z: number): number {
  return WING.rootLeX + (z - WING.rootZ) * Math.tan((WING.sweepDeg * Math.PI) / 180)
}

/** The cranked trailing edge: straight to the crank, then swept forward. */
export function wingTrailingEdgeX(z: number): number {
  const tipTe = wingLeadingEdgeX(WING.tipZ) + WING.tipChord
  if (z <= WING.crankZ) {
    const u = (z - WING.rootZ) / (WING.crankZ - WING.rootZ)
    return WING.rootTeX + (WING.crankTeX - WING.rootTeX) * u
  }
  const u = (z - WING.crankZ) / (WING.tipZ - WING.crankZ)
  return WING.crankTeX + (tipTe - WING.crankTeX) * u
}

/** Twin nozzles: centre height, spanwise offset of each, petal geometry. */
export const NOZZLE = {
  y: -0.06,
  z: 0.72,
  innerRadius: 0.65,
  exitRadius: 0.57,
  startX: 17.55,
  seamX: 18.06,
  tipX: 18.2,
  petals: 14,
} as const

/** The fuselage's control tables: chine half-width, top, belly, chine height. */
export const FUSELAGE = {
  xs: [0, 1.4, 3.2, 5.0, 6.6, 8.0, 10.5, 13.5, 16.0, 17.6],
  halfWidth: [0.02, 0.36, 0.72, 1.05, 1.55, 1.95, 1.95, 1.75, 1.35, 1.15],
  top: [0.0, 0.2, 0.38, 0.5, 0.52, 0.5, 0.5, 0.46, 0.42, 0.36],
  belly: [0.0, -0.16, -0.32, -0.42, -0.5, -0.55, -0.55, -0.5, -0.44, -0.38],
  chine: [0.0, 0.0, 0.0, -0.02, -0.04, -0.05, -0.05, -0.04, -0.03, -0.02],
} as const

/** A smooth (smoothstep) interpolation through a control table. */
export function tableAt(xs: readonly number[], vs: readonly number[], x: number): number {
  if (x <= xs[0]) return vs[0]
  for (let i = 1; i < xs.length; i++) {
    if (x <= xs[i]) {
      const u = (x - xs[i - 1]) / (xs[i] - xs[i - 1])
      const s = u * u * (3 - 2 * u)
      return vs[i - 1] + (vs[i] - vs[i - 1]) * s
    }
  }
  return vs[vs.length - 1]
}

export const WAKE_KIND = { wing: 0, nozzle: 1, tip: 2, tail: 3 } as const

/**
 * Where each wake particle starts and its seed: `origin` [x,y,z] per
 * particle, `seed` [filament hash, phase, kind, side] per particle. Wing
 * particles start on the trailing edge in 70 filaments a side, nozzle ones
 * inside the exits (the dense core), tip ones at the wingtips (vortices),
 * the rest along the tailplanes. `rand` returns [0, 1).
 */
export function wakeSeeds(count: number, rand: () => number): { origin: Float32Array; seed: Float32Array; count: number } {
  const origin = new Float32Array(count * 3)
  const seed = new Float32Array(count * 4)
  const hash = (n: number) => {
    const s = Math.sin(n * 127.1 + 311.7) * 43758.5453
    return s - Math.floor(s)
  }
  for (let i = 0; i < count; i++) {
    const r = rand()
    const side = rand() < 0.5 ? -1 : 1
    let kind: number
    let lane: number
    let ox: number
    let oy: number
    let oz: number
    if (r < 0.52) {
      kind = WAKE_KIND.wing
      lane = Math.floor(rand() * 70)
      const u = Math.min(1, Math.max(0, (lane + 0.5 + (rand() - 0.5) * 0.3) / 70))
      const z = WING.rootZ + (WING.tipZ - WING.rootZ) * u
      ox = wingTrailingEdgeX(z)
      oy = -0.05 - (z - WING.rootZ) * Math.tan((WING.anhedralDeg * Math.PI) / 180)
      oz = side * z
    } else if (r < 0.8) {
      kind = WAKE_KIND.nozzle
      lane = Math.floor(rand() * 18)
      const a = rand() * Math.PI * 2
      const rr = NOZZLE.exitRadius * 0.88 * Math.sqrt(rand())
      ox = NOZZLE.tipX
      oy = NOZZLE.y + rr * Math.sin(a)
      oz = side * NOZZLE.z + rr * Math.cos(a)
    } else if (r < 0.9) {
      kind = WAKE_KIND.tip
      lane = Math.floor(rand() * 12)
      ox = wingTrailingEdgeX(WING.tipZ)
      oy = -0.05 - (WING.tipZ - WING.rootZ) * Math.tan((WING.anhedralDeg * Math.PI) / 180)
      oz = side * WING.tipZ
    } else {
      kind = WAKE_KIND.tail
      lane = Math.floor(rand() * 30)
      const u = (lane + 0.5) / 30
      ox = 17.5
      oy = -0.06
      oz = side * (1.1 + 2.9 * u)
    }
    origin[i * 3] = ox
    origin[i * 3 + 1] = oy
    origin[i * 3 + 2] = oz
    seed[i * 4] = hash(lane * 7.31 + kind * 101.7 + (side > 0 ? 13.1 : 0))
    seed[i * 4 + 1] = rand()
    seed[i * 4 + 2] = kind
    seed[i * 4 + 3] = side
  }
  return { origin, seed, count }
}
