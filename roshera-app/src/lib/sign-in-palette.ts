/**
 * The sign-in aircraft's colours, in the app's theme (Varun 2026-11-19:
 * "match the sign in with the theme"). Light is a technical illustration in
 * ink on paper; dark is the same drawing in light linework on graphite, so
 * signing in and the app it opens into look like one place.
 */
export interface SignInScenePalette {
  /** Toon-shaded body. */
  skin: string
  glass: string
  /** Nozzle throats and intakes: the darkest part in either theme. */
  void: string
  shade: string
  /** Fine creases, panel lines, control-surface outlines. */
  crease: string
  /** The nozzle rims and leading-edge bands. */
  rim: string
  band: string
  /** The bold outline round the whole aircraft. */
  silhouette: string
  skyLight: string
  groundLight: string
  /** The wake's overall opacity. Its particles are coloured by speed with
   *  the turbo colormap, as a CFD plot is, in both themes. */
  wakeAlpha: number
}

export type SignInTheme = 'light' | 'dark'

export function signInScenePalette(theme: SignInTheme): SignInScenePalette {
  if (theme === 'dark') {
    return {
      skin: '#2c333d',
      glass: '#3d5677',
      void: '#07090c',
      shade: '#171c22',
      crease: '#c8d0da',
      rim: '#e8edf3',
      band: '#aab4c0',
      silhouette: '#dfe5ec',
      skyLight: '#ffffff',
      groundLight: '#262b31',
      // On graphite the same particle count reads twice as heavy as on paper.
      wakeAlpha: 0.5,
    }
  }
  return {
    skin: '#f6f4ee',
    glass: '#bcd2ee',
    void: '#0b0d10',
    shade: '#20262d',
    crease: '#1a1f25',
    rim: '#12161b',
    band: '#2b3138',
    silhouette: '#12161a',
    skyLight: '#ffffff',
    groundLight: '#d8d4cc',
    wakeAlpha: 1,
  }
}
