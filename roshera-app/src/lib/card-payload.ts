/**
 * Whether a Blackboard card's payload is structured data (a JSON object or
 * array) the card schema should validate, rather than a readable line a tool
 * printed into the card's fence -- e.g. a GD&T verdict "⌖ 0.10mm A B -> IN
 * SPEC (measured 0.00mm)". A plain-text payload is shown as text; only a
 * structured one that fails its schema is reported as invalid (2026-11-19).
 */
export function isStructuredPayload(source: string): boolean {
  const s = source.trimStart()
  return s.startsWith('{') || s.startsWith('[')
}
