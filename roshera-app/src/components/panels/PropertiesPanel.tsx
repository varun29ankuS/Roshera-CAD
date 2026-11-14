import { useEffect, useState, type ReactNode } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { useSceneStore } from '@/stores/scene-store'
import { cn } from '@/lib/utils'
import { verifyPart } from '@/lib/dfm'
import {
  diameterList,
  fetchFace,
  fetchPartProperties,
  formatArea,
  formatMass,
  formatPoint3,
  formatSize,
  formatVolume,
  type FaceReport,
  type PartProperties,
} from '@/lib/part-properties'
import {
  Box,
  Eye,
  EyeOff,
  Lock,
  Unlock,
  Ruler,
} from '@/components/ui/icons'

/** A titled section that folds; the panel scrolls as a whole. */
function Section({ title, defaultOpen = true, children }: { title: string; defaultOpen?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen)
  return (
    <section className="border-t border-border/70 first:border-t-0">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="cad-focus flex w-full items-center gap-1 px-3 py-1.5 text-left text-[10.5px] font-medium uppercase tracking-[0.08em] text-muted-foreground hover:text-foreground"
      >
        {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        {title}
      </button>
      {open && <div className="space-y-1 px-3 pb-2.5">{children}</div>}
    </section>
  )
}

/** One spec-sheet row: the label, then the value. */
function Row({ label, value, mono = true }: { label: string; value: ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-3 text-[11.5px]">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      {/* Wraps rather than cuts: a value cut to "4× …" is a wrong number. */}
      <span className={cn('min-w-0 break-words text-right text-foreground', mono && 'font-mono tabular-nums')}>{value}</span>
    </div>
  )
}

const MATERIAL_KEYS = ['metalness', 'roughness', 'opacity'] as const

/**
 * Properties of the selected part (Varun 2026-11-14: "unscrollable ... and
 * it needs a lot more features"): what it is and whether the kernel calls it
 * sound; its size; its mass, material, volume, area and centre of mass; what
 * it is made of (faces, edges, vertices, every cylinder's diameter); the
 * picked face; its position; and, folded, its appearance and display.
 * Every number is the kernel's own (lib/part-properties).
 */
export function PropertiesPanel() {
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const objects = useSceneStore((s) => s.objects)
  const updateObject = useSceneStore((s) => s.updateObject)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const subSelections = useSceneStore((s) => s.subElementSelections)

  const id = selectedIds.size === 1 ? Array.from(selectedIds)[0] : null
  const obj = id ? objects.get(id) : undefined
  const solidId = obj?.analyticalGeometry?.solidId
  const mesh = obj?.mesh

  const [props, setProps] = useState<{ solidId: number; data: PartProperties } | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  const [verifying, setVerifying] = useState(false)

  // Read again when the part changes (an edit hands it a new mesh).
  useEffect(() => {
    if (solidId === undefined) return
    let live = true
    fetchPartProperties(solidId)
      .then((data) => {
        if (!live) return
        setProps({ solidId, data })
        setLoadError(null)
      })
      .catch((e: unknown) => {
        if (live) setLoadError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      live = false
    }
  }, [solidId, mesh, reload])

  // The picked face, when picking faces.
  const pickedFace = selectionMode === 'face' && subSelections.length === 1 ? subSelections[0].index : null
  const [face, setFace] = useState<FaceReport | null>(null)
  useEffect(() => {
    if (pickedFace === null) return
    let live = true
    fetchFace(pickedFace)
      .then((f) => {
        if (live) setFace(f)
      })
      .catch(() => {
        if (live) setFace(null)
      })
    return () => {
      live = false
    }
  }, [pickedFace])

  if (selectedIds.size > 1) {
    return <div className="p-3 text-[11.5px] text-muted-foreground">{selectedIds.size} parts selected</div>
  }
  if (!id || !obj) return null

  const data = props && props.solidId === solidId ? props.data : null
  const p = data?.part
  const verdict = data?.perception

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Header: what it is, and whether the kernel stands behind it. */}
      <div className="flex items-center gap-2 border-b border-border px-3 py-2">
        <Box size={16} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12.5px] font-semibold">{obj.name}</div>
          <div className="text-[10.5px] text-muted-foreground">Part{solidId !== undefined ? ` · solid ${solidId}` : ''}</div>
        </div>
        <button
          type="button"
          onClick={() => updateObject(id, { visible: !obj.visible })}
          className="cad-icon-btn h-6 w-6"
          title={obj.visible ? 'Hide' : 'Show'}
          aria-label={obj.visible ? 'Hide part' : 'Show part'}
        >
          {obj.visible ? <Eye size={12} /> : <EyeOff size={12} />}
        </button>
        <button
          type="button"
          onClick={() => updateObject(id, { locked: !obj.locked })}
          className="cad-icon-btn h-6 w-6"
          title={obj.locked ? 'Unlock' : 'Lock'}
          aria-label={obj.locked ? 'Unlock part' : 'Lock part'}
        >
          {obj.locked ? <Lock size={12} /> : <Unlock size={12} />}
        </button>
      </div>
      {verdict && (
        <div
          className={cn(
            'flex items-center gap-2 border-b border-border px-3 py-1.5 text-[11px]',
            verdict.sound ? 'text-emerald-700 dark:text-emerald-400' : 'text-amber-700 dark:text-amber-400',
          )}
          title={verdict.verdict}
        >
          <span className="font-medium">{verdict.sound ? '✓ Sound' : verdict.verified ? '✗ Not sound' : 'Not verified'}</span>
          <span className="min-w-0 flex-1 truncate text-muted-foreground">
            {verdict.sound ? 'closed, manifold, no self-intersection' : verdict.status}
          </span>
          {!verdict.sound && solidId !== undefined && (
            <button
              type="button"
              disabled={verifying}
              onClick={async () => {
                setVerifying(true)
                await verifyPart(solidId).catch(() => false)
                setVerifying(false)
                setReload((n) => n + 1)
              }}
              className="cad-focus rounded border border-border px-1.5 py-0.5 text-foreground hover:bg-accent"
            >
              {verifying ? 'Verifying…' : 'Verify'}
            </button>
          )}
        </div>
      )}

      {/* The body scrolls; the header stays. */}
      <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-width:thin]">
        {loadError && !data && (
          <p className="px-3 py-2 text-[11px] text-red-700 dark:text-red-300">Could not read this part ({loadError}).</p>
        )}
        {p && (
          <>
            <Section title="Size">
              <Row label="W × D × H" value={formatSize(p.dimensions_world)} />
              <Row label="Centre" value={formatPoint3(p.center_world)} />
              <Row label="From" value={formatPoint3(p.world_bbox_min)} />
              <Row label="To" value={formatPoint3(p.world_bbox_max)} />
            </Section>
            <Section title="Mass">
              <Row label="Mass" value={data?.mass ? formatMass(data.mass.mass) : '—'} />
              <Row label="Material" value={`${p.material.name} · ${p.material.density.toLocaleString()} kg/m³`} mono={false} />
              <Row label="Volume" value={formatVolume(p.volume)} />
              <Row label="Surface area" value={formatArea(p.surface_area)} />
              {data?.mass && <Row label="Centre of mass" value={formatPoint3(data.mass.center_of_mass)} />}
              {data?.mass && (
                <Row label="Principal moments" value={`${data.mass.principal_moments.map((m) => Math.round(m)).join(' · ')} kg·mm²`} />
              )}
            </Section>
            <Section title="Contents">
              <Row label="Faces · edges · vertices" value={`${p.topology.face_count} · ${p.topology.edge_count} · ${p.topology.vertex_count}`} />
              <Row label="Diameters" value={diameterList(data?.diameters ?? [])} />
            </Section>
          </>
        )}
        {pickedFace !== null && face && face.id === pickedFace && (
          <Section title={`Face ${face.id}`}>
            <Row label="Surface" value={face.surface_type} mono={false} />
            <Row label="Area" value={formatArea(face.area)} />
            <Row label="Touches faces" value={face.neighbour_face_ids.join(', ') || 'none'} />
          </Section>
        )}
        {selectionMode !== 'object' && subSelections.length > 1 && (
          <Section title={`Picked (${selectionMode})`}>
            <Row label="Count" value={subSelections.length} />
            <Row label="Ids" value={subSelections.map((s) => s.index).join(', ')} />
          </Section>
        )}
        <Section title="Position">
          <TransformEditor objectId={id} />
        </Section>
        <Section title="Appearance" defaultOpen={false}>
          <ColorPicker value={obj.material.color} onChange={(c) => updateObject(id, { material: { ...obj.material, color: c } })} />
          {MATERIAL_KEYS.map((k) => (
            <div key={k}>
              <div className="mb-0.5 flex justify-between text-[11px] text-muted-foreground">
                <span className="capitalize">{k}</span>
                <span className="font-mono">{obj.material[k].toFixed(2)}</span>
              </div>
              <input
                type="range"
                min={0}
                max={1}
                step={0.01}
                value={obj.material[k]}
                aria-label={k}
                onChange={(e) => updateObject(id, { material: { ...obj.material, [k]: Number(e.target.value) } })}
                className="h-1 w-full accent-primary"
              />
            </div>
          ))}
        </Section>
        <Section title="Display" defaultOpen={false}>
          <EdgeDisplayControls />
        </Section>
      </div>
    </div>
  )
}

/**
 * Inline material-color picker. Click the swatch to slide out a grid
 * of engineering-palette presets directly under the Material header —
 * no native browser dialog. Hex text input stays editable so any
 * arbitrary `#rrggbb` value can still be entered by typing.
 *
 * Palette is curated for CAD workflows: neutral greys for parts, a
 * row of metal tones (steel, aluminum, copper, brass, gold, iron),
 * and saturated primaries for callouts / variants.
 */
const COLOR_PRESETS: ReadonlyArray<readonly [string, string]> = [
  ['#1a1a1a', 'Charcoal'],
  ['#444444', 'Graphite'],
  ['#7a7a7a', 'Mid grey'],
  ['#b0b0b0', 'Light grey'],
  ['#e8e8e8', 'Off white'],
  ['#ffffff', 'White'],
  ['#5b6770', 'Steel'],
  ['#a8aab0', 'Aluminum'],
  ['#b87333', 'Copper'],
  ['#d4af37', 'Brass'],
  ['#cdb56a', 'Gold'],
  ['#3a3a3a', 'Iron'],
  ['#c0392b', 'Red'],
  ['#e67e22', 'Orange'],
  ['#f1c40f', 'Yellow'],
  ['#27ae60', 'Green'],
  ['#2980b9', 'Blue'],
  ['#8e44ad', 'Purple'],
]

function ColorPicker({
  value,
  onChange,
}: {
  value: string
  onChange: (color: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState(value)

  // Keep the hex input in sync if the value updates externally
  // (e.g. another part of the UI changes the material).
  if (draft !== value && !open) {
    // Read during render is fine here — we only want to mirror prop
    // changes back into local draft when the popover is closed.
    setDraft(value)
  }

  function commit(v: string) {
    if (/^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(v)) {
      onChange(v)
    }
  }

  return (
    <div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="w-5 h-5 rounded border border-border cursor-pointer p-0 transition-transform hover:scale-110"
          style={{ backgroundColor: value }}
          title={open ? 'Close picker' : 'Pick color'}
          aria-label="Toggle material color picker"
          aria-expanded={open}
        />
        <input
          type="text"
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value)
            commit(e.target.value.trim())
          }}
          className="flex-1 bg-background/50 rounded px-1.5 py-0.5 text-[11px] font-mono outline-none text-foreground"
          spellCheck={false}
        />
      </div>

      {/* Slide-out palette. `grid-rows-[Nfr]` + max-height transition
          keeps the panel inside the Properties scroll area instead of
          floating above other UI as a popover would. */}
      <div
        className={`grid transition-[grid-template-rows,opacity,margin] duration-150 ease-out ${
          open
            ? 'grid-rows-[1fr] opacity-100 mt-1.5'
            : 'grid-rows-[0fr] opacity-0 mt-0'
        }`}
      >
        <div className="overflow-hidden">
          <div className="rounded border border-border bg-background/40 p-1.5">
            <div className="grid grid-cols-6 gap-1">
              {COLOR_PRESETS.map(([hex, name]) => {
                const selected = hex.toLowerCase() === value.toLowerCase()
                return (
                  <button
                    key={hex}
                    type="button"
                    onClick={() => onChange(hex)}
                    className={`w-full aspect-square rounded border cursor-pointer transition-transform hover:scale-110 ${
                      selected
                        ? 'border-foreground ring-1 ring-foreground'
                        : 'border-border/60'
                    }`}
                    style={{ backgroundColor: hex }}
                    title={`${name} (${hex})`}
                    aria-label={name}
                    aria-pressed={selected}
                  />
                )
              })}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function EdgeDisplayControls() {
  const edgeSettings = useSceneStore((s) => s.edgeSettings)
  const setEdgeSettings = useSceneStore((s) => s.setEdgeSettings)

  return (
    <div>
      <p className="text-[11px] text-muted-foreground uppercase tracking-wider mb-1.5">
        Edge Display
      </p>
      <div className="space-y-1.5">
        <div data-role="pair" className="flex items-center justify-between">
          <span className="text-[11px] text-muted-foreground">Show Edges</span>
          <button
            onClick={() => setEdgeSettings({ visible: !edgeSettings.visible })}
            className={`w-7 h-4 rounded-full transition-colors ${edgeSettings.visible ? 'bg-primary' : 'bg-muted'}`}
          >
            <div className={`w-3 h-3 rounded-full bg-white transition-transform ${edgeSettings.visible ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>
        <div>
          <div data-role="pair" className="flex justify-between text-[11px] text-muted-foreground mb-0.5">
            <span>Threshold</span>
            <span className="font-mono">{edgeSettings.threshold}°</span>
          </div>
          <input
            type="range"
            min={1}
            max={45}
            value={edgeSettings.threshold}
            onChange={(e) => setEdgeSettings({ threshold: Number(e.target.value) })}
            className="w-full h-1 accent-primary"
          />
        </div>
        <div>
          <div data-role="pair" className="flex justify-between text-[11px] text-muted-foreground mb-0.5">
            <span>Line Width</span>
            <span className="font-mono">{edgeSettings.lineWidth.toFixed(1)}</span>
          </div>
          <input
            type="range"
            min={0.5}
            max={5}
            step={0.5}
            value={edgeSettings.lineWidth}
            onChange={(e) => setEdgeSettings({ lineWidth: Number(e.target.value) })}
            className="w-full h-1 accent-primary"
          />
        </div>
      </div>
    </div>
  )
}

function TransformEditor({ objectId }: { objectId: string }) {
  const objects = useSceneStore((s) => s.objects)
  const updateObject = useSceneStore((s) => s.updateObject)
  const obj = objects.get(objectId)
  if (!obj) return null

  function handleChange(
    field: 'position' | 'rotation',
    axis: 0 | 1 | 2,
    value: string,
  ) {
    const num = parseFloat(value)
    if (isNaN(num)) return
    const current = objects.get(objectId)
    if (!current) return
    const vec = [...current[field]] as [number, number, number]
    vec[axis] = num
    updateObject(objectId, { [field]: vec })
  }

  return (
    <div>
      <p className="text-[11px] text-muted-foreground uppercase tracking-wider mb-1.5">
        <Ruler size={10} className="inline mr-1" />
        Transform (editable)
      </p>
      <div className="space-y-2">
        <TransformRow
          label="Pos"
          values={obj.position}
          onChange={(axis, val) => handleChange('position', axis, val)}
        />
        <TransformRow
          label="Rot"
          values={obj.rotation.map((r) => (r * 180) / Math.PI) as [number, number, number]}
          onChange={(axis, val) => {
            const rad = (parseFloat(val) * Math.PI) / 180
            if (isNaN(rad)) return
            const current = [...obj.rotation] as [number, number, number]
            current[axis] = rad
            updateObject(objectId, { rotation: current })
          }}
          suffix="°"
        />
        {/* Display only: the kernel has no scale operation, so a scale
            typed here would resize the part on screen with nothing behind
            it (and every later gizmo drag of it would be refused). */}
        <TransformRow
          label="Scl"
          values={obj.scale}
        />
      </div>
    </div>
  )
}

function TransformRow({
  label,
  values,
  onChange,
  suffix = '',
}: {
  label: string
  values: [number, number, number]
  /** Absent: the row is display-only. */
  onChange?: (axis: 0 | 1 | 2, value: string) => void
  suffix?: string
}) {
  const colors = ['text-red-400', 'text-green-400', 'text-blue-400']
  const axes = ['X', 'Y', 'Z'] as const

  return (
    <div className="flex items-center gap-1">
      <span className="text-[11px] text-muted-foreground w-5 shrink-0">{label}</span>
      {([0, 1, 2] as const).map((i) => (
        // `min-w-0` is required so the flex item can shrink below the
        // browser's default `<input type="number">` intrinsic width
        // (~150px in Chromium). Without it, three axis cells each
        // refuse to shrink and the row blows past the panel column.
        <div key={i} className="flex-1 min-w-0 flex items-center gap-0.5 bg-background/50 rounded px-1 py-0.5">
          <span className={`text-[11px] ${colors[i]} shrink-0`}>{axes[i]}</span>
          <input
            type="number"
            step={label === 'Scl' ? 0.1 : 1}
            value={values[i].toFixed(label === 'Scl' ? 2 : 1)}
            readOnly={!onChange}
            onChange={onChange ? (e) => onChange(i, e.target.value) : undefined}
            className="w-full min-w-0 bg-transparent text-[11px] font-mono outline-none text-foreground"
          />
          {suffix && <span className="text-[11px] text-muted-foreground shrink-0">{suffix}</span>}
        </div>
      ))}
    </div>
  )
}
