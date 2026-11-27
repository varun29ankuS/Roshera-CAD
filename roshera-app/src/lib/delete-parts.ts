/**
 * Delete parts through the kernel, and say so when it does not happen.
 *
 * The part leaves the screen only when the kernel confirms (the
 * `ObjectDeleted` broadcast), so a refused or unreachable delete used to look
 * like a key that did nothing: the failure went to the browser console,
 * where no engineer looks. Each failure is now reported by the part's name.
 *
 * Shared by the Delete key and Edit > Delete, which were two copies of the
 * same loop.
 */

export interface PartToDelete {
  id: string
  name: string
}

/** Ask the kernel to delete one part. */
export type DeleteRequest = (id: string) => Promise<{ ok: boolean; status: number }>

/** Delete `parts` one by one; `report` gets one line per part that was NOT
 *  deleted. Resolves to how many the kernel accepted. */
export async function deleteParts(
  parts: readonly PartToDelete[],
  request: DeleteRequest,
  report: (line: string) => void,
): Promise<number> {
  let deleted = 0
  for (const part of parts) {
    try {
      const resp = await request(part.id)
      if (resp.ok) {
        deleted += 1
      } else {
        report(`Could not delete ${part.name}: the kernel refused (${resp.status}). It is still in the document.`)
      }
    } catch {
      report(`Could not delete ${part.name}: the kernel could not be reached. It is still in the document.`)
    }
  }
  return deleted
}
