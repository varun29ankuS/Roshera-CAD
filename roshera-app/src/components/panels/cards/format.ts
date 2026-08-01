/** Compact numeric formatting for measured values — no float noise. Kept
 *  out of the component files so they export only components
 *  (react-refresh constraint). */
export function fmtNum(v: number): string {
  if (!Number.isFinite(v)) return String(v)
  if (v !== 0 && (Math.abs(v) >= 1e5 || Math.abs(v) < 1e-3)) {
    return v.toExponential(2)
  }
  return String(Number(v.toPrecision(5)))
}

/**
 * Genus from the Euler characteristic, g = (2 − χ) / 2 — valid ONLY for a
 * closed, connected, orientable surface. The caller must gate on
 * `watertight && manifold` (closed + orientable) before showing this; a
 * non-integer result (odd χ) means the precondition doesn't hold here and
 * the derivation is withheld rather than shown wrong.
 */
export function eulerGenus(chi: number): number | null {
  if (!Number.isInteger(chi)) return null
  const g = (2 - chi) / 2
  return Number.isInteger(g) && g >= 0 ? g : null
}
