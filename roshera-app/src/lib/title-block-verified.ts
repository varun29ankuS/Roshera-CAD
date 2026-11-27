/**
 * Approved, but verified?
 *
 * A sheet's title block carries two different statements: STATUS (a person
 * signed it off -- "approved by" is filled in) and VERIFIED (the kernel's
 * verification stamp). They can disagree: a person can approve a sheet the
 * kernel has not verified. That is allowed -- approval is a human act -- but
 * it must not look like an ordinary approval, so it is flagged.
 *
 * The kernel prints the stamp in the sheet's own SVG and exposes it nowhere
 * else, so it is read from there: the value beside the VERIFIED label.
 */

const VERIFIED_FIELD = />\s*VERIFIED\s*<\/text>\s*<text[^>]*class="field-value[^"]*"[^>]*>([^<]*)<\/text>/

/** true / false as the title block states it; null when the sheet has no
 *  verification field (or it is empty), which is neither. */
export function titleBlockVerified(svg: string): boolean | null {
  const m = VERIFIED_FIELD.exec(svg)
  if (!m) return null
  const value = m[1].trim()
  if (value === '' || value === '—' || value === '-') return null
  return value.toUpperCase() !== 'UNVERIFIED'
}

export type ApprovalFlag = 'for-review' | 'approved' | 'approved-unverified'

/** What the status chip shows. Only a KNOWN unverified sheet is flagged. */
export function approvalFlag(approved: boolean, verified: boolean | null): ApprovalFlag {
  if (!approved) return 'for-review'
  return verified === false ? 'approved-unverified' : 'approved'
}
