/**
 * AddComponentDialog — modal for adding a new component instance to an
 * assembly. The kernel-side component is created with a fresh empty
 * `BRepModel` (part-binding lands in a later slice); the user supplies
 * a name and an optional starting translation.
 *
 * Rotation is not part of this dialog by design — the common case for
 * authoring an assembly is "place a copy at this offset and let the
 * solver pin orientation through mates". Components needing an
 * arbitrary initial pose can use the inline transform editor on the
 * component row (`set_component_transform` REST).
 */

import { useEffect, useState } from 'react'
import {
  addComponent,
  translationMatrix,
  COMPONENT_PRIMITIVE_TAGS,
  type ComponentPrimitive,
  type ComponentPrimitiveTag,
} from '@/lib/assembly-api'

type PrimitiveChoice = 'None' | ComponentPrimitiveTag

const PRIMITIVE_CHOICES: readonly PrimitiveChoice[] = [
  'None',
  ...COMPONENT_PRIMITIVE_TAGS,
] as const

interface Props {
  assemblyId: string
  /** Suggested default for the name field (e.g. `Component 3`). */
  defaultName: string
  onClose: () => void
  onCreated: (componentId: string) => void
}

export function AddComponentDialog({ assemblyId, defaultName, onClose, onCreated }: Props) {
  const [name, setName] = useState(defaultName)
  const [x, setX] = useState(0)
  const [y, setY] = useState(0)
  const [z, setZ] = useState(0)
  // Default to a unit box so a freshly-added component shows up in the
  // viewport — empty BRepModel components are valid but invisible.
  const [primitive, setPrimitive] = useState<PrimitiveChoice>('Box')
  const [boxDx, setBoxDx] = useState(10)
  const [boxDy, setBoxDy] = useState(10)
  const [boxDz, setBoxDz] = useState(10)
  const [cylRadius, setCylRadius] = useState(5)
  const [cylHeight, setCylHeight] = useState(10)
  const [sphereRadius, setSphereRadius] = useState(5)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  /** Compose the wire-shape primitive payload from the dialog state. */
  const buildPrimitive = (): ComponentPrimitive | undefined => {
    switch (primitive) {
      case 'None':
        return undefined
      case 'Box':
        return { type: 'Box', dx: boxDx, dy: boxDy, dz: boxDz }
      case 'Cylinder':
        return { type: 'Cylinder', radius: cylRadius, height: cylHeight }
      case 'Sphere':
        return { type: 'Sphere', radius: sphereRadius }
    }
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !busy) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, busy])

  const submit = async () => {
    const trimmed = name.trim()
    if (!trimmed) {
      setError('Component name is required.')
      return
    }
    const primitivePayload = buildPrimitive()
    // Reject obviously-invalid primitives client-side rather than
    // round-tripping to the kernel's `InvalidParameters` error path.
    if (primitivePayload) {
      const checkPositive = (label: string, n: number): boolean => {
        if (!Number.isFinite(n) || n <= 0) {
          setError(`${label} must be a positive number.`)
          return false
        }
        return true
      }
      if (primitivePayload.type === 'Box') {
        if (
          !checkPositive('Box X', primitivePayload.dx) ||
          !checkPositive('Box Y', primitivePayload.dy) ||
          !checkPositive('Box Z', primitivePayload.dz)
        ) {
          return
        }
      } else if (primitivePayload.type === 'Cylinder') {
        if (
          !checkPositive('Cylinder radius', primitivePayload.radius) ||
          !checkPositive('Cylinder height', primitivePayload.height)
        ) {
          return
        }
      } else if (primitivePayload.type === 'Sphere') {
        if (!checkPositive('Sphere radius', primitivePayload.radius)) {
          return
        }
      }
    }
    setBusy(true)
    setError(null)
    try {
      // Skip the transform payload when it's the identity translation
      // — keeps the recorded RecordedOperation parameters terse.
      const isIdentity = x === 0 && y === 0 && z === 0
      const id = await addComponent(assemblyId, {
        name: trimmed,
        transform: isIdentity ? undefined : translationMatrix(x, y, z),
        primitive: primitivePayload,
      })
      onCreated(id)
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose()
      }}
    >
      <div
        className="w-[380px] max-w-[92vw] bg-popover text-popover-foreground border border-border rounded shadow-lg"
        role="dialog"
        aria-label="Add component"
      >
        <div className="px-4 py-3 border-b border-border/60 flex items-center justify-between">
          <h2 className="text-sm font-medium">Add Component</h2>
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            aria-label="Close"
            className="cad-focus text-muted-foreground hover:text-foreground disabled:opacity-50"
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.75">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>

        <div className="px-4 py-3 space-y-3 text-xs">
          {error && (
            <div className="px-2 py-1 rounded bg-destructive/10 text-destructive border border-destructive/30">
              {error}
            </div>
          )}

          <label className="flex items-center gap-2">
            <span className="w-24 text-muted-foreground">Name</span>
            <input
              autoFocus
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void submit()
                }
              }}
              className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
            />
          </label>

          <div className="border-t border-border/40 pt-2">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground mb-2">
              Geometry
            </div>
            <label className="flex items-center gap-2">
              <span className="w-24 text-muted-foreground">Primitive</span>
              <select
                value={primitive}
                onChange={(e) => setPrimitive(e.target.value as PrimitiveChoice)}
                className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
              >
                {PRIMITIVE_CHOICES.map((c) => (
                  <option key={c} value={c}>
                    {c === 'None' ? 'None (empty)' : c}
                  </option>
                ))}
              </select>
            </label>
            {primitive === 'Box' && (
              <div className="grid grid-cols-3 gap-2 mt-2">
                <NumField label="X" value={boxDx} onChange={setBoxDx} />
                <NumField label="Y" value={boxDy} onChange={setBoxDy} />
                <NumField label="Z" value={boxDz} onChange={setBoxDz} />
              </div>
            )}
            {primitive === 'Cylinder' && (
              <div className="grid grid-cols-2 gap-2 mt-2">
                <NumField label="R" value={cylRadius} onChange={setCylRadius} />
                <NumField label="H" value={cylHeight} onChange={setCylHeight} />
              </div>
            )}
            {primitive === 'Sphere' && (
              <div className="grid grid-cols-1 gap-2 mt-2">
                <NumField label="R" value={sphereRadius} onChange={setSphereRadius} />
              </div>
            )}
            {primitive === 'None' && (
              <div className="mt-2 text-[10px] text-muted-foreground">
                Empty BRepModel — useful for placeholder components or
                later part-binding. Won't be visible in the viewport.
              </div>
            )}
          </div>

          <div className="border-t border-border/40 pt-2">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground mb-2">
              Starting position (mm)
            </div>
            <div className="grid grid-cols-3 gap-2">
              <NumField label="X" value={x} onChange={setX} />
              <NumField label="Y" value={y} onChange={setY} />
              <NumField label="Z" value={z} onChange={setZ} />
            </div>
            <div className="mt-2 text-[10px] text-muted-foreground">
              Rotation defaults to identity. Use the row editor or a mate
              after creation to orient.
            </div>
          </div>
        </div>

        <div className="px-4 py-3 border-t border-border/60 flex items-center justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="cad-focus px-3 py-1 text-xs rounded border border-border hover:bg-accent/40 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => void submit()}
            disabled={busy}
            className="cad-focus px-3 py-1 text-xs font-medium rounded bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-50"
          >
            {busy ? 'Adding…' : 'Add Component'}
          </button>
        </div>
      </div>
    </div>
  )
}

function NumField({
  label,
  value,
  onChange,
}: {
  label: string
  value: number
  onChange: (n: number) => void
}) {
  return (
    <label className="flex items-center gap-1">
      <span className="w-3 text-muted-foreground">{label}</span>
      <input
        type="number"
        value={value}
        step="1"
        onChange={(e) => onChange(Number(e.target.value))}
        className="cad-focus flex-1 min-w-0 px-2 py-1 rounded border border-border bg-background"
      />
    </label>
  )
}
