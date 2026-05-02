import { useMemo, useRef } from 'react'
import { useFrame, useThree } from '@react-three/fiber'
import * as THREE from 'three'
import { useSceneStore, type SketchPlane } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Infinite ground-plane grid drawn with a screen-space `fwidth` shader so
 * cell and section lines stay a true 1-pixel wide regardless of zoom.
 * drei's `<Grid>` was rejected because its world-space thickness produces
 * fat anti-aliased bands once the camera moves close.
 *
 * The grid lies on the active sketch plane (xy / xz / yz) when sketch
 * mode is on, and on the world XZ ground plane otherwise. Plane-local
 * (px, py) coordinates are mapped onto the active plane in the vertex
 * shader via a 2x2 matrix supplied by JS, then offset by the camera's
 * projection onto the same plane so the grid feels infinite.
 *
 * Cell and section colors are read from the blueprint design tokens
 * (`--cad-grid`, `--cad-grid-section`) so the viewport tracks the rest of
 * the UI through theme switches. Lines fade out radially toward
 * `fadeDistance` so the horizon doesn't strobe.
 */

// Mesh rotation that aligns the +Z plane geometry with each sketch plane.
const PLANE_ROTATION: Record<SketchPlane, [number, number, number]> = {
  xy: [0, 0, 0],
  xz: [-Math.PI / 2, 0, 0],
  yz: [0, Math.PI / 2, 0],
}

/**
 * Per-plane mapping from local (px, py) on the unrotated plane geometry
 * to the two world-space axes that span the plane after rotation. Stored
 * as the four entries [m00, m10, m01, m11] of a column-major mat2 — the
 * order GLSL's `mat2(a, b, c, d)` constructor expects. The shader
 * computes `vGridCoord = uPlaneMap * vec2(px, py) + uCameraOnPlane`.
 *
 * Derivation:
 *   xy: rotation [0,0,0]      → local x→worldX, local y→worldY  → identity
 *   xz: rotation [-π/2,0,0]   → local x→worldX, local y→world(-Z) → diag(1,-1)
 *   yz: rotation [0,π/2,0]    → local x→world(-Z), local y→worldY → swap+sign
 */
const PLANE_MAP: Record<SketchPlane, [number, number, number, number]> = {
  xy: [1, 0, 0, 1],
  xz: [1, 0, 0, -1],
  yz: [0, -1, 1, 0],
}

/** World-space mesh origin so the plane stays centered under the camera. */
function meshPositionForPlane(
  plane: SketchPlane,
  camera: THREE.Camera,
  out: THREE.Vector3,
): THREE.Vector3 {
  switch (plane) {
    case 'xy':
      return out.set(camera.position.x, camera.position.y, 0)
    case 'xz':
      return out.set(camera.position.x, 0, camera.position.z)
    case 'yz':
      return out.set(0, camera.position.y, camera.position.z)
  }
}

/** Camera projected onto the active plane in (u, v) coordinates. */
function cameraOnPlane(
  plane: SketchPlane,
  camera: THREE.Camera,
  out: THREE.Vector2,
): THREE.Vector2 {
  switch (plane) {
    case 'xy':
      return out.set(camera.position.x, camera.position.y)
    case 'xz':
      return out.set(camera.position.x, camera.position.z)
    case 'yz':
      return out.set(camera.position.y, camera.position.z)
  }
}

export function CADGrid() {
  const { visible, cellSize, sectionSize, fadeDistance } = useSceneStore(
    (s) => s.gridSettings,
  )
  // Track the sketch plane only when sketch mode is active; otherwise the
  // grid stays on the conventional XZ ground plane.
  const sketchActive = useSceneStore((s) => s.sketch.active)
  const sketchPlane = useSceneStore((s) => s.sketch.plane)
  const activePlane: SketchPlane = sketchActive ? sketchPlane : 'xz'

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

  const uniforms = useMemo<{ [name: string]: THREE.IUniform }>(() => {
    // Pre-seed the plane map with the XZ ground-plane mapping so the
    // very first render before useFrame ticks is already correct.
    const initialMap = new THREE.Matrix3().set(
      PLANE_MAP.xz[0], PLANE_MAP.xz[2], 0,
      PLANE_MAP.xz[1], PLANE_MAP.xz[3], 0,
      0, 0, 1,
    )
    return {
      uCellSize: { value: cellSize },
      uSectionSize: { value: sectionSize },
      uCellColor: { value: cellColor },
      uSectionColor: { value: sectionColor },
      uCellAlpha: { value: cellAlpha },
      uSectionAlpha: { value: sectionAlpha },
      uFadeDistance: { value: fadeDistance },
      uCameraOnPlane: { value: new THREE.Vector2(0, 0) },
      uPlaneMap: { value: initialMap }, // upper-left 2x2 carries the local-to-plane mapping
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Drive the mesh transform AND the uniforms from the same frame tick.
  // OrbitControls mutates the camera imperatively without triggering a
  // React re-render, so binding the mesh transform via JSX props would
  // leave the transform a frame (or many) behind while the shader's
  // reconstructed plane coordinates stay current — the visible result
  // would be the grid lattice sliding relative to the world axes.
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

    cameraOnPlane(activePlane, camera, u.uCameraOnPlane.value as THREE.Vector2)

    // Update the plane map's upper-left 2x2 to reflect the current plane.
    // Matrix3.set takes row-major args: (n11, n12, n13, n21, n22, n23, ...).
    // PLANE_MAP packs column-major (a, b, c, d) where the GLSL mat reads
    // | a c | / | b d |, so n11=a, n12=c, n21=b, n22=d.
    const map = PLANE_MAP[activePlane]
    ;(u.uPlaneMap.value as THREE.Matrix3).set(
      map[0], map[2], 0,
      map[1], map[3], 0,
      0, 0, 1,
    )

    meshPositionForPlane(activePlane, camera, m.position)
    m.rotation.set(
      PLANE_ROTATION[activePlane][0],
      PLANE_ROTATION[activePlane][1],
      PLANE_ROTATION[activePlane][2],
    )
  })

  if (!visible) return null

  return (
    <mesh
      ref={meshRef}
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

// Plane-local (px, py) → in-plane (u, v), then add the camera's
// projection onto the plane to recover an absolute lattice coordinate.
// `uPlaneMap` is the column-major 2x2 (carried as a mat3 so we get a
// stable layout in three.js) that handles each plane's axis swap/sign.
const VERT = /* glsl */ `
varying vec2 vGridCoord;
uniform vec2 uCameraOnPlane;
uniform mat3 uPlaneMap;

void main() {
  vec2 mapped = vec2(
    uPlaneMap[0][0] * position.x + uPlaneMap[1][0] * position.y,
    uPlaneMap[0][1] * position.x + uPlaneMap[1][1] * position.y
  );
  vGridCoord = mapped + uCameraOnPlane;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`

// Crisp 1px lines via `fwidth`: divide plane coordinate by spacing, take
// fract distance from each integer, and compare against the derivative —
// produces a constant pixel-thick line at any zoom.
const FRAG = /* glsl */ `
precision highp float;

varying vec2 vGridCoord;
uniform float uCellSize;
uniform float uSectionSize;
uniform vec3 uCellColor;
uniform vec3 uSectionColor;
uniform float uCellAlpha;
uniform float uSectionAlpha;
uniform float uFadeDistance;
uniform vec2 uCameraOnPlane;

float gridFactor(vec2 coord, float spacing) {
  vec2 g = coord / spacing;
  vec2 d = abs(fract(g - 0.5) - 0.5) / fwidth(g);
  float line = min(d.x, d.y);
  return 1.0 - min(line, 1.0);
}

void main() {
  float cell = gridFactor(vGridCoord, uCellSize);
  float section = gridFactor(vGridCoord, uSectionSize);

  // Section lines win over cell lines where they coincide.
  vec3 color = mix(uCellColor, uSectionColor, section);
  float alpha = max(cell * uCellAlpha, section * uSectionAlpha);

  // Radial fade to horizon so distant cells don't moir\u00e9.
  float dist = length(vGridCoord - uCameraOnPlane);
  float fade = 1.0 - smoothstep(uFadeDistance * 0.5, uFadeDistance, dist);
  alpha *= fade;

  if (alpha < 0.001) discard;
  gl_FragColor = vec4(color, alpha);
}
`
