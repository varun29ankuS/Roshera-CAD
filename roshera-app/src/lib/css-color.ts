import * as THREE from 'three'

/**
 * Bridge from CSS custom properties (oklch / rgb / hex / named) to
 * THREE.Color + alpha.
 *
 * Implementation note: we set the value as the `color` property on a
 * detached element and read back the computed style. Browsers normalise
 * `color` to `rgb(r, g, b)` or `rgba(r, g, b, a)` regardless of input
 * syntax — including modern color spaces like `oklch()` — so this works
 * across Chrome / Safari / Firefox without per-syntax parsing.
 */

export interface ResolvedColor {
  color: THREE.Color
  alpha: number
}

const FALLBACK: ResolvedColor = {
  color: new THREE.Color(0x888888),
  alpha: 1,
}

let probeEl: HTMLDivElement | null = null

function getProbe(): HTMLDivElement | null {
  if (probeEl) return probeEl
  if (typeof document === 'undefined') return null
  const el = document.createElement('div')
  el.style.position = 'absolute'
  el.style.left = '-9999px'
  el.style.top = '-9999px'
  el.style.visibility = 'hidden'
  el.style.pointerEvents = 'none'
  document.body.appendChild(el)
  probeEl = el
  return el
}

/**
 * Resolves a CSS variable on `:root` (e.g. `--cad-grid`) to a THREE.Color
 * plus its alpha component. Falls back to neutral grey on any failure.
 */
export function resolveCssVar(varName: string): ResolvedColor {
  const probe = getProbe()
  if (!probe) return FALLBACK
  probe.style.color = ''
  probe.style.color = `var(${varName})`
  // If the var didn't resolve, color stays as the inherited default
  // (typically a foreground value); we test for an unset var explicitly.
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(varName)
    .trim()
  if (!raw) return FALLBACK

  const computed = getComputedStyle(probe).color
  return parseRgbString(computed)
}

/**
 * Parses any CSS color string by routing it through the browser's
 * computed-style normaliser. Always returns rgb()/rgba() regardless of
 * input format (oklch, hsl, hex, named).
 */
export function parseCssColor(value: string): ResolvedColor {
  const probe = getProbe()
  if (!probe) return FALLBACK
  probe.style.color = ''
  probe.style.color = value
  const computed = getComputedStyle(probe).color
  if (!computed) return FALLBACK
  return parseRgbString(computed)
}

function parseRgbString(value: string): ResolvedColor {
  const match = value.match(/rgba?\(([^)]+)\)/i)
  if (!match) return FALLBACK
  const parts = match[1].split(/[,\s/]+/).filter(Boolean).map((s) => parseFloat(s))
  if (parts.length < 3 || parts.some((p) => Number.isNaN(p))) return FALLBACK
  const [r, g, b, a] = parts
  return {
    color: new THREE.Color(r / 255, g / 255, b / 255),
    alpha: a === undefined ? 1 : a,
  }
}
