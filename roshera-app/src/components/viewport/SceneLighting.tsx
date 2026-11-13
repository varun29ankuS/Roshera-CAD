import { useMemo } from 'react'
import { useThemeStore } from '@/stores/theme-store'

/**
 * Scene lighting that follows the theme. Neutral, like the mockup's kernel
 * renders (2026-11-14): white key and a soft grey sky/ground bounce, so a
 * part reads light grey with crisp edges and no colour cast. The dark theme
 * keeps the same neutrality, a little dimmer.
 */
export function SceneLighting() {
  const theme = useThemeStore((s) => s.theme)

  const palette = useMemo(() => {
    if (theme === 'dark') {
      return {
        ambient: '#e4e6ea',
        keyLight: '#ffffff',
        fillLight: '#c9ced6',
        hemiSky: '#9aa3ae',
        hemiGround: '#2a2d33',
        ambientIntensity: 0.4,
      }
    }
    return {
      ambient: '#ffffff',
      keyLight: '#ffffff',
      fillLight: '#e6e8eb',
      hemiSky: '#f4f5f7',
      hemiGround: '#a7adb4',
      ambientIntensity: 0.45,
    }
  }, [theme])

  return (
    <>
      <ambientLight intensity={palette.ambientIntensity} color={palette.ambient} />
      {/* Z-up world (main.tsx sets DEFAULT_UP): the key light is above the
          part, in front and to the right -- the default iso camera's side --
          so the tops and the faces you look at are lit. (It sat at
          [15, 25, 10], "above" only in a Y-up world: behind the part, with
          the fill below it, which left the part dark.) */}
      <directionalLight
        position={[20, -25, 35]}
        intensity={2.4}
        color={palette.keyLight}
        castShadow
        shadow-mapSize-width={4096}
        shadow-mapSize-height={4096}
        shadow-camera-near={1}
        shadow-camera-far={400}
        // Frustum sized to cover typical parts (was ±30 — clipped anything
        // larger than 60mm, e.g. a 120mm bracket, producing hard "square"
        // shadow artifacts where the part fell outside the shadow map).
        shadow-camera-left={-150}
        shadow-camera-right={150}
        shadow-camera-top={150}
        shadow-camera-bottom={-150}
        // `normalBias` offsets the shadow lookup along the surface normal —
        // the modern cure for shadow ACNE (the random square/moiré self-
        // shadowing on lit faces). `bias` alone (the old -0.0005) can't fix
        // acne across a large frustum without causing peter-panning.
        shadow-bias={-0.0006}
        shadow-normalBias={0.04}
      />
      <directionalLight
        position={[-25, 20, 12]}
        intensity={0.8}
        color={palette.fillLight}
      />
      <hemisphereLight args={[palette.hemiSky, palette.hemiGround, 0.5]} />
    </>
  )
}
