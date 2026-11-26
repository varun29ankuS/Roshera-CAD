/**
 * The app's workspaces: the one list every surface reads -- the top bar's
 * tabs, the command palette, the saved choice and the URL hash -- so a
 * workspace added here is offered, remembered and reachable everywhere at
 * once, and an unknown name is refused rather than half-handled.
 *
 * A workspace exists here only once it does something real. DFM runs the
 * kernel's rule packs; Simulate runs a static study through the kernel's
 * mesher and solver (sim-engine; interim engines named in every result).
 */

export const WORKSPACES = [
  { id: 'part', label: 'Part' },
  { id: 'assembly', label: 'Assembly' },
  { id: 'drawing', label: 'Drawing' },
  { id: 'dfm', label: 'DFM' },
  { id: 'simulate', label: 'Simulate' },
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

/**
 * Is the Part workspace parked while `id` is open?
 *
 * Drawing and Assembly bring their own workspace; the Part workspace -- its
 * viewport, tree, feature cards and tool dialogs -- stays mounted beneath
 * them, unseen (`App.tsx`). `inert` keeps the pointer and the focus out of
 * it, but NOT a key: a key listener on `window` hears every key whichever
 * workspace is open. So a parked workspace's key listeners must ask this
 * first. They did not, and Delete pressed on a drawing view also deleted the
 * feature still selected in the parked tree (2026-11-26).
 */
export function partWorkspaceParked(id: WorkspaceId): boolean {
  return id === 'drawing' || id === 'assembly'
}

/**
 * Does a key pressed in workspace `id` reach the part scene's shortcuts
 * (`lib/shortcuts.ts`: Delete, select all, the transform tools, K to sketch,
 * the selection modes, the camera presets, Escape)?
 *
 * Not from the Drawing workspace: the scene is not on screen there, and
 * Delete -- which removes the selected VIEW from the sheet -- also deleted
 * every part still selected in the parked scene. Undo and redo are the
 * document's history, not the scene's, and the sheet relies on them
 * ("Ctrl+Z undoes it"), so those two still reach.
 *
 * The Assembly workspace shows the scene's parts and uses its selection, so
 * its keys are left as they are.
 */
export function sceneShortcutApplies(id: WorkspaceId, key: string, ctrl: boolean): boolean {
  if (id !== 'drawing') return true
  const k = key.toLowerCase()
  return ctrl && (k === 'z' || k === 'y')
}
