import { useMemo } from 'react'
import { useThemeStore } from '@/stores/theme-store'

/**
 * Scene lighting that follows the active blueprint theme.
 *
 * Light theme: warm cream sky / paper ground bounce — geometry reads as
 * if lit on a drafting table.
 * Dark theme: cool cyan key / saturated navy ground bounce — geometry
 * reads as if lit under a hooded engineering lamp.
 */
export function SceneLighting() {
  const theme = useThemeStore((s) => s.theme)

  const palette = useMemo(() => {
    if (theme === 'dark') {
      return {
        ambient: '#c8cde8',
        keyLight: '#ffffff',
        fillLight: '#b0c4de',
        hemiSky: '#7090c0',
        hemiGround: '#1e1e2e',
      }
    }
    return {
      ambient: '#f0e8d8',
      keyLight: '#ffffff',
      fillLight: '#d4c8b0',
      hemiSky: '#e8e0c8',
      hemiGround: '#b0a890',
    }
  }, [theme])

  return (
    <>
      <ambientLight intensity={0.6} color={palette.ambient} />
      <directionalLight
        position={[15, 25, 10]}
        intensity={1.0}
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
        position={[-10, 10, -15]}
        intensity={0.4}
        color={palette.fillLight}
      />
      <hemisphereLight args={[palette.hemiSky, palette.hemiGround, 0.4]} />
    </>
  )
}
