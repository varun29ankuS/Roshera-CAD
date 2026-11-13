import { cn } from '@/lib/utils'
import { FRAME_FIT, useSceneStore } from '@/stores/scene-store'

const VIEWS: { preset: string; label: string; key: string }[] = [
  { preset: 'isometric', label: 'Iso', key: '0' },
  { preset: 'front', label: 'Front', key: '1' },
  { preset: 'top', label: 'Top', key: '7' },
  { preset: 'right', label: 'Right', key: '3' },
]

/**
 * The viewport's view bar (the approved mockup): how the part is drawn --
 * shaded with its edges, shaded only, a section cut -- then which side it
 * is seen from, and Fit. One compact row over the top of the viewport.
 */
export function ViewBar() {
  const edges = useSceneStore((s) => s.edgeSettings.visible)
  const setEdgeSettings = useSceneStore((s) => s.setEdgeSettings)
  const section = useSceneStore((s) => s.sectionView.enabled)
  const toggleSection = useSceneStore((s) => s.toggleSectionView)
  const preset = useSceneStore((s) => s.cameraPreset)
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const frame = useSceneStore((s) => s.setPendingFrameObject)

  const btn = (on: boolean) =>
    cn(
      'cad-focus h-6 rounded px-2 text-[11.5px] transition-colors',
      on ? 'bg-muted font-medium text-foreground' : 'text-muted-foreground hover:bg-accent hover:text-foreground',
    )

  return (
    <div
      role="toolbar"
      aria-label="View"
      className="pointer-events-auto flex items-center gap-px rounded-lg border border-border bg-card/95 p-0.5 shadow-sm"
    >
      <button type="button" className={btn(edges)} aria-pressed={edges} onClick={() => setEdgeSettings({ visible: true })}>
        Shaded + edges
      </button>
      <button type="button" className={btn(!edges)} aria-pressed={!edges} onClick={() => setEdgeSettings({ visible: false })}>
        Shaded
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
    </div>
  )
}
