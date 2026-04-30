import { useMemo, useRef } from 'react'
import { useFrame, useThree } from '@react-three/fiber'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Infinite ground-plane grid drawn with a screen-space `fwidth` shader so
 * cell and section lines stay a true 1-pixel wide regardless of zoom.
 * drei's `<Grid>` was rejected because its world-space thickness produces
 * fat anti-aliased bands once the camera moves close.
 *
 * Cell and section colors are read from the blueprint design tokens
 * (`--cad-grid`, `--cad-grid-section`) so the viewport tracks the rest of
 * the UI through theme switches. Lines fade out radially toward
 * `fadeDistance` so the horizon doesn't strobe.
 */
export function CADGrid() {
  const { visible, cellSize, sectionSize, fadeDistance } = useSceneStore(
    (s) => s.gridSettings,
  )
  const theme = useThemeStore((s) => s.theme)
  const { camera } = useThree()
  const matRef = useRef<THREE.ShaderMaterial>(null)
  const meshRef = useRef<THREE.Mesh>(null)

  const { cellColor, sectionColor, cellAlpha, sectionAlpha } = useMemo(() => {
    const cell = resolveCssVar('--cad-grid')
    const section = resolveCssVar('--cad-grid-section')
    return {
      cellColor: cell.color,
      sectionColor: section.color,
      cellAlpha: cell.alpha,
      sectionAlpha: section.alpha,
    }
    // Re-resolve on theme switch so CSS-var changes flow through.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  // Single big quad covering the visible region, recentered on the camera
  // each frame so the grid feels infinite without rebuilding geometry.
  const PLANE_SIZE = 4000

  const uniforms = useMemo<{ [name: string]: THREE.IUniform }>(
    () => ({
      uCellSize: { value: cellSize },
      uSectionSize: { value: sectionSize },
      uCellColor: { value: cellColor },
      uSectionColor: { value: sectionColor },
      uCellAlpha: { value: cellAlpha },
      uSectionAlpha: { value: sectionAlpha },
      uFadeDistance: { value: fadeDistance },
      uCameraXZ: { value: new THREE.Vector2(0, 0) },
    }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  )

  // Drive the mesh transform AND the uniforms from the same frame tick.
  // OrbitControls mutates the camera imperatively without triggering a
  // React re-render, so binding the mesh position to camera.x/z via the
  // JSX prop would leave the transform a frame (or many) behind while the
  // shader's reconstructed world XZ stays current — the visible result
  // is the grid lattice sliding relative to the world-fixed axes.
  useFrame(() => {
    const u = matRef.current?.uniforms
    const m = meshRef.current
    if (!u || !m) return
    u.uCellSize.value = cellSize
    u.uSectionSize.value = sectionSize
    u.uCellColor.value = cellColor
    u.uSectionColor.value = sectionColor
    u.uCellAlpha.value = cellAlpha
    u.uSectionAlpha.value = sectionAlpha
    u.uFadeDistance.value = fadeDistance
    ;(u.uCameraXZ.value as THREE.Vector2).set(camera.position.x, camera.position.z)
    m.position.set(camera.position.x, 0, camera.position.z)
  })

  if (!visible) return null

  return (
    <mesh
      ref={meshRef}
      rotation={[-Math.PI / 2, 0, 0]}
      frustumCulled={false}
      renderOrder={-1}
    >
      <planeGeometry args={[PLANE_SIZE, PLANE_SIZE, 1, 1]} />
      <shaderMaterial
        ref={matRef}
        uniforms={uniforms}
        transparent
        depthWrite={false}
        side={THREE.DoubleSide}
        vertexShader={VERT}
        fragmentShader={FRAG}
      />
    </mesh>
  )
}

// World-XZ position is reconstructed in the fragment shader by undoing
// the mesh's recenter offset, so cell/section lines stay locked to the
// world origin no matter where the camera roams.
const VERT = /* glsl */ `
varying vec2 vWorldXZ;
uniform vec2 uCameraXZ;

void main() {
  // After the [-PI/2, 0, 0] rotation the plane lies on world XZ with its
  // local +X mapping to world +X and local +Y mapping to world -Z. We
  // pre-shift by the camera so the plane follows the eye; add it back to
  // recover absolute world coordinates for the line lattice.
  vWorldXZ = vec2(position.x + uCameraXZ.x, -position.y + uCameraXZ.y);
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`

// Crisp 1px lines via `fwidth`: divide world coordinate by spacing,
// take fract distance from each integer, and compare against the
// derivative — produces a constant pixel-thick line at any zoom.
const FRAG = /* glsl */ `
precision highp float;

varying vec2 vWorldXZ;
uniform float uCellSize;
uniform float uSectionSize;
uniform vec3 uCellColor;
uniform vec3 uSectionColor;
uniform float uCellAlpha;
uniform float uSectionAlpha;
uniform float uFadeDistance;
uniform vec2 uCameraXZ;

float gridFactor(vec2 coord, float spacing) {
  vec2 g = coord / spacing;
  vec2 d = abs(fract(g - 0.5) - 0.5) / fwidth(g);
  float line = min(d.x, d.y);
  return 1.0 - min(line, 1.0);
}

void main() {
  float cell = gridFactor(vWorldXZ, uCellSize);
  float section = gridFactor(vWorldXZ, uSectionSize);

  // Section lines win over cell lines where they coincide.
  vec3 color = mix(uCellColor, uSectionColor, section);
  float alpha = max(cell * uCellAlpha, section * uSectionAlpha);

  // Radial fade to horizon so distant cells don't moir\u00e9.
  float dist = length(vWorldXZ - uCameraXZ);
  float fade = 1.0 - smoothstep(uFadeDistance * 0.5, uFadeDistance, dist);
  alpha *= fade;

  if (alpha < 0.001) discard;
  gl_FragColor = vec4(color, alpha);
}
`
