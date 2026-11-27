import { cn } from '@/lib/utils'
import { displayModeOf, displayModeSettings, type DisplayMode } from '@/lib/display-mode'
import { FRAME_FIT, useSceneStore } from '@/stores/scene-store'

const VIEWS: { preset: string; label: string; key: string }[] = [
  { preset: 'isometric', label: 'Iso', key: '0' },
  { preset: 'front', label: 'Front', key: '1' },
  { preset: 'top', label: 'Top', key: '7' },
  { preset: 'right', label: 'Right', key: '3' },
]

/**
 * The viewport's view bar (the approved mockup): how the part is drawn --
 * shaded with its edges, shaded only, sketched (the concept cards' pencil
 * look on the real mesh), a section cut -- then which side it
 * is seen from, and Fit -- then perspective/ortho, grid, labels and GD&T.
 * One compact row over the top of the viewport.
 */
export function ViewBar() {
  const edges = useSceneStore((s) => s.edgeSettings.visible)
  const setEdgeSettings = useSceneStore((s) => s.setEdgeSettings)
  const sketch = useSceneStore((s) => s.sketchStyle)
  const setSketchStyle = useSceneStore((s) => s.setSketchStyle)
  const mode = displayModeOf({ edges, sketch })
  const setMode = (m: DisplayMode) => {
    const next = displayModeSettings(m)
    setEdgeSettings({ visible: next.edges })
    setSketchStyle(next.sketch)
  }
  const section = useSceneStore((s) => s.sectionView.enabled)
  const toggleSection = useSceneStore((s) => s.toggleSectionView)
  const preset = useSceneStore((s) => s.cameraPreset)
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const frame = useSceneStore((s) => s.setPendingFrameObject)
  const ortho = useSceneStore((s) => s.cameraProjection) === 'orthographic'
  const toggleProjection = useSceneStore((s) => s.toggleCameraProjection)
  const grid = useSceneStore((s) => s.gridSettings.visible)
  const setGridSettings = useSceneStore((s) => s.setGridSettings)
  const labels = useSceneStore((s) => s.labelsVisible)
  const toggleLabels = useSceneStore((s) => s.toggleLabelsVisible)
  const gdt = useSceneStore((s) => s.gdtVisible)
  const toggleGdt = useSceneStore((s) => s.toggleGdtVisible)

  const btn = (on: boolean) =>
    cn(
      'cad-focus h-6 shrink-0 whitespace-nowrap rounded px-2 text-[11.5px] transition-colors',
      on ? 'bg-muted font-medium text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground',
    )

  return (
    <div
      role="toolbar"
      aria-label="View"
      // In a narrow window the bar scrolls sideways inside its slot rather than
      // wrapping a label or sliding under the panels beside it.
      className="pointer-events-auto flex max-w-full items-center gap-px overflow-x-auto rounded-lg border border-border bg-card/95 p-0.5 shadow-sm [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      <button type="button" className={btn(mode === 'edges')} aria-pressed={mode === 'edges'} onClick={() => setMode('edges')}>
        Shaded + edges
      </button>
      <button type="button" className={btn(mode === 'shaded')} aria-pressed={mode === 'shaded'} onClick={() => setMode('shaded')}>
        Shaded
      </button>
      <button
        type="button"
        className={btn(mode === 'sketch')}
        aria-pressed={mode === 'sketch'}
        onClick={() => setMode('sketch')}
        title="Draw the parts as a pencil sketch (display only)"
      >
        Sketch
      </button>
      <button type="button" className={btn(section)} aria-pressed={section} onClick={toggleSection} title="Cut the part with a plane">
        Section
      </button>
      <span aria-hidden className="mx-1 h-4 w-px bg-border" />
      {VIEWS.map((v) => (
        <button
          key={v.preset}
          type="button"
          className={btn(preset === v.preset)}
          aria-pressed={preset === v.preset}
          onClick={() => setCameraPreset(v.preset)}
          title={`${v.label} view`}
        >
          {v.label}
        </button>
      ))}
      <span aria-hidden className="mx-1 h-4 w-px bg-border" />
      <button type="button" className={btn(false)} onClick={() => frame(FRAME_FIT)} title="Fit the part to the view">
        Fit
      </button>
      <span aria-hidden className="mx-1 h-4 w-px bg-border" />
      {/* What else the view shows (these sat in a corner the Blackboard now
          covers). */}
      <button type="button" className={btn(ortho)} aria-pressed={ortho} onClick={toggleProjection} title="Orthographic: parallel lines stay parallel">
        Ortho
      </button>
      <button type="button" className={btn(grid)} aria-pressed={grid} onClick={() => setGridSettings({ visible: !grid })} title="Show the grid">
        Grid
      </button>
      <button type="button" className={btn(labels)} aria-pressed={labels} onClick={toggleLabels} title="Show part labels">
        Labels
      </button>
      <button type="button" className={btn(gdt)} aria-pressed={gdt} onClick={toggleGdt} title="Show GD&T annotations">
        GD&amp;T
      </button>
    </div>
  )
}
