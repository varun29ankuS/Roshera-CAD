import { useCallback, useEffect, useRef } from 'react'
import { Canvas } from '@react-three/fiber'
import { Box, Grid3x3, SquareDashed } from 'lucide-react'
import { CADGrid } from './CADGrid'
import { GizmoNav } from './GizmoNav'
import { SceneLighting } from './SceneLighting'
import { CameraController } from './CameraController'
import { ReferencePlanes } from './ReferencePlanes'
import { SceneObjects } from './SceneObjects'
import { TransformGizmo } from './TransformGizmo'
import { ExtrudeGizmo } from './ExtrudeGizmo'
import { SelectionOutline } from './SelectionOutline'
import { SubElementHighlight } from './SubElementHighlight'
import { ViewportContextMenu } from './ViewportContextMenu'
import { SketchOverlay } from './SketchOverlay'
import { SketchPanel } from '@/components/panels/SketchPanel'
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

  // Suppress the browser's native context menu over the entire viewport
  // so neither empty-space right-clicks nor the synthesized contextmenu
  // events from a CADMesh hit ever surface "Save image as / Inspect".
  // The CADMesh onContextMenu handler opens our own menu; clicks on
  // empty viewport space simply do nothing.
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault()
  }, [])

  return (
    <div
      ref={containerRef}
      onContextMenu={handleContextMenu}
      className="absolute inset-0 overflow-hidden bg-[var(--cad-viewport-bg)]"
    >
      <Canvas
        shadows
        dpr={[1, 2]}
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
        <SubElementHighlight />
        <ExtrudeGizmo />
        <SelectionOutline />
        <SketchOverlay />
      </Canvas>

      <ViewportFrame />
      <ViewportControls />
      <ViewportReadout />
      <ViewportHints />
      <ModeBanner />
      <ViewportContextMenu />
      <SketchPanel />
    </div>
  )
}

/**
 * Top-center banner that becomes visible whenever the user is in a
 * sub-element selection mode. Hidden in object mode so the viewport stays
 * clean. Tells the user exactly what they will pick on the next click.
 */
function ModeBanner() {
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)

  if (selectionMode === 'object') return null

  const labels: Record<'face' | 'edge' | 'vertex', { title: string; hint: string }> = {
    face:   { title: 'FACE MODE',   hint: 'Click a face to select · Press 1 to exit' },
    edge:   { title: 'EDGE MODE',   hint: 'Click an edge to select · Press 1 to exit' },
    vertex: { title: 'VERTEX MODE', hint: 'Click a vertex to select · Press 1 to exit' },
  }
  const { title, hint } = labels[selectionMode]

  return (
    <div className="absolute top-3 left-1/2 -translate-x-1/2 pointer-events-auto cad-panel px-4 py-2 flex items-center gap-3 text-[11px] uppercase tracking-wider">
      <span className="w-2 h-2 rounded-full bg-foreground animate-pulse" />
      <span className="text-foreground font-semibold">{title}</span>
      <span className="text-muted-foreground">{hint}</span>
      <button
        type="button"
        onClick={() => setSelectionMode('object')}
        className="ml-2 px-2 py-0.5 border border-border/60 hover:border-border text-muted-foreground hover:text-foreground transition-colors"
        title="Exit to object mode"
      >
        Exit
      </button>
    </div>
  )
}

/**
 * Floating viewport toggles — grid on/off and projection (perspective
 * vs orthographic). Sits just above the readout in the bottom-right
 * corner. Engineers expect orthographic for measurement-accurate
 * views; designers prefer perspective for spatial reasoning. The
 * toggle preserves camera position/orientation across the swap (see
 * `CameraController`'s projection effect for the FOV ↔ zoom mapping).
 */
function ViewportControls() {
  const gridVisible = useSceneStore((s) => s.gridSettings.visible)
  const setGridSettings = useSceneStore((s) => s.setGridSettings)
  const projection = useSceneStore((s) => s.cameraProjection)
  const toggleProjection = useSceneStore((s) => s.toggleCameraProjection)

  const toggleGrid = useCallback(() => {
    setGridSettings({ visible: !gridVisible })
  }, [gridVisible, setGridSettings])

  const isOrtho = projection === 'orthographic'
  const ProjectionIcon = isOrtho ? SquareDashed : Box

  return (
    <div className="absolute bottom-[68px] right-3 flex items-center gap-1">
      <button
        type="button"
        onClick={toggleProjection}
        title={
          isOrtho
            ? 'Orthographic — click for perspective'
            : 'Perspective — click for orthographic'
        }
        aria-pressed={isOrtho}
        className={[
          'cad-panel w-7 h-7 flex items-center justify-center transition-colors',
          isOrtho
            ? 'text-foreground border-border'
            : 'text-muted-foreground/70 border-border/60 hover:text-foreground',
        ].join(' ')}
      >
        <ProjectionIcon className="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        onClick={toggleGrid}
        title={gridVisible ? 'Hide grid' : 'Show grid'}
        aria-pressed={gridVisible}
        className={[
          'cad-panel w-7 h-7 flex items-center justify-center transition-colors',
          gridVisible
            ? 'text-foreground border-border'
            : 'text-muted-foreground/70 border-border/60 hover:text-foreground',
        ].join(' ')}
      >
        <Grid3x3 className="w-3.5 h-3.5" />
      </button>
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
