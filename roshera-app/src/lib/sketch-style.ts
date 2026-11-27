/**
 * THE SKETCH LOOK -- one source for every renderer that draws in it.
 * ==================================================================
 * The pencil style of the concept cards (Varun 2026-11-27: "the most
 * aesthetic sketches you have ever done for me"), shared by:
 *   - the concept cards' ray-marched SDF shader (concept-glsl.ts), and
 *   - the viewport's Sketch display mode on the kernel's real mesh
 *     (sketch-material.ts).
 * Both embed SKETCH_STYLE_GLSL and read their colours from `sketchColors`,
 * so the look cannot drift (test/sketch-style.test.mjs fails if either keeps
 * a private copy).
 *
 * The look:
 *   - PAPER: every lit surface is the paper tone -- a whisper (7%) of the
 *     ink on the page background. The part's own colour is not used.
 *   - THREE TONES from one light (from the viewer, up and to the left):
 *     light = paper; middle = paper + one diagonal hatch; dark = crossed
 *     hatch. Hatching is screen-space, one pixel in every five.
 *   - INK on creases and silhouettes (each renderer finds those its own
 *     way: the SDF from its field, the mesh from its normals and edges).
 *   - LIGHT AND DARK: ink is the theme's foreground, paper its background,
 *     caution its caution colour -- read from CSS on every draw.
 *   - THE PRINT: a new drawing appears top to bottom in four fixed steps,
 *     70 ms apart (mechanical, never eased); instantly under reduced motion.
 *
 * The TypeScript functions below mirror the GLSL ones line for line; the
 * GLSL is generated from the same constants.
 */

import { resolveCssVar } from './css-color.ts'

// ── constants (the GLSL is generated from these) ──────────────────────

/** lam above this: the light tone (paper, no hatch). */
export const SKETCH_TONE_LIGHT = 0.62
/** lam above this (and not light): the middle tone (one hatch). */
export const SKETCH_TONE_MID = 0.3
/** Ink strength of a hatch line. */
export const SKETCH_HATCH_INK = 0.55
/** One hatch pixel in every this many. */
export const SKETCH_HATCH_SPACING = 5
/** Crease ink fades in between these normal differences. */
export const SKETCH_CREASE_FROM = 0.03
export const SKETCH_CREASE_TO = 0.08
/** Paper = background mixed this far toward the ink. */
export const SKETCH_PAPER_INK = 0.07
/** The light, in the viewer's frame: -forward + UP * up - RIGHT * right. */
export const SKETCH_LIGHT_UP = 0.7
export const SKETCH_LIGHT_LEFT = 0.45

/** The stepped print. */
export const REVEAL_STEPS: readonly number[] = [0.25, 0.5, 0.75, 1]
export const REVEAL_STEP_MS = 70

// ── TypeScript mirrors of the GLSL ────────────────────────────────────

/** 0 light, 1 middle, 2 dark -- GLSL `sketchTone`. */
export function sketchTone(lam: number): 0 | 1 | 2 {
  return lam > SKETCH_TONE_LIGHT ? 0 : lam > SKETCH_TONE_MID ? 1 : 2
}

/** GLSL `sketchHatch` at pixel (x, y): step(mod(x*slope + y, spacing), 1). */
export function sketchHatch(x: number, y: number, spacing: number, slope: number): 0 | 1 {
  const c = x * slope + y
  const m = c - spacing * Math.floor(c / spacing)
  return m <= 1 ? 1 : 0
}

/** GLSL `sketchShadeInk`: the hatch ink for a surface lit at `lam`. */
export function sketchShadeInk(lam: number, x: number, y: number): number {
  const tone = sketchTone(lam)
  let ink = 0
  if (tone >= 1) ink = Math.max(ink, SKETCH_HATCH_INK * sketchHatch(x, y, SKETCH_HATCH_SPACING, 1))
  if (tone >= 2) ink = Math.max(ink, SKETCH_HATCH_INK * sketchHatch(x, y, SKETCH_HATCH_SPACING, -1))
  return ink
}

/** GLSL `sketchLightDir(right, up, fwd)` in the viewer's frame (x right, y up, looking down -z). */
export function sketchLightView(): [number, number, number] {
  const v: [number, number, number] = [-SKETCH_LIGHT_LEFT, SKETCH_LIGHT_UP, 1]
  const l = Math.hypot(...v)
  return [v[0] / l, v[1] / l, v[2] / l]
}

export function revealStart(reducedMotion: boolean): number {
  return reducedMotion ? 1 : 0
}

export function nextReveal(current: number): number {
  return REVEAL_STEPS.find((r) => r > current) ?? 1
}

// ── colours ───────────────────────────────────────────────────────────

export type Rgb = [number, number, number]

export interface SketchColors {
  /** sRGB 0..1. */
  ink: Rgb
  face: Rgb
  warn: Rgb
}

export function sketchPalette(fg: Rgb, bg: Rgb, warn: Rgb): SketchColors {
  const mix = (a: number, b: number) => a + (b - a) * SKETCH_PAPER_INK
  return { ink: fg, face: [mix(bg[0], fg[0]), mix(bg[1], fg[1]), mix(bg[2], fg[2])], warn }
}

/** The theme's palette, read from CSS now (call per draw: themes change). */
export function sketchColors(): SketchColors {
  const rgb = (v: string): Rgb => {
    const c = resolveCssVar(v).color
    return [c.r, c.g, c.b]
  }
  return sketchPalette(rgb('--foreground'), rgb('--background'), rgb('--caution'))
}

/** True when the OS asks for less motion. */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

// ── GLSL ──────────────────────────────────────────────────────────────

/** A GLSL ES float literal: always with a decimal point, never NaN. */
export function glslFloat(n: number): string {
  if (!Number.isFinite(n)) throw new Error(`not a finite number: ${n}`)
  if (Number.isInteger(n)) return `${n === 0 ? 0 : n}.0`
  let s = String(n)
  if (s.includes('e')) s = n.toFixed(12).replace(/0+$/, '')
  return s.includes('.') ? s : `${s}.0`
}

const f = glslFloat

/** The shared GLSL. Needs nothing but gl_FragCoord; every input is an argument. */
export const SKETCH_STYLE_GLSL = `
// ── SKETCH_STYLE (lib/sketch-style.ts) ──
float sketchHatch(float spacing, float slope) {
  float c = gl_FragCoord.x * slope + gl_FragCoord.y;
  return step(mod(c, spacing), 1.0);
}
float sketchTone(float lam) {
  return lam > ${f(SKETCH_TONE_LIGHT)} ? 0.0 : lam > ${f(SKETCH_TONE_MID)} ? 1.0 : 2.0;
}
float sketchShadeInk(float lam) {
  float tone = sketchTone(lam);
  float ink = 0.0;
  if (tone >= 1.0) ink = max(ink, ${f(SKETCH_HATCH_INK)} * sketchHatch(${f(SKETCH_HATCH_SPACING)}, 1.0));
  if (tone >= 2.0) ink = max(ink, ${f(SKETCH_HATCH_INK)} * sketchHatch(${f(SKETCH_HATCH_SPACING)}, -1.0));
  return ink;
}
float sketchCrease(float c) {
  return smoothstep(${f(SKETCH_CREASE_FROM)}, ${f(SKETCH_CREASE_TO)}, c);
}
vec3 sketchLightDir(vec3 right, vec3 up, vec3 fwd) {
  return normalize(-fwd + ${f(SKETCH_LIGHT_UP)} * up - ${f(SKETCH_LIGHT_LEFT)} * right);
}
bool sketchUnrevealed(float reveal, float height) {
  return 1.0 - gl_FragCoord.y / height > reveal;
}
`
