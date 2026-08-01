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
 *
 * Uses the shared `ui/dialog` primitives (focus trap, Escape, overlay)
 * and the `dialog-form` vocabulary so it matches every other dialog.
 */

import { useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { DialogError, FormField } from './dialog-form'
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
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next && !busy) onClose()
      }}
    >
      <DialogContent className="sm:max-w-sm">
        <DialogHeader>
          <DialogTitle>Add component</DialogTitle>
          <DialogDescription>
            New instance in this assembly, with optional starting geometry.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <FormField label="Name">
            <Input
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
            />
          </FormField>

          <FormField label="Primitive">
            <select
              value={primitive}
              onChange={(e) => setPrimitive(e.target.value as PrimitiveChoice)}
              className="cad-focus h-8 rounded border border-border bg-background px-2 text-xs"
            >
              {PRIMITIVE_CHOICES.map((c) => (
                <option key={c} value={c}>
                  {c === 'None' ? 'None (empty)' : c}
                </option>
              ))}
            </select>
          </FormField>

          {primitive === 'Box' && (
            <div className="grid grid-cols-3 gap-2">
              <SizeField label="X" value={boxDx} onChange={setBoxDx} />
              <SizeField label="Y" value={boxDy} onChange={setBoxDy} />
              <SizeField label="Z" value={boxDz} onChange={setBoxDz} />
            </div>
          )}
          {primitive === 'Cylinder' && (
            <div className="grid grid-cols-2 gap-2">
              <SizeField label="R" value={cylRadius} onChange={setCylRadius} />
              <SizeField label="H" value={cylHeight} onChange={setCylHeight} />
            </div>
          )}
          {primitive === 'Sphere' && (
            <div className="grid grid-cols-1 gap-2">
              <SizeField label="R" value={sphereRadius} onChange={setSphereRadius} />
            </div>
          )}
          {primitive === 'None' && (
            <p className="text-[11px] leading-snug text-muted-foreground">
              Empty BRepModel — useful for placeholder components or later part-binding.
              Won't be visible in the viewport.
            </p>
          )}

          {/* Placement is deliberately demoted behind a disclosure: an
              assembly is RELATIONSHIPS, not placements. Mates own the
              final pose — a typed-in offset is a temporary scaffold the
              solver overrides, and making it prominent taught the wrong
              mental model (Varun, 2026-08-01). */}
          <details className="group">
            <summary className="cad-focus cursor-pointer list-none font-mono text-[10px] uppercase tracking-wider text-muted-foreground/70 hover:text-foreground">
              <span className="mr-1 inline-block transition-transform group-open:rotate-90">▸</span>
              Starting offset (mm) — optional
            </summary>
            <div className="mt-2 grid grid-cols-3 gap-2">
              <SizeField label="X" value={x} onChange={setX} />
              <SizeField label="Y" value={y} onChange={setY} />
              <SizeField label="Z" value={z} onChange={setZ} />
            </div>
            <p className="mt-1.5 text-[11px] leading-snug text-muted-foreground">
              A convenience so new components don't stack at the origin. Mates own the
              final pose — constrain it with Mate mode after creation.
            </p>
          </details>

          <DialogError>{error}</DialogError>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? 'Adding…' : 'Add component'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function SizeField({
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
      <span className="w-3 font-mono text-[10px] uppercase text-muted-foreground">{label}</span>
      <Input
        type="number"
        value={value}
        step="1"
        onChange={(e) => onChange(Number(e.target.value))}
        aria-label={label}
        className="cad-readout min-w-0 flex-1 text-[12px]"
      />
    </label>
  )
}
