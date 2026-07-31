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
