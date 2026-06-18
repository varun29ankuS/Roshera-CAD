import { useEffect, useMemo, useRef } from 'react'
import { useThree, useFrame } from '@react-three/fiber'
import {
  OrbitControls,
  OrthographicCamera,
  PerspectiveCamera,
} from '@react-three/drei'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import * as THREE from 'three'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type OrbitControlsRef = any

const ANIMATION_DURATION = 0.6
const EASE_OUT_CUBIC = (t: number) => 1 - Math.pow(1 - t, 3)

interface CameraAnimation {
  startPosition: THREE.Vector3
  startTarget: THREE.Vector3
  endPosition: THREE.Vector3
  endTarget: THREE.Vector3
  endUp: THREE.Vector3
  progress: number
}

// Initial camera placement — must match the previous Canvas-default
// camera so existing scenes look identical on first paint.
const INITIAL_POSITION: [number, number, number] = [20, 15, 20]
const INITIAL_TARGET: [number, number, number] = [0, 0, 0]
const PERSPECTIVE_FOV = 50
const PERSPECTIVE_NEAR = 0.1
const PERSPECTIVE_FAR = 2000
// Orthographic frustum extends well into both half-spaces so geometry behind
// the look-at point is never clipped. Width/height are derived from `zoom`;
// near/far are world-space distances along the view axis, sized per-frame from
// the scene extent (see `farProp` / the useFrame clip-plane refinement).
// Initial ortho zoom chosen so the default isometric camera frames a
// roughly 60-unit world span on a 1080p viewport. Scroll-wheel
// (OrbitControls) and the viewport's auto-frame logic adjust it.
const INITIAL_ORTHO_ZOOM = 30

// Scene-adaptive camera limits. The viewport must frame anything from a 1 mm
// feature to a 10 m+ assembly, so the dolly range and clip planes are DERIVED
// from the scene's world extent rather than hardcoded. These floors keep the
// behaviour for small/empty scenes identical to before.
const MIN_SCENE_RADIUS = 30 // empty/tiny scene → behaves like the old defaults
const MAX_DOLLY_FLOOR = 500 // never tighter than the previous maxDistance

/** World-space bounding sphere (center + radius) of every object's mesh,
 *  transformed by its placement — the basis for all scene-adaptive framing. */
function computeSceneBounds(
  objects: Map<string, CADObject>,
): { center: THREE.Vector3; radius: number } {
  const min = new THREE.Vector3(Infinity, Infinity, Infinity)
  const max = new THREE.Vector3(-Infinity, -Infinity, -Infinity)
  const v = new THREE.Vector3()
  const m = new THREE.Matrix4()
  let any = false
  for (const obj of objects.values()) {
    const verts = obj.mesh?.vertices
    if (!verts || verts.length === 0) continue
    m.compose(
      new THREE.Vector3(obj.position[0], obj.position[1], obj.position[2]),
      new THREE.Quaternion().setFromEuler(
        new THREE.Euler(obj.rotation[0], obj.rotation[1], obj.rotation[2], 'XYZ'),
      ),
      new THREE.Vector3(obj.scale[0], obj.scale[1], obj.scale[2]),
    )
    for (let i = 0; i < verts.length; i += 3) {
      v.set(verts[i], verts[i + 1], verts[i + 2]).applyMatrix4(m)
      min.min(v)
      max.max(v)
      any = true
    }
  }
  if (!any) return { center: new THREE.Vector3(0, 0, 0), radius: MIN_SCENE_RADIUS }
  const center = new THREE.Vector3().addVectors(min, max).multiplyScalar(0.5)
  const radius = Math.max(new THREE.Vector3().subVectors(max, min).length() * 0.5, 1e-3)
  return { center, radius }
}

export function CameraController() {
  const controlsRef = useRef<OrbitControlsRef>(null)
  const animRef = useRef<CameraAnimation | null>(null)
  const { camera } = useThree()
  const perspRef = useRef<THREE.PerspectiveCamera>(null)
  const orthoRef = useRef<THREE.OrthographicCamera>(null)

  const pendingPreset = useSceneStore((s) => s.pendingCameraPreset)
  const clearPending = useSceneStore((s) => s.clearPendingCameraPreset)
  const pendingFrameObjectId = useSceneStore((s) => s.pendingFrameObjectId)
  const setPendingFrameObject = useSceneStore((s) => s.setPendingFrameObject)
  const setCameraRef = useSceneStore((s) => s.setCameraRef)
  const projection = useSceneStore((s) => s.cameraProjection)
  // Suppresses orbit-rotation while a viewport gizmo (e.g. face-pull
  // arrow) owns the pointer. OrbitControls listens at the canvas DOM
  // level so R3F's `e.stopPropagation()` doesn't reach it; gating
  // `enableRotate` here is the only reliable suppression.
  const gizmoDragging = useSceneStore((s) => s.gizmoDragging)

  // Scene-adaptive framing: recompute the world bounding sphere whenever the
  // object set changes, and derive the dolly range from it. The clip planes are
  // adapted per-frame (below) from the live camera distance so precision holds
  // from millimetre features to 10 m+ assemblies.
  const objects = useSceneStore((s) => s.objects)
  const sceneBounds = useMemo(() => computeSceneBounds(objects), [objects])
  const maxDistance = Math.max(sceneBounds.radius * 8, MAX_DOLLY_FLOOR)
  const minDistance = Math.max(sceneBounds.radius * 1e-4, 0.02)
  // Clip-plane PROPS sized to the scene so a re-render never clips the model
  // before the per-frame refinement (above) tightens `near` for precision.
  const farProp = Math.max(maxDistance * 4, PERSPECTIVE_FAR)
  const nearProp = Math.max(farProp * 1e-5, PERSPECTIVE_NEAR)

  // When the projection toggles, copy the outgoing camera's transform
  // onto the incoming one so the swap is visually seamless. Runs once
  // per projection change (the new camera is the one r3f selects via
  // `makeDefault`; `camera` here is the post-swap default).
  useEffect(() => {
    const persp = perspRef.current
    const ortho = orthoRef.current
    const controls = controlsRef.current
    if (!persp || !ortho || !controls) return
    const target = controls.target as THREE.Vector3
    if (projection === 'orthographic') {
      // Coming from perspective → ortho. Match position/quaternion/up
      // (presets like Top set a non-default up vector); pick a zoom
      // that keeps the on-screen footprint of the focus point
      // unchanged. Solving for zoom such that a unit at distance `d`
      // covers the same screen height: `zoom = viewportH / (2 * d *
      // tan(fov/2))`.
      ortho.position.copy(persp.position)
      ortho.quaternion.copy(persp.quaternion)
      ortho.up.copy(persp.up)
      const distance = persp.position.distanceTo(target)
      const fovRad = (persp.fov * Math.PI) / 180
      const { height } = useSceneStore.getState().viewportSize
      if (height > 0 && distance > 1e-6) {
        ortho.zoom = height / (2 * distance * Math.tan(fovRad / 2))
        ortho.updateProjectionMatrix()
      }
    } else {
      // Coming from ortho → perspective. Match position/quaternion/up;
      // adjust the camera's distance from the target so the apparent
      // size of the focus point matches. Same identity solved for
      // distance: `d = viewportH / (2 * zoom * tan(fov/2))`.
      persp.position.copy(ortho.position)
      persp.quaternion.copy(ortho.quaternion)
      persp.up.copy(ortho.up)
      const fovRad = (persp.fov * Math.PI) / 180
      const { height } = useSceneStore.getState().viewportSize
      if (height > 0 && ortho.zoom > 1e-6) {
        const desiredDist = height / (2 * ortho.zoom * Math.tan(fovRad / 2))
        const dir = new THREE.Vector3()
          .subVectors(persp.position, target)
          .normalize()
        if (Number.isFinite(dir.x) && dir.lengthSq() > 1e-6) {
          persp.position.copy(target).addScaledVector(dir, desiredDist)
        }
      }
    }
    controls.update()
  }, [projection])

  useEffect(() => {
    setCameraRef(camera)
    return () => setCameraRef(null)
  }, [camera, setCameraRef])

  useEffect(() => {
    if (!pendingPreset || !controlsRef.current) return

    const controls = controlsRef.current

    animRef.current = {
      startPosition: camera.position.clone(),
      startTarget: controls.target.clone(),
      endPosition: new THREE.Vector3(...pendingPreset.position),
      endTarget: new THREE.Vector3(...pendingPreset.target),
      endUp: new THREE.Vector3(...pendingPreset.up),
      progress: 0,
    }

    clearPending()
  }, [pendingPreset, camera, clearPending])

  // Auto-frame newly-created objects. ws-bridge sets this whenever a
  // brand-new object id appears in the scene (ObjectCreated, or a
  // GeometryUpdate(Tessellated) that introduces an unseen id). We
  // compute the world-space AABB from the raw vertex buffer + the
  // object's transform, place the camera along its current viewing
  // direction at a distance derived from the perspective fov (or 3×
  // the bounding radius for orthographic), and animate over the same
  // 0.6 s ease-out as preset switches.
  useEffect(() => {
    if (!pendingFrameObjectId || !controlsRef.current) return
    const controls = controlsRef.current
    const obj = useSceneStore.getState().objects.get(pendingFrameObjectId)
    if (!obj) {
      setPendingFrameObject(null)
      return
    }
    const vertices = obj.mesh.vertices
    if (vertices.length === 0) {
      setPendingFrameObject(null)
      return
    }

    const worldMatrix = new THREE.Matrix4().compose(
      new THREE.Vector3(...obj.position),
      new THREE.Quaternion().setFromEuler(
        new THREE.Euler(obj.rotation[0], obj.rotation[1], obj.rotation[2], 'XYZ'),
      ),
      new THREE.Vector3(...obj.scale),
    )

    const min = new THREE.Vector3(Infinity, Infinity, Infinity)
    const max = new THREE.Vector3(-Infinity, -Infinity, -Infinity)
    const v = new THREE.Vector3()
    for (let i = 0; i < vertices.length; i += 3) {
      v.set(vertices[i], vertices[i + 1], vertices[i + 2]).applyMatrix4(worldMatrix)
      min.min(v)
      max.max(v)
    }

    const center = new THREE.Vector3().addVectors(min, max).multiplyScalar(0.5)
    const size = new THREE.Vector3().subVectors(max, min)
    const maxDim = Math.max(size.x, size.y, size.z, 1e-3)

    let distance: number
    const persp = camera as THREE.PerspectiveCamera
    if (persp.isPerspectiveCamera) {
      const fov = (persp.fov * Math.PI) / 180
      // Distance s.t. an object of half-size `maxDim/2` fits with 1.5×
      // padding inside the vertical fov. Using maxDim (not size.y) so
      // the framing is robust against tall/wide aspect ratios.
      distance = (maxDim / 2 / Math.tan(fov / 2)) * 1.5
    } else {
      // Orthographic: distance doesn't affect apparent size — `zoom`
      // does. Pick a viewing distance large enough to keep the camera
      // outside the object's bounding sphere (so OrbitControls doesn't
      // place the focus inside the geometry), then snap the zoom so
      // the object fits the viewport with the same 1.5× padding the
      // perspective branch uses.
      distance = maxDim * 3
      const ortho = camera as THREE.OrthographicCamera
      const { height } = useSceneStore.getState().viewportSize
      if (ortho.isOrthographicCamera && height > 0) {
        // Imperative Three.js camera mutation inside an effect (not React
        // state) — required by the r3f/three API. The react-hooks
        // immutability rule can't distinguish an external mutable object
        // from frozen render state, so it false-positives here.
        // eslint-disable-next-line react-hooks/immutability
        ortho.zoom = height / (maxDim * 1.5)
        ortho.updateProjectionMatrix()
      }
    }

    // Preserve the user's current viewing direction. Fall back to a
    // gentle isometric direction if the camera sits exactly on the
    // existing target (no usable direction vector).
    const dir = new THREE.Vector3()
      .subVectors(camera.position, controls.target)
      .normalize()
    if (!Number.isFinite(dir.x) || dir.lengthSq() < 1e-6) {
      dir.set(1, 0.7, 1).normalize()
    }
    const endPosition = center.clone().addScaledVector(dir, distance)

    animRef.current = {
      startPosition: camera.position.clone(),
      startTarget: controls.target.clone(),
      endPosition,
      endTarget: center,
      endUp: camera.up.clone(),
      progress: 0,
    }
    setPendingFrameObject(null)
  }, [pendingFrameObjectId, camera, setPendingFrameObject])

  useFrame((_, delta) => {
    // Scene-adaptive clip planes, every frame. far must reach past the whole
    // scene from wherever the camera sits; near is pulled as close as depth
    // precision allows for the current distance, so a 1 mm detail and a 10 m
    // assembly are both crisp without z-fighting. Runs regardless of animation.
    const controls = controlsRef.current
    if (controls) {
      const d = camera.position.distanceTo(controls.target as THREE.Vector3)
      const far = Math.max((d + sceneBounds.radius) * 4, PERSPECTIVE_FAR)
      const near = Math.max(far * 1e-4, d * 1e-3, 0.02)
      const persp = perspRef.current
      if (persp && (persp.near !== near || persp.far !== far)) {
        persp.near = near
        persp.far = far
        persp.updateProjectionMatrix()
      }
      const ortho = orthoRef.current
      if (ortho && (ortho.near !== -far || ortho.far !== far)) {
        ortho.near = -far
        ortho.far = far
        ortho.updateProjectionMatrix()
      }
    }

    const anim = animRef.current
    if (!anim || !controlsRef.current) return

    anim.progress += delta / ANIMATION_DURATION
    const t = Math.min(anim.progress, 1)
    const eased = EASE_OUT_CUBIC(t)

    camera.position.lerpVectors(anim.startPosition, anim.endPosition, eased)
    controlsRef.current.target.lerpVectors(
      anim.startTarget,
      anim.endTarget,
      eased,
    )
    camera.up.copy(anim.endUp)
    controlsRef.current.update()

    if (t >= 1) {
      animRef.current = null
    }
  })

  return (
    <>
      {/*
        Both cameras are mounted simultaneously; only one is marked
        `makeDefault` based on the store's `cameraProjection`. The
        `useEffect` above mirrors transforms across the swap so the
        view doesn't jump. OrbitControls reads `useThree().camera` on
        each render and follows the new default automatically.
      */}
      <PerspectiveCamera
        ref={perspRef}
        makeDefault={projection === 'perspective'}
        fov={PERSPECTIVE_FOV}
        near={nearProp}
        far={farProp}
        position={INITIAL_POSITION}
      />
      <OrthographicCamera
        ref={orthoRef}
        makeDefault={projection === 'orthographic'}
        near={-farProp}
        far={farProp}
        zoom={INITIAL_ORTHO_ZOOM}
        position={INITIAL_POSITION}
      />
      <OrbitControls
        ref={controlsRef}
        makeDefault
        target={INITIAL_TARGET}
        enableDamping
        dampingFactor={0.08}
        rotateSpeed={0.8}
        panSpeed={0.8}
        zoomSpeed={1.2}
        minDistance={minDistance}
        maxDistance={maxDistance}
        // Sketch mode shares the left button with OrbitControls:
        // click-without-drag places a sketch point (SketchOverlay
        // implements a movement threshold on pointerup), drag-beyond-
        // threshold orbits the camera. The standard parametric-CAD pattern —
        // and the only way to give the user an orbit affordance while
        // sketching without burning a separate modifier key.
        // Gizmo drags still fully suppress orbit (the arrow owns the
        // gesture end-to-end).
        enableRotate={!gizmoDragging}
        enablePan
        screenSpacePanning
        mouseButtons={{
          // While a gizmo is mid-drag the left button must pass through
          // cleanly (OrbitControls treats `undefined` as "no binding")
          // — otherwise the same gesture would also rotate the camera.
          LEFT: gizmoDragging
            ? (undefined as unknown as THREE.MOUSE)
            : THREE.MOUSE.ROTATE,
          MIDDLE: THREE.MOUSE.PAN,
          // RIGHT intentionally unbound — reserved for the viewport
          // context menu (CADMesh.onContextMenu). Pan is still
          // available via MIDDLE.
        }}
      />
    </>
  )
}
