/**
 * Merging an `ObjectCreated` frame into the scene.
 *
 * The backend re-broadcasts `ObjectCreated` under an EXISTING uuid when a
 * kernel op changes a part in place (a transform, a face-extrude). For such a
 * frame only the geometry and the pose are new: the user's name, the part's
 * colour, its visibility and its type stay, and it is not announced as a new
 * part — no camera flight to it, no "Created …" line. The broadcast carries a
 * default material, so taking it would silently erase a colour the part
 * already has. A frame for an id the scene does not have is a new part and
 * is taken as sent.
 *
 * Pure module so it runs under `node --test`.
 */
import type { CADObject } from '@/stores/scene-store'

export interface MergedObjectCreated {
  obj: CADObject
  /** Frame the camera on it (a genuinely new part only). */
  announceAsNew: boolean
  /** The blackboard "Created …" line, or null. */
  echoMessage: string | null
}

export function mergeObjectCreated(
  incoming: CADObject,
  existing: CADObject | undefined,
  echo: () => string | null,
): MergedObjectCreated {
  if (!existing) {
    return { obj: incoming, announceAsNew: true, echoMessage: echo() }
  }
  return {
    obj: {
      ...incoming,
      name: existing.name,
      objectType: existing.objectType,
      material: existing.material,
      visible: existing.visible,
      locked: existing.locked,
      parentId: existing.parentId,
    },
    announceAsNew: false,
    echoMessage: null,
  }
}
