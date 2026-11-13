/**
 * Which camera preset a click on the view cube means, in the kernel's Z-up
 * world (+Z top, -Y front, +X right). The cube is three kinds of piece:
 * the faces (centred at the origin), and small edge and corner boxes placed
 * out on the cube. A click on an edge or corner box reports the normal of
 * whichever side of the little box was hit, so those go by where the box
 * sits, not by the side hit.
 */
interface V3 {
  x: number
  y: number
  z: number
}

/** The side a direction points at: its largest component (ties: Z, then X). */
function sideOf(n: V3): string {
  const ax = Math.abs(n.x)
  const ay = Math.abs(n.y)
  const az = Math.abs(n.z)
  if (az >= ax && az >= ay) return n.z > 0 ? 'top' : 'bottom'
  if (ax >= ay) return n.x > 0 ? 'right' : 'left'
  return n.y > 0 ? 'back' : 'front'
}

/** `piece`: where the clicked piece sits (the origin for a face);
 *  `normal`: the normal of the side that was hit. */
export function cubePreset(piece: V3, normal: V3): string {
  const off = [piece.x, piece.y, piece.z].filter((c) => Math.abs(c) > 1e-6).length
  if (off === 0) return sideOf(normal)
  // A corner has no one face; the iso view is the corner view the app has.
  if (off === 3) return 'isometric'
  // An edge: the view of one of its two faces (the top/bottom one first).
  return sideOf(piece)
}
