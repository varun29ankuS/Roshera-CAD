import { useSceneStore } from '@/stores/scene-store'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import { Box, Eye, EyeOff, Lock, Unlock, Ruler } from 'lucide-react'

export function PropertiesPanel() {
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const objects = useSceneStore((s) => s.objects)
  const updateObject = useSceneStore((s) => s.updateObject)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const subSelections = useSceneStore((s) => s.subElementSelections)

  if (selectedIds.size === 0) {
    return (
      <div className="w-56 border-l border-border bg-card/80 backdrop-blur-sm flex flex-col">
        <div className="cad-panel-header">Properties</div>
        <div className="flex-1 flex items-center justify-center p-4">
          <p className="text-xs text-muted-foreground text-center">
            Select an object to view properties
          </p>
        </div>
      </div>
    )
  }

  if (selectedIds.size > 1) {
    return (
      <div className="w-56 border-l border-border bg-card/80 backdrop-blur-sm flex flex-col">
        <div className="cad-panel-header">Properties</div>
        <div className="p-3">
          <p className="text-xs text-muted-foreground">
            {selectedIds.size} objects selected
          </p>
        </div>
      </div>
    )
  }

  const id = Array.from(selectedIds)[0]
  const obj = objects.get(id)
  if (!obj) return null

  const ag = obj.analyticalGeometry

  return (
    <div className="w-56 border-l border-border bg-card/80 backdrop-blur-sm flex flex-col">
      <div className="cad-panel-header">Properties</div>

      <ScrollArea className="flex-1">
        <div className="p-3 space-y-3">
          {/* Name & Type */}
          <div>
            <div className="flex items-center gap-1.5 mb-1">
              <Box size={12} className="text-primary" />
              <span className="text-xs font-medium truncate">{obj.name}</span>
            </div>
            <span className="text-[10px] text-muted-foreground uppercase tracking-wider">
              {obj.objectType}
            </span>
          </div>

          <Separator />

          {/* Visibility & Lock */}
          <div className="flex items-center gap-2">
            <button
              onClick={() => updateObject(id, { visible: !obj.visible })}
              className="p-1 rounded hover:bg-accent text-muted-foreground"
              title={obj.visible ? 'Hide' : 'Show'}
            >
              {obj.visible ? <Eye size={12} /> : <EyeOff size={12} />}
            </button>
            <button
              onClick={() => updateObject(id, { locked: !obj.locked })}
              className="p-1 rounded hover:bg-accent text-muted-foreground"
              title={obj.locked ? 'Unlock' : 'Lock'}
            >
              {obj.locked ? <Lock size={12} /> : <Unlock size={12} />}
            </button>
          </div>

          <Separator />

          {/* Transform */}
          <div>
            <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
              Position
            </p>
            <div className="grid grid-cols-3 gap-1 text-[10px]">
              <PropValue label="X" value={obj.position[0]} color="text-red-400" />
              <PropValue label="Y" value={obj.position[1]} color="text-green-400" />
              <PropValue label="Z" value={obj.position[2]} color="text-blue-400" />
            </div>
          </div>

          <div>
            <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
              Scale
            </p>
            <div className="grid grid-cols-3 gap-1 text-[10px]">
              <PropValue label="X" value={obj.scale[0]} />
              <PropValue label="Y" value={obj.scale[1]} />
              <PropValue label="Z" value={obj.scale[2]} />
            </div>
          </div>

          {/* Analytical geometry params */}
          {ag && (
            <>
              <Separator />
              <div>
                <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
                  Dimensions
                </p>
                <div className="space-y-0.5">
                  {Object.entries(ag.params).map(([key, val]) => (
                    <div key={key} className="flex justify-between text-[10px]">
                      <span className="text-muted-foreground">{key}</span>
                      <span className="font-mono">{typeof val === 'number' ? val.toFixed(2) : val}</span>
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}

          {/* Material */}
          <Separator />
          <div>
            <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
              Material
            </p>
            <div className="flex items-center gap-2">
              <div
                className="w-4 h-4 rounded border border-border"
                style={{ backgroundColor: obj.material.color }}
              />
              <span className="text-[10px]">{obj.material.color}</span>
            </div>
            <div className="mt-1 space-y-0.5 text-[10px]">
              <div className="flex justify-between">
                <span className="text-muted-foreground">Metalness</span>
                <span className="font-mono">{obj.material.metalness.toFixed(2)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Roughness</span>
                <span className="font-mono">{obj.material.roughness.toFixed(2)}</span>
              </div>
            </div>
          </div>

          {/* Sub-element selection info */}
          {selectionMode !== 'object' && subSelections.length > 0 && (
            <>
              <Separator />
              <div>
                <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
                  Selection ({selectionMode})
                </p>
                <div className="space-y-0.5 text-[10px]">
                  {subSelections.map((sel, i) => (
                    <div key={i} className="text-muted-foreground">
                      {sel.type} #{sel.index}
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}

          {/* Display Settings */}
          <Separator />
          <EdgeDisplayControls />

          {/* Transform — editable */}
          <Separator />
          <TransformEditor objectId={id} />
        </div>
      </ScrollArea>
    </div>
  )
}

function PropValue({
  label,
  value,
  color,
}: {
  label: string
  value: number
  color?: string
}) {
  return (
    <div className="flex flex-col items-center bg-background/50 rounded px-1 py-0.5">
      <span className={`text-[9px] ${color || 'text-muted-foreground'}`}>{label}</span>
      <span className="font-mono text-[10px]">{value.toFixed(1)}</span>
    </div>
  )
}

function EdgeDisplayControls() {
  const edgeSettings = useSceneStore((s) => s.edgeSettings)
  const setEdgeSettings = useSceneStore((s) => s.setEdgeSettings)

  return (
    <div>
      <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
        Edge Display
      </p>
      <div className="space-y-1.5">
        <div className="flex items-center justify-between">
          <span className="text-[10px] text-muted-foreground">Show Edges</span>
          <button
            onClick={() => setEdgeSettings({ visible: !edgeSettings.visible })}
            className={`w-7 h-4 rounded-full transition-colors ${edgeSettings.visible ? 'bg-primary' : 'bg-muted'}`}
          >
            <div className={`w-3 h-3 rounded-full bg-white transition-transform ${edgeSettings.visible ? 'translate-x-3.5' : 'translate-x-0.5'}`} />
          </button>
        </div>
        <div>
          <div className="flex justify-between text-[10px] text-muted-foreground mb-0.5">
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
          <div className="flex justify-between text-[10px] text-muted-foreground mb-0.5">
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
    field: 'position' | 'rotation' | 'scale',
    axis: 0 | 1 | 2,
    value: string,
  ) {
    const num = parseFloat(value)
    if (isNaN(num)) return
    const current = [...obj[field]] as [number, number, number]
    current[axis] = num
    updateObject(objectId, { [field]: current })
  }

  return (
    <div>
      <p className="text-[10px] text-muted-foreground uppercase tracking-wider mb-1.5">
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
        <TransformRow
          label="Scl"
          values={obj.scale}
          onChange={(axis, val) => handleChange('scale', axis, val)}
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
  onChange: (axis: 0 | 1 | 2, value: string) => void
  suffix?: string
}) {
  const colors = ['text-red-400', 'text-green-400', 'text-blue-400']
  const axes = ['X', 'Y', 'Z'] as const

  return (
    <div className="flex items-center gap-1">
      <span className="text-[9px] text-muted-foreground w-5 shrink-0">{label}</span>
      {([0, 1, 2] as const).map((i) => (
        <div key={i} className="flex-1 flex items-center gap-0.5 bg-background/50 rounded px-1 py-0.5">
          <span className={`text-[8px] ${colors[i]}`}>{axes[i]}</span>
          <input
            type="number"
            step={label === 'Scl' ? 0.1 : 1}
            value={values[i].toFixed(label === 'Scl' ? 2 : 1)}
            onChange={(e) => onChange(i, e.target.value)}
            className="w-full bg-transparent text-[10px] font-mono outline-none text-foreground"
          />
          {suffix && <span className="text-[8px] text-muted-foreground">{suffix}</span>}
        </div>
      ))}
    </div>
  )
}
