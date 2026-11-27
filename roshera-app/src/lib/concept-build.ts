/**
 * The words a concept card puts on the board and sends to the agent.
 *
 * "Build this one" never makes geometry in the app: it sends this request as
 * the user's next turn, and the agent builds the concept through the kernel
 * (the `concepts` playbook maps every recipe step to a kernel verb) and
 * certifies the result. Only that built part carries a certificate.
 */

import type { Concept, ConceptNumber, ConceptSet } from './concept-recipe.ts'

export function buildRequestText(set: Pick<ConceptSet, 'id' | 'title'>, concept: Pick<Concept, 'id' | 'title'>): string {
  const from = set.id ? ` (concept set ${set.id})` : set.title ? ` (concept set "${set.title}")` : ''
  return `Build concept ${concept.id} — "${concept.title}"${from}: build it in the kernel and certify it.`
}

/** Three significant figures, no trailing zeros, no exponent for sizes we meet. */
export function fmtEstimate(v: number): string {
  if (!Number.isFinite(v)) return String(v)
  if (v === 0) return '0'
  const a = Math.abs(v)
  const digits = a >= 100 ? 0 : a >= 10 ? 1 : a >= 1 ? 2 : Math.min(6, 2 - Math.floor(Math.log10(a)))
  return v.toFixed(digits).replace(/(\.\d*?)0+$/, '$1').replace(/\.$/, '')
}

/** One agent-sent number, always as an estimate with how it was obtained. */
export function conceptNumberText(n: ConceptNumber): string {
  const value = typeof n.value === 'number' ? fmtEstimate(n.value) : n.value
  return `${n.label} ${value}${n.unit ? ` ${n.unit}` : ''} · estimate, ${n.basis}`
}

/** The volume / mass estimate from sampling the recipe's field. */
export function sizeEstimateText(volumeMm3: number, massG: number | null): string {
  const size = massG !== null ? `${fmtEstimate(massG)} g` : `${fmtEstimate(volumeMm3 / 1000)} cm³`
  return size
}
