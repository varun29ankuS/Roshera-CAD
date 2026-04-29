import { useCallback, useEffect, useRef } from 'react'
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
    <div
      ref={containerRef}
      className="absolute inset-0 overflow-hidden bg-[var(--cad-viewport-bg)]"
    >
      <Canvas
        shadows
        dpr={[1, 2]}
        camera={{ fov: 50, near: 0.1, far: 2000, position: [20, 15, 20] }}
        gl={{ antialias: true, alpha: true, powerPreference: 'high-performance' }}
        onCreated={handleCreated}
        onPointerMissed={handlePointerMissed}
      >
        <CameraController />
        <SceneLighting />
        <CADGrid />
        <ReferencePlanes />
        <GizmoNav />
        <SceneObjects />
        <TransformGizmo />
        <SelectionOutline />
      </Canvas>

      <ViewportFrame />
      <ViewportReadout />
      <ViewportHints />
    </div>
  )
}

/**
 * Corner-tick frame surrounding the viewport — four L-shaped marks in the
 * blueprint border color. Purely decorative; pointer-events disabled so it
 * never intercepts orbit/pan/zoom.
 */
function ViewportFrame() {
  const tick = 'w-3 h-3 absolute pointer-events-none border-[var(--border)]'
  return (
    <div className="absolute inset-2 pointer-events-none">
      <span className={`${tick} top-0 left-0 border-t border-l`} />
      <span className={`${tick} top-0 right-0 border-t border-r`} />
      <span className={`${tick} bottom-0 left-0 border-b border-l`} />
      <span className={`${tick} bottom-0 right-0 border-b border-r`} />
    </div>
  )
}

/**
 * Top-right blueprint readout — current tool, selection mode, selected
 * count. Mirrors the LABEL · VALUE pattern used elsewhere in the UI.
 */
function ViewportReadout() {
  const activeTool = useSceneStore((s) => s.activeTool)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const selectedCount = useSceneStore((s) => s.selectedIds.size)

  return (
    <div className="absolute bottom-3 right-3 pointer-events-none cad-panel cad-readout px-2.5 py-1.5 text-[10px] uppercase tracking-wider min-w-[140px]">
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">Tool</span>
        <span className="text-foreground">{activeTool}</span>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">Mode</span>
        <span className="text-foreground">{selectionMode}</span>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">Selected</span>
        <span className="text-foreground tabular-nums">{selectedCount}</span>
      </div>
    </div>
  )
}

/**
 * Bottom-edge mouse-control hints, blueprint styled.
 */
function ViewportHints() {
  return (
    <div className="absolute bottom-3 left-3 pointer-events-none flex items-center gap-4 px-2.5 py-1 text-[10px] uppercase tracking-wider font-mono text-muted-foreground/80">
      <span>LMB · Orbit</span>
      <span>MMB · Pan</span>
      <span>Scroll · Zoom</span>
    </div>
  )
}
