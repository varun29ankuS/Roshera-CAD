/**
 * CONCEPT RENDERER -- one shared WebGL context for every concept sketch.
 * =====================================================================
 * A board's history can hold many concept cards. Chrome keeps only about
 * sixteen live WebGL contexts and drops the OLDEST when a new one is made --
 * which would be the main viewport's. So no card owns a context: this module
 * keeps ONE offscreen Three.js renderer, draws a card's sketch into it on
 * demand (first view, a drag, a point-of-view change -- never in a loop) and
 * copies the frame into that card's own 2D canvas.
 *
 * The picture is the recipe's field sphere-traced in a fragment shader
 * (concept-glsl.ts, generated from the same prepared primitives the numbers
 * use). One compiled material per recipe, kept in a small cache.
 */

import * as THREE from 'three'
import type { ConceptLoad, ConceptSupport, Recipe } from './concept-recipe.ts'
import { MAX_LOAD_POINTS } from './concept-recipe.ts'
import { CONCEPT_VERTEX_SHADER, conceptFragmentShader, type PovCode } from './concept-glsl.ts'
import { cross, dot, length, normalize, scale, sub, add, type Bounds, type Vec3 } from './concept-sdf.ts'
import { resolveCssVar } from './css-color.ts'

export interface OrbitView {
  /** Radians about +Z, measured from +X. */
  yaw: number
  /** Radians above the XY plane. */
  pitch: number
}

/** An isometric-like start: looking from +X +Y +Z, onto the inside of an L. */
export const DEFAULT_VIEW: OrbitView = { yaw: Math.PI / 4 - 0.35, pitch: 0.52 }

export interface ViewFrame {
  target: Vec3
  right: Vec3
  up: Vec3
  fwd: Vec3
  /** Half the view's height, mm (orthographic). */
  half: number
  aspect: number
}

/** The orthographic frame that fits `b` for an orbit view, Z up. */
export function viewFrame(b: Bounds, view: OrbitView, aspect: number): ViewFrame {
  const target: Vec3 = scale(add(b.min, b.max), 0.5)
  const dir: Vec3 = [
    Math.cos(view.pitch) * Math.cos(view.yaw),
    Math.cos(view.pitch) * Math.sin(view.yaw),
    Math.sin(view.pitch),
  ]
  const fwd = scale(dir, -1)
  const right = normalize(cross(fwd, [0, 0, 1]))
  const up = cross(right, fwd)
  const radius = 0.5 * length(sub(b.max, b.min))
  const half = (radius * 1.06) / Math.min(1, aspect)
  return { target, right, up, fwd, half, aspect }
}

/** Where a world point lands on the card's canvas, CSS px. */
export function projectToCanvas(p: Vec3, f: ViewFrame, width: number, height: number): [number, number] {
  const d = sub(p, f.target)
  const x = dot(d, f.right) / (f.half * f.aspect)
  const y = dot(d, f.up) / f.half
  return [((x + 1) / 2) * width, (1 - (y + 1) / 2) * height]
}

interface Shared {
  renderer: THREE.WebGLRenderer
  scene: THREE.Scene
  camera: THREE.OrthographicCamera
  mesh: THREE.Mesh
}

let shared: Shared | null = null
let unavailable = false
const materials = new Map<string, THREE.ShaderMaterial>()
const MATERIAL_CACHE = 24

function getShared(): Shared | null {
  if (shared) return shared
  if (unavailable || typeof document === 'undefined') return null
  try {
    const canvas = document.createElement('canvas')
    const renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: false, preserveDrawingBuffer: true })
    renderer.setClearColor(0x000000, 0)
    canvas.addEventListener('webglcontextlost', () => {
      shared = null
      for (const m of materials.values()) m.dispose()
      materials.clear()
    })
    const scene = new THREE.Scene()
    const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0, 1)
    const mesh = new THREE.Mesh(new THREE.PlaneGeometry(2, 2))
    mesh.frustumCulled = false
    scene.add(mesh)
    shared = { renderer, scene, camera, mesh }
    return shared
  } catch {
    unavailable = true
    return null
  }
}

function zeroes(): THREE.Vector3[] {
  return Array.from({ length: MAX_LOAD_POINTS }, () => new THREE.Vector3())
}

function materialFor(recipe: Recipe): THREE.ShaderMaterial {
  const key = JSON.stringify(recipe)
  const hit = materials.get(key)
  if (hit) {
    materials.delete(key)
    materials.set(key, hit)
    return hit
  }
  const m = new THREE.ShaderMaterial({
    vertexShader: CONCEPT_VERTEX_SHADER,
    fragmentShader: conceptFragmentShader(recipe),
    transparent: true,
    depthTest: false,
    depthWrite: false,
    uniforms: {
      uRes: { value: new THREE.Vector2(1, 1) },
      uTarget: { value: new THREE.Vector3() },
      uRight: { value: new THREE.Vector3() },
      uUp: { value: new THREE.Vector3() },
      uFwd: { value: new THREE.Vector3() },
      uHalf: { value: 1 },
      uBMin: { value: new THREE.Vector3() },
      uBMax: { value: new THREE.Vector3() },
      uPov: { value: 0 },
      uInk: { value: new THREE.Vector3() },
      uFace: { value: new THREE.Vector3() },
      uWarn: { value: new THREE.Vector3() },
      uReveal: { value: 1 },
      uLoad: { value: zeroes() },
      uNLoad: { value: 0 },
      uSup: { value: zeroes() },
      uNSup: { value: 0 },
      uSigma: { value: 0 },
      uZmin: { value: 0 },
      uCell: { value: 1 },
    },
  })
  materials.set(key, m)
  while (materials.size > MATERIAL_CACHE) {
    const oldest = materials.keys().next().value
    if (oldest === undefined) break
    materials.get(oldest)?.dispose()
    materials.delete(oldest)
  }
  return m
}

export interface SketchColors {
  ink: [number, number, number]
  face: [number, number, number]
  warn: [number, number, number]
}

/** Ink = the board's foreground, face = a whisper of it on the background,
 *  warn = the caution colour. Read per draw so a theme change is honoured. */
export function sketchColors(): SketchColors {
  const fg = resolveCssVar('--foreground').color
  const bg = resolveCssVar('--background').color
  const warn = resolveCssVar('--caution').color
  const mix = (a: number, b: number, t: number) => a + (b - a) * t
  return {
    ink: [fg.r, fg.g, fg.b],
    face: [mix(bg.r, fg.r, 0.07), mix(bg.g, fg.g, 0.07), mix(bg.b, fg.b, 0.07)],
    warn: [warn.r, warn.g, warn.b],
  }
}

export interface SketchRequest {
  recipe: Recipe
  bounds: Bounds
  view: OrbitView
  pov: PovCode
  /** 0..1 -- the share of rows printed so far (top to bottom). */
  reveal: number
  loads?: readonly ConceptLoad[]
  supports?: readonly ConceptSupport[]
  sigma: number
  zmin: number
  cell: number
  colors: SketchColors
}

/**
 * Draw one sketch into `target` (a 2D canvas sized in device pixels).
 * Returns the frame used, or null when WebGL is not available -- the card
 * then says so instead of drawing anything.
 */
export function drawSketch(target: HTMLCanvasElement, req: SketchRequest): ViewFrame | null {
  const s = getShared()
  const ctx = target.getContext('2d')
  if (!s || !ctx) return null
  const w = target.width
  const h = target.height
  if (w === 0 || h === 0) return null
  const frame = viewFrame(req.bounds, req.view, w / h)
  const m = materialFor(req.recipe)
  const u = m.uniforms
  u.uRes.value.set(w, h)
  u.uTarget.value.set(...frame.target)
  u.uRight.value.set(...frame.right)
  u.uUp.value.set(...frame.up)
  u.uFwd.value.set(...frame.fwd)
  u.uHalf.value = frame.half
  u.uBMin.value.set(...req.bounds.min)
  u.uBMax.value.set(...req.bounds.max)
  u.uPov.value = req.pov
  u.uInk.value.set(...req.colors.ink)
  u.uFace.value.set(...req.colors.face)
  u.uWarn.value.set(...req.colors.warn)
  u.uReveal.value = req.reveal
  const loads = req.loads ?? []
  const sups = req.supports ?? []
  ;(u.uLoad.value as THREE.Vector3[]).forEach((v, i) => v.set(...(loads[i]?.at ?? [0, 0, 0])))
  ;(u.uSup.value as THREE.Vector3[]).forEach((v, i) => v.set(...(sups[i]?.at ?? [0, 0, 0])))
  u.uNLoad.value = Math.min(loads.length, MAX_LOAD_POINTS)
  u.uNSup.value = Math.min(sups.length, MAX_LOAD_POINTS)
  u.uSigma.value = req.sigma
  u.uZmin.value = req.zmin
  u.uCell.value = req.cell
  s.mesh.material = m
  s.renderer.setPixelRatio(1)
  s.renderer.setSize(w, h, false)
  s.renderer.render(s.scene, s.camera)
  ctx.clearRect(0, 0, w, h)
  ctx.drawImage(s.renderer.domElement, 0, 0, w, h)
  return frame
}
