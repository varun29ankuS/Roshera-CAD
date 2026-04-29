import { useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { resolveCssVar } from '@/lib/css-color'

/**
 * Shader-based infinite grid on the XZ plane.
 * Renders clean single-pixel lines at any zoom level — no z-fighting.
 *
 * Cell and section colors are resolved from the blueprint design tokens
 * (`--cad-grid`, `--cad-grid-section`) so the viewport stays in lockstep
 * with the rest of the UI across theme switches.
 */

const gridVertexShader = /* glsl */ `
  varying vec3 vWorldPos;
  void main() {
    vWorldPos = (modelMatrix * vec4(position, 1.0)).xyz;
    gl_Position = projectionMatrix * viewMatrix * vec4(vWorldPos, 1.0);
  }
`

const gridFragmentShader = /* glsl */ `
  varying vec3 vWorldPos;
  uniform float uCellSize;
  uniform float uSectionSize;
  uniform vec3 uCellColor;
  uniform vec3 uSectionColor;
  uniform float uCellAlpha;
  uniform float uSectionAlpha;
  uniform float uFadeDistance;

  float gridLine(float coord, float size) {
    float d = abs(fract(coord / size - 0.5) - 0.5) * size;
    float lineWidth = fwidth(coord) * 1.0;
    return 1.0 - smoothstep(0.0, lineWidth, d);
  }

  void main() {
    float dist = length(vWorldPos.xz);
    float fade = 1.0 - smoothstep(uFadeDistance * 0.5, uFadeDistance, dist);
    if (fade < 0.001) discard;

    float cellLine = max(
      gridLine(vWorldPos.x, uCellSize),
      gridLine(vWorldPos.z, uCellSize)
    );
    float sectionLine = max(
      gridLine(vWorldPos.x, uSectionSize),
      gridLine(vWorldPos.z, uSectionSize)
    );

    vec3 color = mix(uCellColor, uSectionColor, sectionLine);
    float alpha = max(cellLine * uCellAlpha, sectionLine * uSectionAlpha) * fade;

    if (alpha < 0.01) discard;
    gl_FragColor = vec4(color, alpha);
  }
`

export function CADGrid() {
  const { visible, cellSize, sectionSize, fadeDistance } = useSceneStore(
    (s) => s.gridSettings,
  )
  const theme = useThemeStore((s) => s.theme)

  const material = useMemo(() => {
    const cell = resolveCssVar('--cad-grid')
    const section = resolveCssVar('--cad-grid-section')
    return new THREE.ShaderMaterial({
      vertexShader: gridVertexShader,
      fragmentShader: gridFragmentShader,
      uniforms: {
        uCellSize: { value: cellSize },
        uSectionSize: { value: sectionSize },
        uCellColor: { value: cell.color },
        uSectionColor: { value: section.color },
        uCellAlpha: { value: cell.alpha },
        uSectionAlpha: { value: section.alpha },
        uFadeDistance: { value: fadeDistance },
      },
      transparent: true,
      side: THREE.DoubleSide,
      depthWrite: false,
      extensions: { derivatives: true } as unknown as THREE.ShaderMaterial['extensions'],
    })
  }, [cellSize, sectionSize, fadeDistance, theme])

  if (!visible) return null

  return (
    <mesh rotation={[-Math.PI / 2, 0, 0]} position={[0, -0.001, 0]} material={material}>
      <planeGeometry args={[400, 400]} />
    </mesh>
  )
}
