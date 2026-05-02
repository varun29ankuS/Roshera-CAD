import { useEffect, useRef } from 'react'
import { useThree, useFrame } from '@react-three/fiber'
import {
  OrbitControls,
  OrthographicCamera,
  PerspectiveCamera,
} from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
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
// Orthographic frustum extends well into both half-spaces so geometry
// behind the look-at point is never clipped. Width/height are derived
// from `zoom`; near/far are world-space distances along the view axis.
const ORTHO_NEAR = -2000
const ORTHO_FAR = 2000
// Initial ortho zoom chosen so the default isometric camera frames a
// roughly 60-unit world span on a 1080p viewport. Scroll-wheel
// (OrbitControls) and the viewport's auto-frame logic adjust it.
const INITIAL_ORTHO_ZOOM = 30

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
  const sketchActive = useSceneStore((s) => s.sketch.active)
  const projection = useSceneStore((s) => s.cameraProjection)
  // Suppresses orbit-rotation while a viewport gizmo (e.g. face-pull
  // arrow) owns the pointer. OrbitControls listens at the canvas DOM
  // level so R3F's `e.stopPropagation()` doesn't reach it; gating
  // `enableRotate` here is the only reliable suppression.
  const gizmoDragging = useSceneStore((s) => s.gizmoDragging)

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
        near={PERSPECTIVE_NEAR}
        far={PERSPECTIVE_FAR}
        position={INITIAL_POSITION}
      />
      <OrthographicCamera
        ref={orthoRef}
        makeDefault={projection === 'orthographic'}
        near={ORTHO_NEAR}
        far={ORTHO_FAR}
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
        minDistance={1}
        maxDistance={500}
        // Sketch mode owns left-click for point placement; rotate must
        // not fire on the same gesture. Pan + zoom stay enabled via
        // middle button + scroll so the user can still navigate while
        // drawing on a plane.
        enableRotate={!sketchActive && !gizmoDragging}
        enablePan
        screenSpacePanning
        mouseButtons={{
          // Sketch mode owns left-click; OrbitControls treats `undefined`
          // as "no binding" so the gesture passes through cleanly. Same
          // pass-through applies while a gizmo is mid-drag — the
          // gesture must not also rotate the camera.
          LEFT:
            sketchActive || gizmoDragging
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
