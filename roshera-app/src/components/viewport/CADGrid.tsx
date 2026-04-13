import { useMemo } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'

/**
 * Shader-based infinite grid on the XZ plane.
 * Renders clean single-pixel lines at any zoom level — no z-fighting.
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
    float alpha = max(cellLine * 0.3, sectionLine * 0.5) * fade;

    if (alpha < 0.01) discard;
    gl_FragColor = vec4(color, alpha);
  }
`

const GRID_COLORS = {
  dark: { cell: '#2a2f45', section: '#4a5080' },
  light: { cell: '#c0c4d0', section: '#8890aa' },
}

export function CADGrid() {
  const { visible, cellSize, sectionSize, fadeDistance } = useSceneStore(
    (s) => s.gridSettings,
  )
  const theme = useThemeStore((s) => s.theme)

  const material = useMemo(() => {
    const colors = GRID_COLORS[theme]
    return new THREE.ShaderMaterial({
      vertexShader: gridVertexShader,
      fragmentShader: gridFragmentShader,
      uniforms: {
        uCellSize: { value: cellSize },
        uSectionSize: { value: sectionSize },
        uCellColor: { value: new THREE.Color(colors.cell) },
        uSectionColor: { value: new THREE.Color(colors.section) },
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
