// Run: node --experimental-strip-types --test test/sketch-material.test.mjs
//
// The viewport's Sketch display mode (lib/sketch-material.ts): the REAL kernel
// mesh drawn in the concept cards' pencil style by patching the parts' own
// MeshStandardMaterial, so everything the standard material does for the
// viewport -- section clipping planes, the selection glow (emissive), opacity
// while a sketch is edited, double-sided faces -- keeps working. These pin
// the patch against Three's real shader source, so a Three upgrade that moves
// the chunk we hook fails here, not silently on screen.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import * as THREE from 'three'

const M = await import('../src/lib/sketch-material.ts')
const { displayModeOf, displayModeSettings } = await import('../src/lib/display-mode.ts')

const STANDARD = THREE.ShaderLib.standard.fragmentShader

test("the patch hooks Three's real standard fragment shader", () => {
  const out = M.sketchifyFragment(STANDARD)
  assert.notEqual(out, STANDARD)
  assert.ok(out.includes('SKETCH_STYLE'), 'the shared library is in')
  assert.ok(out.indexOf('sketchShadeInk(') < out.indexOf('#include <opaque_fragment>'), 'shading replaces the colour before it is written')
  assert.ok(out.includes('totalEmissiveRadiance'), 'the selection glow (emissive) is kept')
  assert.ok(out.includes('#include <clipping_planes_fragment>'), 'section clipping untouched')
  assert.ok(out.includes('sRGBTransferEOTF'), 'the sketch colours are sRGB, converted once for the output transform')
})

test('a shader without the hook is refused, never silently left unsketched', () => {
  assert.throws(() => M.sketchifyFragment('void main() { gl_FragColor = vec4(1.0); }'), /opaque_fragment/)
})

test('a sketched material shares one program and the shared uniforms', () => {
  const uniforms = M.createSketchUniforms()
  const a = new THREE.MeshStandardMaterial({ color: 0xff0000 })
  const b = new THREE.MeshStandardMaterial({ color: 0x00ff00 })
  M.applySketchStyle(a, uniforms)
  M.applySketchStyle(b, uniforms)
  assert.equal(a.customProgramCacheKey(), b.customProgramCacheKey())
  assert.equal(a.toneMapped, false, 'no tone mapping: the paper and ink are the card colours')
  const shader = { fragmentShader: STANDARD, vertexShader: THREE.ShaderLib.standard.vertexShader, uniforms: {} }
  a.onBeforeCompile(shader, null)
  assert.equal(shader.uniforms.uSketchInk, uniforms.uSketchInk, 'uniform objects are shared, so one update reaches every part')
  assert.equal(shader.uniforms.uSketchReveal, uniforms.uSketchReveal)
})

test('display modes: shaded + edges, shaded, sketch', () => {
  assert.equal(displayModeOf({ edges: true, sketch: false }), 'edges')
  assert.equal(displayModeOf({ edges: false, sketch: false }), 'shaded')
  assert.equal(displayModeOf({ edges: true, sketch: true }), 'sketch')
  assert.equal(displayModeOf({ edges: false, sketch: true }), 'sketch', 'sketch wins: it draws its own ink edges')
  assert.deepEqual(displayModeSettings('edges'), { edges: true, sketch: false })
  assert.deepEqual(displayModeSettings('shaded'), { edges: false, sketch: false })
  assert.deepEqual(displayModeSettings('sketch'), { edges: true, sketch: true })
})
