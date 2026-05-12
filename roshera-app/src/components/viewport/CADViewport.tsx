import { useCallback, useEffect, useRef } from 'react'
import { Canvas } from '@react-three/fiber'
import { Box, Grid3x3, SquareDashed } from 'lucide-react'
import { CADGrid } from './CADGrid'
import { GizmoNav } from './GizmoNav'
import { SceneLighting } from './SceneLighting'
import { CameraController } from './CameraController'
import { Datums } from './Datums'
import { SceneObjects } from './SceneObjects'
import { TransformGizmo } from './TransformGizmo'
import { ExtrudeGizmo } from './ExtrudeGizmo'
import { SelectionOutline } from './SelectionOutline'
import { SubElementHighlight } from './SubElementHighlight'
import { ModifyPreview } from './ModifyPreview'
import { ViewportContextMenu } from './ViewportContextMenu'
import { ExtrudeHoverTooltip } from './ExtrudeHoverTooltip'
import { SketchOverlay } from './SketchOverlay'
import { SketchPanel } from '@/components/panels/SketchPanel'
import { isStandardPlane, useSceneStore, type SketchPlane } from '@/stores/scene-store'
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
      // Section View toggles per-material clipping planes; the renderer
      // honours them only when local clipping is on. Enable globally so
      // a per-CADMesh `material.clippingPlanes` array is respected the
      // moment Section View is turned on, with no canvas re-mount.
      state.gl.localClippingEnabled = true
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
        // Three.js r170+ deprecated PCFSoftShadowMap (the default that
        // `shadows` / `shadows={true}` resolves to) and now silently
        // remaps it to PCFShadowMap with a console warning. Pin to
        // PCFShadowMap explicitly so the warning is gone and the actual
        // shadow algorithm matches what the renderer is using.
        shadows="percentage"
        dpr={[1, 2]}
        gl={{ antialias: true, alpha: true, powerPreference: 'high-performance' }}
        onCreated={handleCreated}
        onPointerMissed={handlePointerMissed}
      >
        <CameraController />
        <SceneLighting />
        <CADGrid />
        <Datums />
        <GizmoNav />
        <SceneObjects />
        <TransformGizmo />
        <SubElementHighlight />
        <ModifyPreview />
        <ExtrudeGizmo />
        <SelectionOutline />
        <SketchOverlay />
      </Canvas>

      <ViewportFrame />
      <ViewportControls />
      <ViewportReadout />
      <SketchCoordReadout />
      <ViewportHints />
      <ModeBanner />
      <SectionViewPanel />
      <ViewportContextMenu />
      <ExtrudeHoverTooltip />
      <CSketchDofHud />
      <SketchPanel />
    </div>
  )
}

/**
 * Live cursor coordinates while in 2D sketch mode. Mounts only when
 * `sketch.active` is true; hidden otherwise so the viewport stays
 * clean for 3D work.
 *
 * Shows two readouts:
 *   * Plane-local (U, V) — what the kernel actually stores. This is
 *     the coordinate space the user is drawing in.
 *   * World (X, Y, Z) — the lifted position. Useful for cross-
 *     referencing with the 3D origin / grid.
 *
 * (U, V) and (X, Y, Z) are derived from the same `sketch.hover`
 * field that the SketchOverlay maintains on every pointer-move; this
 * component only reads, never updates. When the pointer leaves the
 * capture plane `sketch.hover` becomes `null` and the values blank
 * out to dashes instead of stale numbers.
 *
 * The world-space computation mirrors `SketchOverlay.uvToWorld`
 * exactly (`origin + u·u_axis + v·v_axis` for custom planes,
 * standard-plane lift otherwise) so the displayed XYZ agrees with
 * the position the backend will record.
 */
function SketchCoordReadout() {
  const active = useSceneStore((s) => s.sketch.active)
  const plane = useSceneStore((s) => s.sketch.plane)
  const hover = useSceneStore((s) => s.sketch.hover)

  if (!active) return null

  const planeLabel = isStandardPlane(plane) ? plane.toUpperCase() : 'FACE'

  const fmt = (n: number) => {
    // Three decimals matches the dimension annotations rendered by
    // SketchOverlay's `<Html>` labels. Pad to a fixed width so the
    // readout doesn't jitter as the cursor crosses zero.
    const s = n.toFixed(3)
    return s.startsWith('-') ? s : ` ${s}`
  }

  const worldXYZ = hover ? sketchUvToWorldXYZ(hover, plane) : null

  return (
    <div className="absolute bottom-3 left-1/2 -translate-x-1/2 pointer-events-none cad-panel cad-readout px-2.5 py-1.5 text-[10px] uppercase tracking-wider min-w-[180px]">
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">Plane</span>
        <span className="text-foreground">{planeLabel}</span>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">U</span>
        <span className="text-foreground tabular-nums font-mono">
          {hover ? fmt(hover[0]) : '   ---'}
        </span>
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">V</span>
        <span className="text-foreground tabular-nums font-mono">
          {hover ? fmt(hover[1]) : '   ---'}
        </span>
      </div>
      <div className="mt-1 pt-1 border-t border-border/40">
        <div className="flex items-center justify-between gap-3">
          <span className="text-muted-foreground/70">X</span>
          <span className="text-muted-foreground tabular-nums font-mono">
            {worldXYZ ? fmt(worldXYZ[0]) : '   ---'}
          </span>
        </div>
        <div className="flex items-center justify-between gap-3">
          <span className="text-muted-foreground/70">Y</span>
          <span className="text-muted-foreground tabular-nums font-mono">
            {worldXYZ ? fmt(worldXYZ[1]) : '   ---'}
          </span>
        </div>
        <div className="flex items-center justify-between gap-3">
          <span className="text-muted-foreground/70">Z</span>
          <span className="text-muted-foreground tabular-nums font-mono">
            {worldXYZ ? fmt(worldXYZ[2]) : '   ---'}
          </span>
        </div>
      </div>
    </div>
  )
}

/**
 * Floating HUD that reports the constraint-solver verdict for the
 * **active csketch** (`csketch.activeId`). Hidden when there is no
 * active csketch or no `lastDofReport` has been pulled yet.
 *
 * The DOF report is refreshed in `refreshCSketch` (in the scene
 * store), which is called after every mutation that touches
 * entities or constraints, so the HUD stays in lock-step with the
 * user's edits without needing its own polling loop.
 *
 * Three pieces of state are surfaced:
 *
 *   1. **Structural verdict** — `fully_constrained` / `under_…` /
 *      `over_…`. Determines the pill colour (positive / neutral /
 *      negative) and the headline number (free dofs vs excess).
 *   2. **Conflicts** — `redundant`/`conflicts` from slice H
 *      (`/api/csketch/{id}/dof` carries them under `#[serde(default)]`
 *      so old payloads silently degrade to empty arrays).
 *   3. **Skipped** — count of constraints the analyser had to drop
 *      because they touched unsupported entity kinds (rectangles
 *      pending C-2 etc.). Surfaced so the user knows the verdict
 *      is partial.
 *
 * Slice D-3a, added 2026-05-12.
 */
function CSketchDofHud() {
  const activeId = useSceneStore((s) => s.csketch.activeId)
  const report = useSceneStore((s) => s.csketch.lastDofReport)

  if (!activeId || !report) return null

  // Headline pill: derived from the structural status. Colours match
  // the rest of the viewport HUD vocabulary — foreground/background
  // accents drive the eye toward the over-constrained case which is
  // the only one that needs the user to act.
  let statusLabel: string
  let statusTone: 'positive' | 'neutral' | 'negative'
  switch (report.status.kind) {
    case 'fully_constrained':
      statusLabel = 'FULLY CONSTRAINED'
      statusTone = 'positive'
      break
    case 'under_constrained':
      statusLabel = `FREE DOFs: ${report.status.dofs}`
      statusTone = 'neutral'
      break
    case 'over_constrained':
      statusLabel = `EXCESS: ${report.status.conflicting_constraints}`
      statusTone = 'negative'
      break
  }
  const statusClass =
    statusTone === 'positive'
      ? 'text-emerald-400'
      : statusTone === 'negative'
        ? 'text-rose-400'
        : 'text-amber-300'

  const conflictCount = report.conflicts.length
  const redundantCount = report.redundant.length
  const skippedCount = report.constraints_skipped

  return (
    <div className="absolute top-3 right-3 pointer-events-none cad-panel cad-readout px-2.5 py-1.5 text-[10px] uppercase tracking-wider min-w-[180px]">
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">DOF</span>
        <span className={`${statusClass} font-semibold`}>{statusLabel}</span>
      </div>
      {(conflictCount > 0 || redundantCount > 0 || skippedCount > 0) && (
        <div className="mt-1 pt-1 border-t border-border/40 space-y-0.5">
          {conflictCount > 0 && (
            <div className="flex items-center justify-between gap-3">
              <span className="text-muted-foreground">Conflicts</span>
              <span className="text-rose-400 font-semibold tabular-nums">
                {conflictCount}
              </span>
            </div>
          )}
          {redundantCount > 0 && (
            <div className="flex items-center justify-between gap-3">
              <span className="text-muted-foreground">Redundant</span>
              <span className="text-amber-300 font-semibold tabular-nums">
                {redundantCount}
              </span>
            </div>
          )}
          {skippedCount > 0 && (
            <div className="flex items-center justify-between gap-3">
              <span className="text-muted-foreground/70">Skipped</span>
              <span className="text-muted-foreground tabular-nums">
                {skippedCount}
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * Lift plane-local (u, v) to world (x, y, z). Mirrors
 * `SketchOverlay.uvToWorld` byte-for-byte; duplicated here to avoid
 * pulling the entire R3F-side module into the HUD layer.
 */
function sketchUvToWorldXYZ(
  uv: [number, number],
  plane: SketchPlane,
): [number, number, number] {
  const [u, v] = uv
  if (isStandardPlane(plane)) {
    switch (plane) {
      case 'xy': return [u, v, 0]
      case 'xz': return [u, 0, v]
      case 'yz': return [0, u, v]
    }
  }
  return [
    plane.origin[0] + plane.u_axis[0] * u + plane.v_axis[0] * v,
    plane.origin[1] + plane.u_axis[1] * u + plane.v_axis[1] * v,
    plane.origin[2] + plane.u_axis[2] * u + plane.v_axis[2] * v,
  ]
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
 * Floating panel surfaced when Section View is enabled. Lets the user
 * pick an axis (X/Y/Z), drag the offset slider, and flip which side
 * is culled. Hidden when section view is off so the viewport stays
 * uncluttered.
 *
 * Pure UI over `sectionView` store state — CADMesh materials read the
 * same state and slot a `THREE.Plane` into their `clippingPlanes`
 * array. No backend round-trip.
 */
function SectionViewPanel() {
  const sectionView = useSceneStore((s) => s.sectionView)
  const setSectionView = useSceneStore((s) => s.setSectionView)
  const toggleSectionView = useSceneStore((s) => s.toggleSectionView)

  if (!sectionView.enabled) return null

  return (
    <div className="absolute top-3 right-3 cad-panel px-3 py-2 flex flex-col gap-2 text-[10px] uppercase tracking-wider min-w-[200px]">
      <div className="flex items-center justify-between gap-3">
        <span className="text-foreground font-semibold">Section View</span>
        <button
          type="button"
          onClick={toggleSectionView}
          className="px-2 py-0.5 border border-border/60 hover:border-border text-muted-foreground hover:text-foreground transition-colors"
          title="Disable section view"
        >
          Off
        </button>
      </div>
      <div className="flex items-center gap-1">
        {(['x', 'y', 'z'] as const).map((axis) => (
          <button
            key={axis}
            type="button"
            onClick={() => setSectionView({ axis })}
            className={[
              'flex-1 px-2 py-1 border transition-colors',
              sectionView.axis === axis
                ? 'border-border text-foreground bg-accent'
                : 'border-border/40 text-muted-foreground hover:text-foreground',
            ].join(' ')}
          >
            {axis.toUpperCase()}
          </button>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground w-12">Offset</span>
        <input
          type="range"
          min={-50}
          max={50}
          step={0.5}
          value={sectionView.offset}
          onChange={(e) => setSectionView({ offset: Number(e.target.value) })}
          className="flex-1"
        />
        <span className="tabular-nums text-foreground w-10 text-right">
          {sectionView.offset.toFixed(1)}
        </span>
      </div>
      <button
        type="button"
        onClick={() => setSectionView({ flipped: !sectionView.flipped })}
        className={[
          'px-2 py-1 border transition-colors',
          sectionView.flipped
            ? 'border-border text-foreground bg-accent'
            : 'border-border/40 text-muted-foreground hover:text-foreground',
        ].join(' ')}
      >
        Flip side
      </button>
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
