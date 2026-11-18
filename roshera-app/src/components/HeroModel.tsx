import { useEffect, useMemo, useRef, useState } from 'react'
import { Canvas, useFrame } from '@react-three/fiber'
import { Edges } from '@react-three/drei'
import * as THREE from 'three'
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js'

/**
 * The sign-in hero: a real Roshera part, live in 3D, turning slowly.
 *
 * `src` is an STL the kernel tessellated from a certified STEP part (see
 * /showcase/manifest.json); it ships with the app because before sign-in the
 * server answers nothing. Kernel Z-up is shown Y-up, centred and scaled to
 * fit; under reduced motion it stands still at a three-quarter view.
 */
export function HeroModel({ src }: { src: string }) {
  const [geometry, setGeometry] = useState<THREE.BufferGeometry | null>(null)

  useEffect(() => {
    let cancelled = false
    new STLLoader().load(
      src,
      (g) => {
        if (cancelled) return
        g.computeVertexNormals()
        g.computeBoundingBox()
        const box = g.boundingBox ?? new THREE.Box3()
        const centre = box.getCenter(new THREE.Vector3())
        g.translate(-centre.x, -centre.y, -centre.z)
        const size = box.getSize(new THREE.Vector3())
        const s = 2.6 / Math.max(size.x, size.y, size.z, 1e-9)
        g.scale(s, s, s)
        setGeometry(g)
      },
      undefined,
      () => {
        if (!cancelled) setGeometry(null)
      },
    )
    return () => {
      cancelled = true
    }
  }, [src])

  return (
    <Canvas
      camera={{ position: [2.6, 1.7, 2.9], fov: 35 }}
      dpr={[1, 2]}
      gl={{ antialias: true, alpha: true }}
      aria-hidden="true"
    >
      <ambientLight intensity={0.55} />
      <directionalLight position={[4, 6, 5]} intensity={1.4} />
      <directionalLight position={[-5, 2, -3]} intensity={0.5} />
      <directionalLight position={[0, -4, 2]} intensity={0.25} />
      {geometry && <Turntable geometry={geometry} />}
    </Canvas>
  )
}

function Turntable({ geometry }: { geometry: THREE.BufferGeometry }) {
  const group = useRef<THREE.Group>(null)
  const still = useMemo(
    () => typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches,
    [],
  )
  useFrame((_, dt) => {
    if (!still && group.current) group.current.rotation.y += dt * 0.18
  })
  return (
    <group ref={group} rotation={[0, -0.6, 0]}>
      {/* Kernel Z-up shown Y-up. */}
      <mesh geometry={geometry} rotation={[-Math.PI / 2, 0, 0]}>
        <meshStandardMaterial color="#c3c9d1" metalness={0.35} roughness={0.42} />
        {/* Only real creases: the display mesh is simplified, and a low
            threshold drew its facet seams as scratches. */}
        <Edges threshold={50} color="#2b3138" />
      </mesh>
    </group>
  )
}
