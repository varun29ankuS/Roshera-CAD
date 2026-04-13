import { useEffect, useRef } from 'react'
import { useThree, useFrame } from '@react-three/fiber'
import { OrbitControls } from '@react-three/drei'
import { useSceneStore } from '@/stores/scene-store'
import * as THREE from 'three'
import type { OrbitControls as OrbitControlsImpl } from 'three-stdlib'

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
  const controlsRef = useRef<OrbitControlsImpl>(null)
  const animRef = useRef<CameraAnimation | null>(null)
  const { camera } = useThree()

  const pendingPreset = useSceneStore((s) => s.pendingCameraPreset)
  const clearPending = useSceneStore((s) => s.clearPendingCameraPreset)
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
        RIGHT: THREE.MOUSE.PAN,
      }}
    />
  )
}
