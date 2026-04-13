import { useCallback, useEffect, useRef, useMemo } from 'react'
import { Canvas } from '@react-three/fiber'
import { CADGrid } from './CADGrid'
import { GizmoNav } from './GizmoNav'
import { SceneLighting } from './SceneLighting'
import { CameraController } from './CameraController'
import { ReferencePlanes } from './ReferencePlanes'
import { SceneObjects } from './SceneObjects'
import { TransformGizmo } from './TransformGizmo'
import { SelectionOutline } from './SelectionOutline'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import type * as THREE from 'three'

export function CADViewport() {
  const containerRef = useRef<HTMLDivElement>(null)
  const setViewportSize = useSceneStore((s) => s.setViewportSize)
  const setGlRef = useSceneStore((s) => s.setGlRef)
  const setSceneRef = useSceneStore((s) => s.setSceneRef)
  const deselectAll = useSceneStore((s) => s.deselectAll)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) {
        const { width, height } = entry.contentRect
        setViewportSize({ width, height })
      }
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [setViewportSize])

  const handleCreated = useCallback(
    (state: { gl: THREE.WebGLRenderer; scene: THREE.Scene }) => {
      setGlRef(state.gl)
      setSceneRef(state.scene)
    },
    [setGlRef, setSceneRef],
  )

  const handlePointerMissed = useCallback(() => {
    deselectAll()
  }, [deselectAll])

  return (
    <div ref={containerRef} className="absolute inset-0 overflow-hidden">
      <Canvas
        shadows
        dpr={[1, 2]}
        camera={{ fov: 50, near: 0.1, far: 2000, position: [20, 15, 20] }}
        gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
        onCreated={handleCreated}
        onPointerMissed={handlePointerMissed}
      >
        <ViewportBackground />

        <CameraController />
        <SceneLighting />
        <CADGrid />
        <ReferencePlanes />
        <GizmoNav />
        <SceneObjects />
        <TransformGizmo />
        <SelectionOutline />
      </Canvas>

      <ViewportInfo />
    </div>
  )
}

function ViewportBackground() {
  const theme = useThemeStore((s) => s.theme)
  const bg = theme === 'dark' ? '#1e1e2e' : '#eeeef2'
  return <color attach="background" args={[bg]} />
}

function ViewportInfo() {
  const activeTool = useSceneStore((s) => s.activeTool)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const selectedCount = useSceneStore((s) => s.selectedIds.size)

  return (
    <div className="absolute bottom-0 left-0 right-0 pointer-events-none">
      <div className="flex items-center justify-between px-3 py-1 text-[10px] text-muted-foreground bg-background/60 backdrop-blur-sm border-t border-border/50">
        <div className="flex items-center gap-3">
          <span className="uppercase tracking-wider">
            {selectionMode === 'object' ? activeTool : selectionMode}
          </span>
          {selectedCount > 0 && (
            <span>{selectedCount} selected</span>
          )}
        </div>
        <div className="flex items-center gap-3">
          <span>Scroll: Zoom</span>
          <span>MMB: Pan</span>
          <span>LMB: Orbit</span>
        </div>
      </div>
    </div>
  )
}
