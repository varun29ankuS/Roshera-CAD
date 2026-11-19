/**
 * The drawings the Drawing workspace lists for the open document.
 *
 * A drawing bound to another document cannot render against this model (the
 * server refuses with "does not resolve against the server's current live
 * state"), so it is never listed. When the open document is not known yet,
 * no document-owned drawing is listed -- fail closed, never "keep them all"
 * (that opened the previous document's drawing in a fresh one; found
 * rehearsing the demo video, 2026-11-18). Part-owned drawings carry no
 * document and stay listed; the server binds them to their part.
 */
export type DrawingOwnerLike = { kind: 'part'; part_id: string } | { kind: 'legacy'; document_id: string }

export function drawingsForDocument<T extends { owner: DrawingOwnerLike }>(entries: T[], openDocument: string | null): T[] {
  return entries.filter((e) => e.owner.kind !== 'legacy' || (openDocument !== null && e.owner.document_id === openDocument))
}
