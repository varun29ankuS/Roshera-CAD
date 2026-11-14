import { useEffect, useLayoutEffect, useMemo, useRef } from 'react'
import { useSketchBench } from '@/stores/sketch-bench-store'
import { keepCameraPose, keptCameraPose } from '@/lib/camera-memory'
import { FIT_MARGIN, fitDistance } from '@/lib/camera-fit'
import { CAMERA_PRESETS, FRAME_ALL, FRAME_FIT } from '@/stores/scene-store'
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
const INITIAL_POSITION: [number, number, number] = [20, -20, 15]
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
  // The canvas renders on demand (CADViewport `frameloop="demand"`): the
  // fly-to animation below runs inside `useFrame`, which only fires on a
  // requested frame, so starting one must request the first frame and each
  // animated frame must request the next.
  const invalidate = useThree((s) => s.invalidate)
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
  // Sketch mode: on ENTRY, snap the camera normal-to the sketch plane so the
  // 2D profile reads face-on instead of as a foreshortened diagonal.
  const benchOpen = useSketchBench((s) => s.sketchId !== null)
  const benchPlane = useSketchBench((s) => s.plane)
  // The old sketch mode or the sketch workbench: the same camera / grid rules.
  const sketchActive = useSceneStore((s) => s.sketch.active) || benchOpen
  const legacyPlane = useSceneStore((s) => s.sketch.plane)
  const sketchPlane = benchOpen ? benchPlane : legacyPlane
  const wasSketchingRef = useRef(false)

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

  // Return to the view the user left. Switching workspace unmounts this
  // viewport; a fresh camera starts at INITIAL_POSITION, 30 mm from the
  // origin -- inside any real part -- and nothing re-frames it, so the
  // pose is kept on the way out and put back on the way in (both
  // cameras, so a projection toggle afterwards stays seamless).
  useLayoutEffect(() => {
    const controls = controlsRef.current
    const pose = keptCameraPose()
    if (controls && pose) {
      for (const cam of [perspRef.current, orthoRef.current]) {
        if (!cam) continue
        cam.position.set(...pose.position)
        cam.up.set(...pose.up)
        if (cam instanceof THREE.OrthographicCamera) cam.zoom = pose.zoom
        cam.lookAt(...pose.target)
        cam.updateProjectionMatrix()
      }
      ;(controls.target as THREE.Vector3).set(...pose.target)
      controls.update()
      invalidate()
    }
    const persp = perspRef.current
    const ortho = orthoRef.current
    return () => {
      const live = useSceneStore.getState().cameraProjection === 'orthographic' ? ortho : persp
      const t = controls?.target as THREE.Vector3 | undefined
      if (!live || !t) return
      keepCameraPose({
        position: [live.position.x, live.position.y, live.position.z],
        target: [t.x, t.y, t.z],
        up: [live.up.x, live.up.y, live.up.z],
        zoom: ortho?.zoom ?? 1,
      })
    }
  }, [invalidate])

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
    invalidate()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projection])

  useEffect(() => {
    setCameraRef(camera)
    return () => setCameraRef(null)
  }, [camera, setCameraRef])

  useEffect(() => {
    if (!pendingPreset || !controlsRef.current) return

    const controls = controlsRef.current

    // A named view (Front, Top, Iso...) is a viewing DIRECTION. Its stored
    // position sits 30 mm from the origin, which puts the camera inside
    // anything bigger than a few centimetres, so its distance and target come
    // from the model: the same fit the sketch look-at uses. Any other pose
    // (the agent bridge's set_camera) is exact and is applied as written, as
    // is every view of an empty scene.
    const presetTarget = new THREE.Vector3(...pendingPreset.target)
    const presetPos = new THREE.Vector3(...pendingPreset.position)
    const isNamedView = Object.values(CAMERA_PRESETS).includes(pendingPreset)
    let endTarget = presetTarget
    let endPosition = presetPos
    if (isNamedView && sceneBounds.radius > 1e-6) {
      const dir = new THREE.Vector3().subVectors(presetPos, presetTarget).normalize()
      endTarget = sceneBounds.center.clone()
      let distance = sceneBounds.radius * 2.5
      const persp = perspRef.current
      if (persp && (camera as THREE.PerspectiveCamera).isPerspectiveCamera) {
        distance = fitDistance(sceneBounds.radius, persp.fov, persp.aspect)
      }
      endPosition = endTarget.clone().addScaledVector(dir, distance)
    }

    animRef.current = {
      startPosition: camera.position.clone(),
      startTarget: controls.target.clone(),
      endPosition,
      endTarget,
      endUp: new THREE.Vector3(...pendingPreset.up),
      progress: 0,
    }
    invalidate()

    clearPending()
  }, [pendingPreset, camera, clearPending, invalidate, sceneBounds])

  // Sketch-plane look-at. Every CAD sketcher orients the view normal-to the
  // active sketch plane on entry so the 2D profile is face-on and EDITABLE —
  // without it an XZ/YZ sketch on an iso camera reads as an unreadable
  // foreshortened diagonal. Fires only on the false→true transition (the
  // `wasSketchingRef` guard) so adding geometry mid-sketch doesn't re-snap.
  useEffect(() => {
    const entering = sketchActive && !wasSketchingRef.current
    wasSketchingRef.current = sketchActive
    if (!entering || !controlsRef.current) return

    // View offset (camera sits along this from the target) + screen-up, per
    // plane, matching CAD convention (Top / Front / Right).
    let offset: THREE.Vector3
    let up: THREE.Vector3
    if (sketchPlane === 'xy') {
      offset = new THREE.Vector3(0, 0, 1)
      up = new THREE.Vector3(0, 1, 0)
    } else if (sketchPlane === 'xz') {
      offset = new THREE.Vector3(0, -1, 0)
      up = new THREE.Vector3(0, 0, 1)
    } else if (sketchPlane === 'yz') {
      offset = new THREE.Vector3(1, 0, 0)
      up = new THREE.Vector3(0, 0, 1)
    } else {
      // Custom plane: face its normal (u × v), keep its v-axis upright.
      const u = new THREE.Vector3(...sketchPlane.u_axis)
      const v = new THREE.Vector3(...sketchPlane.v_axis)
      offset = new THREE.Vector3().crossVectors(u, v).normalize()
      up = v.clone().normalize()
      if (offset.lengthSq() < 1e-9) offset = new THREE.Vector3(0, 0, 1)
    }

    const center = sceneBounds.center.clone()
    let distance = sceneBounds.radius * 2.5
    const persp = perspRef.current
    if (persp && (camera as THREE.PerspectiveCamera).isPerspectiveCamera) {
      distance = fitDistance(sceneBounds.radius, persp.fov, persp.aspect)
    }

    animRef.current = {
      startPosition: camera.position.clone(),
      startTarget: controlsRef.current.target.clone(),
      endPosition: center.clone().addScaledVector(offset, distance),
      endTarget: center,
      endUp: up,
      progress: 0,
    }
    invalidate()
  }, [sketchActive, sketchPlane, sceneBounds, camera, invalidate])

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
    // FRAME_ALL / FRAME_FIT frame the union of every object (a document
    // opening / a part created); any other id frames that one object.
    const all = useSceneStore.getState().objects
    const objs =
      pendingFrameObjectId === FRAME_ALL || pendingFrameObjectId === FRAME_FIT
        ? [...all.values()]
        : [all.get(pendingFrameObjectId)].filter((o) => o !== undefined)

    const min = new THREE.Vector3(Infinity, Infinity, Infinity)
    const max = new THREE.Vector3(-Infinity, -Infinity, -Infinity)
    const v = new THREE.Vector3()
    for (const obj of objs) {
      const vertices = obj.mesh.vertices
      const worldMatrix = new THREE.Matrix4().compose(
        new THREE.Vector3(...obj.position),
        new THREE.Quaternion().setFromEuler(
          new THREE.Euler(obj.rotation[0], obj.rotation[1], obj.rotation[2], 'XYZ'),
        ),
        new THREE.Vector3(...obj.scale),
      )
      for (let i = 0; i < vertices.length; i += 3) {
        v.set(vertices[i], vertices[i + 1], vertices[i + 2]).applyMatrix4(worldMatrix)
        min.min(v)
        max.max(v)
      }
    }
    if (!Number.isFinite(min.x) || !Number.isFinite(max.x)) {
      // Nothing with geometry to frame.
      setPendingFrameObject(null)
      return
    }

    const center = new THREE.Vector3().addVectors(min, max).multiplyScalar(0.5)
    const size = new THREE.Vector3().subVectors(max, min)
    const maxDim = Math.max(size.x, size.y, size.z, 1e-3)
    // The bounding sphere: fits the part from any direction.
    const radius = Math.max(size.length() / 2, 1e-3)

    let distance: number
    const persp = camera as THREE.PerspectiveCamera
    if (persp.isPerspectiveCamera) {
      // The part fills about half the view, on the tighter of height and
      // width (lib/camera-fit).
      distance = fitDistance(radius, persp.fov, persp.aspect)
    } else {
      // Orthographic: distance doesn't affect apparent size — `zoom`
      // does. Pick a viewing distance large enough to keep the camera
      // outside the object's bounding sphere (so OrbitControls doesn't
      // place the focus inside the geometry), then snap the zoom so
      // the object fits the viewport with the same 1.5× padding the
      // perspective branch uses.
      distance = maxDim * 3
      const ortho = camera as THREE.OrthographicCamera
      const { width, height } = useSceneStore.getState().viewportSize
      if (ortho.isOrthographicCamera && height > 0) {
        // Imperative Three.js camera mutation inside an effect (not React
        // state) — required by the r3f/three API. The react-hooks
        // immutability rule can't distinguish an external mutable object
        // from frozen render state, so it false-positives here.
        // eslint-disable-next-line react-hooks/immutability
        // The same margin as perspective: the sphere's diameter takes half
        // the tighter of the view's height and width.
        ortho.zoom = Math.min(height, width > 0 ? width : height) / (2 * radius * FIT_MARGIN)
        ortho.updateProjectionMatrix()
      }
    }

    // Framing the whole scene happens when a document opens (first load or
    // a switch): a fresh view, so it looks from the iso direction, Z up. The
    // camera's current direction means nothing then, and can be garbage: at
    // boot the server's default document is framed and then the app switches
    // to the last-used one while that first animation is still in flight.
    // Framing one part keeps the user's direction (iso if the camera sits on
    // its target).
    const iso = new THREE.Vector3(1, -1, 0.75).normalize()
    const dir =
      pendingFrameObjectId === FRAME_ALL
        ? iso
        : new THREE.Vector3().subVectors(camera.position, controls.target).normalize()
    if (!Number.isFinite(dir.x) || dir.lengthSq() < 1e-6) dir.copy(iso)
    const endPosition = center.clone().addScaledVector(dir, distance)

    animRef.current = {
      startPosition: camera.position.clone(),
      startTarget: controls.target.clone(),
      endPosition,
      endTarget: center,
      endUp: pendingFrameObjectId === FRAME_ALL ? new THREE.Vector3(0, 0, 1) : camera.up.clone(),
      progress: 0,
    }
    invalidate()
    setPendingFrameObject(null)
  }, [pendingFrameObjectId, camera, setPendingFrameObject, invalidate])

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
    } else {
      invalidate()
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
