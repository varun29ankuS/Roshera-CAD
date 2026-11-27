/**
 * SKETCH MATERIAL -- the viewport's Sketch display mode.
 * =====================================================
 * Draws the REAL kernel mesh (the tessellation the viewport already holds)
 * in the concept cards' pencil look, by patching each part's own
 * MeshStandardMaterial rather than replacing it. Everything that material
 * already does for the viewport keeps working unchanged: section clipping
 * planes, the selection glow (emissive), the low opacity while a sketch is
 * edited, double-sided faces, polygon offset under the edge lines. Picking is
 * a CPU raycast and never touches the shader.
 *
 * The look comes from lib/sketch-style.ts (SKETCH_STYLE_GLSL), the same
 * library the concept cards' shader uses:
 *   - tone and hatching from the shared light, in the viewer's frame (the
 *     standard shader's `normal` is a view-space normal);
 *   - paper and ink from the theme (shared uniforms, sRGB like the cards,
 *     converted once for Three's output transform; tone mapping off);
 *   - ink on the silhouette where the surface turns away from the viewer
 *     (a mesh has no distance field to find it the cards' way) -- the creases
 *     are the part's edge lines, drawn in ink by CADMesh;
 *   - the stepped top-to-bottom print when the mode is switched on.
 * All sketched materials share ONE program (one cache key) and ONE uniform
 * set, so a theme change or a reveal step is a single update for every part.
 */

import * as THREE from 'three'
import { SKETCH_STYLE_GLSL } from './sketch-style.ts'

export interface SketchUniforms {
  uSketchInk: { value: THREE.Vector3 }
  uSketchFace: { value: THREE.Vector3 }
  uSketchReveal: { value: number }
  /** The drawing buffer's size, device pixels. */
  uSketchRes: { value: THREE.Vector2 }
}

export function createSketchUniforms(): SketchUniforms {
  return {
    uSketchInk: { value: new THREE.Vector3(0.2, 0.2, 0.2) },
    uSketchFace: { value: new THREE.Vector3(0.95, 0.95, 0.95) },
    uSketchReveal: { value: 1 },
    uSketchRes: { value: new THREE.Vector2(1, 1) },
  }
}

/** Silhouette ink: where |n . v| falls below these, the surface is turning away. */
const RIM_FROM = 0.12
const RIM_TO = 0.3

const HOOK = '#include <opaque_fragment>'

const DECLARATIONS = `
uniform vec3 uSketchInk;
uniform vec3 uSketchFace;
uniform float uSketchReveal;
uniform vec2 uSketchRes;
${SKETCH_STYLE_GLSL}
`

const SHADE = `
  {
    if (sketchUnrevealed(uSketchReveal, uSketchRes.y)) discard;
    vec3 sketchN = normalize(normal);
    vec3 sketchV = isOrthographic ? vec3(0.0, 0.0, 1.0) : normalize(vViewPosition);
    vec3 sketchL = sketchLightDir(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, -1.0));
    float sketchLam = max(dot(sketchN, sketchL), 0.0);
    float sketchRim = 1.0 - smoothstep(${RIM_FROM.toFixed(2)}, ${RIM_TO.toFixed(2)}, abs(dot(sketchN, sketchV)));
    vec3 sketchCol = mix(uSketchFace, uSketchInk, max(sketchShadeInk(sketchLam), sketchRim));
    outgoingLight = sRGBTransferEOTF(vec4(sketchCol, 1.0)).rgb + totalEmissiveRadiance;
  }
  ${HOOK}`

/**
 * The standard fragment shader with the sketch shading put in front of the
 * colour write. Refuses a shader without the hook: a Three upgrade that moves
 * the chunk must fail loudly, never leave parts silently unsketched.
 */
export function sketchifyFragment(fragmentShader: string): string {
  if (!fragmentShader.includes(HOOK)) throw new Error(`sketch material: no ${HOOK} in this fragment shader`)
  const main = fragmentShader.indexOf('void main()')
  if (main < 0) throw new Error('sketch material: no main() in this fragment shader')
  const withDecls = fragmentShader.slice(0, main) + DECLARATIONS + fragmentShader.slice(main)
  return withDecls.replace(HOOK, SHADE)
}

const CACHE_KEY = 'roshera-sketch-style-v1'

/** Turn `material` into a sketched one, sharing `uniforms` with every other. */
export function applySketchStyle(material: THREE.MeshStandardMaterial, uniforms: SketchUniforms): void {
  material.toneMapped = false
  material.onBeforeCompile = (shader) => {
    Object.assign(shader.uniforms, uniforms)
    shader.fragmentShader = sketchifyFragment(shader.fragmentShader)
  }
  material.customProgramCacheKey = () => CACHE_KEY
  material.needsUpdate = true
}

/** The one uniform set every sketched part in the viewport shares. */
export const viewportSketchUniforms: SketchUniforms = createSketchUniforms()
