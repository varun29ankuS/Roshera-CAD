/**
 * The material a part is drawn with when the server sends none (parts the
 * agent tools build arrive without one). A light steel grey, the same as the
 * server's own default diffuse (0.7, 0.7, 0.75), so every part reads alike.
 * One place: the live scene and the timeline's scrub frames both use it.
 */
export const DEFAULT_PART_MATERIAL = {
  color: '#b3b3bf',
  metalness: 0.1,
  roughness: 0.6,
  opacity: 1,
} as const
