import { useEffect, useRef } from 'react'
import { useThree, useFrame } from '@react-three/fiber'
import { OrbitControls } from '@react-three/drei'
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

export function CameraController() {
  const controlsRef = useRef<OrbitControlsRef>(null)
  const animRef = useRef<CameraAnimation | null>(null)
  const { camera } = useThree()

  const pendingPreset = useSceneStore((s) => s.pendingCameraPreset)
  const clearPending = useSceneStore((s) => s.clearPendingCameraPreset)
  const pendingFrameObjectId = useSceneStore((s) => s.pendingFrameObjectId)
  const setPendingFrameObject = useSceneStore((s) => s.setPendingFrameObject)
  const setCameraRef = useSceneStore((s) => s.setCameraRef)

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
      distance = maxDim * 3
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
    <OrbitControls
      ref={controlsRef}
      makeDefault
      enableDamping
      dampingFactor={0.08}
      rotateSpeed={0.8}
      panSpeed={0.8}
      zoomSpeed={1.2}
      minDistance={1}
      maxDistance={500}
      enablePan
      screenSpacePanning
      mouseButtons={{
        LEFT: THREE.MOUSE.ROTATE,
        MIDDLE: THREE.MOUSE.PAN,
        // RIGHT intentionally unbound — reserved for the viewport
        // context menu (CADMesh.onContextMenu). Pan is still
        // available via MIDDLE.
      }}
    />
  )
}
