export function SceneLighting() {
  return (
    <>
      <ambientLight intensity={0.6} color="#c8cde8" />
      <directionalLight
        position={[15, 25, 10]}
        intensity={1.0}
        color="#ffffff"
        castShadow
        shadow-mapSize-width={2048}
        shadow-mapSize-height={2048}
        shadow-camera-near={0.5}
        shadow-camera-far={100}
        shadow-camera-left={-30}
        shadow-camera-right={30}
        shadow-camera-top={30}
        shadow-camera-bottom={-30}
        shadow-bias={-0.0005}
      />
      <directionalLight
        position={[-10, 10, -15]}
        intensity={0.4}
        color="#b0c4de"
      />
      <hemisphereLight
        args={['#7090c0', '#1e1e2e', 0.4]}
      />
    </>
  )
}
