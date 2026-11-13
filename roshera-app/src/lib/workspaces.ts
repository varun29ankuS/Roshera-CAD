/**
 * The app's workspaces: the one list every surface reads -- the top bar's
 * tabs, the command palette, the saved choice and the URL hash -- so a
 * workspace added here is offered, remembered and reachable everywhere at
 * once, and an unknown name is refused rather than half-handled.
 *
 * A workspace exists here only once it does something real. Simulate joins
 * when the kernel has a mesher and a solver behind it.
 */

export const WORKSPACES = [
  { id: 'part', label: 'Part' },
  { id: 'assembly', label: 'Assembly' },
  { id: 'drawing', label: 'Drawing' },
] as const

export type WorkspaceId = (typeof WORKSPACES)[number]['id']

/** A workspace id from untrusted text (a saved value, a URL hash), or null. */
export function parseWorkspace(raw: unknown): WorkspaceId | null {
  if (typeof raw !== 'string') return null
  const id = raw.trim().toLowerCase()
  return WORKSPACES.some((w) => w.id === id) ? (id as WorkspaceId) : null
}

/** The tab label for a workspace. */
export function workspaceLabel(id: WorkspaceId): string {
  return WORKSPACES.find((w) => w.id === id)?.label ?? id
}
