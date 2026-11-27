/**
 * The viewport's display modes, as the view bar offers them: shaded with
 * edges, shaded, or sketch (the concept cards' pencil look on the real mesh,
 * lib/sketch-material.ts). Two store flags carry them -- the edge lines'
 * visibility and the sketch style -- and sketch wins: it draws its own ink
 * edges whatever the edge flag says. Section is a separate toggle.
 */

export type DisplayMode = 'edges' | 'shaded' | 'sketch'

export function displayModeOf(s: { edges: boolean; sketch: boolean }): DisplayMode {
  if (s.sketch) return 'sketch'
  return s.edges ? 'edges' : 'shaded'
}

export function displayModeSettings(mode: DisplayMode): { edges: boolean; sketch: boolean } {
  switch (mode) {
    case 'edges':
      return { edges: true, sketch: false }
    case 'shaded':
      return { edges: false, sketch: false }
    case 'sketch':
      return { edges: true, sketch: true }
  }
}
