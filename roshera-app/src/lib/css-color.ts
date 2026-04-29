import * as THREE from 'three'

/**
 * Bridge from CSS custom properties (oklch / rgb / hex) to THREE.Color +
 * alpha. The browser's Canvas2D `fillStyle` setter is the canonical CSS
 * color parser — assigning any valid color and reading it back yields a
 * normalised `#rrggbb` or `rgba(r, g, b, a)` string regardless of input
 * syntax (named colors, oklch, hsl, color() functions, etc.).
 *
 * This lets the Three.js scene consume the same blueprint tokens as the
 * rest of the UI without duplicating palette literals.
 */

export interface ResolvedColor {
  color: THREE.Color
  alpha: number
}

const FALLBACK: ResolvedColor = {
  color: new THREE.Color(0x888888),
  alpha: 1,
}

let canvasProbe: CanvasRenderingContext2D | null = null

function getProbe(): CanvasRenderingContext2D | null {
  if (canvasProbe) return canvasProbe
  if (typeof document === 'undefined') return null
  const canvas = document.createElement('canvas')
  const ctx = canvas.getContext('2d')
  if (!ctx) return null
  canvasProbe = ctx
  return ctx
}

/**
 * Resolves a CSS variable on `:root` (e.g. `--cad-grid`) to a THREE.Color
 * plus its alpha component. Returns a neutral grey at full opacity if the
 * variable is unset or unparseable so the scene never renders black.
 */
export function resolveCssVar(varName: string): ResolvedColor {
  if (typeof document === 'undefined') return FALLBACK
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue(varName)
    .trim()
  if (!raw) return FALLBACK
  return parseCssColor(raw)
}

/**
 * Parses any CSS color string (hex, rgb/rgba, hsl, oklch, named) using a
 * Canvas2D context as the canonical normaliser.
 */
export function parseCssColor(value: string): ResolvedColor {
  const ctx = getProbe()
  if (!ctx) return FALLBACK

  // Reset to a known sentinel so a rejected fillStyle assignment leaves a
  // detectable previous value.
  ctx.fillStyle = '#000000'
  try {
    ctx.fillStyle = value
  } catch {
    return FALLBACK
  }
  const normalised = ctx.fillStyle as string

  // Hex form — no alpha channel.
  if (normalised.startsWith('#')) {
    return { color: new THREE.Color(normalised), alpha: 1 }
  }

  // rgb() / rgba() form.
  const match = normalised.match(/rgba?\(([^)]+)\)/i)
  if (!match) return FALLBACK
  const parts = match[1].split(',').map((s) => parseFloat(s.trim()))
  if (parts.length < 3 || parts.some((p) => Number.isNaN(p))) return FALLBACK
  const [r, g, b, a] = parts
  return {
    color: new THREE.Color(r / 255, g / 255, b / 255),
    alpha: a === undefined ? 1 : a,
  }
}
