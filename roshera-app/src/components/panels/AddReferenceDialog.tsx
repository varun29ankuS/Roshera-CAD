/**
 * AddReferenceDialog — modal for registering a `MateReference` slot on
 * a component. The slot is a named handle (e.g. `axis1`, `top_face`)
 * that mate constraints later target by name; the geometry payload
 * tells the kernel how to evaluate the constraint.
 *
 * Five reference variants are supported (mirroring
 * `geometry_engine::assembly::MateReference`):
 *
 *   - Face   — `{ face_id: UUID, normal: Vec3 }`
 *   - Edge   — `{ edge_id: UUID, direction: Vec3 }`
 *   - Point  — `{ position: Vec3 }`
 *   - Axis   — `{ origin: Vec3, direction: Vec3 }`
 *   - Plane  — `{ origin: Vec3, normal: Vec3 }`
 *
 * Face/Edge require a topology UUID that comes from the component's
 * `BRepModel` — once part-binding is wired, the picker can populate
 * a dropdown from the actual face/edge list. For now the user types
 * the UUID, or uses Point/Axis/Plane (which carry their own geometry).
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
import { DialogError, FormField, Vec3Input } from './dialog-form'
import {
  registerMateReference,
  type ComponentSummary,
  type MateReference,
} from '@/lib/assembly-api'

type RefKind = 'Face' | 'Edge' | 'Point' | 'Axis' | 'Plane'

const REF_KINDS: readonly RefKind[] = ['Point', 'Axis', 'Plane', 'Face', 'Edge'] as const

interface Props {
  assemblyId: string
  component: ComponentSummary
  onClose: () => void
  onCreated: () => void
}

export function AddReferenceDialog({ assemblyId, component, onClose, onCreated }: Props) {
  const [kind, setKind] = useState<RefKind>('Axis')
  const [name, setName] = useState('')
  // Two generic Vec3 buffers covering every variant's needs.
  const [v1, setV1] = useState<[number, number, number]>([0, 0, 0])
  const [v2, setV2] = useState<[number, number, number]>([0, 0, 1])
  // Topology UUID (only Face / Edge consume this).
  const [topologyId, setTopologyId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const buildReference = (): MateReference | null => {
    const vec = (a: [number, number, number]) => ({ x: a[0], y: a[1], z: a[2] })
    switch (kind) {
      case 'Face':
        if (!topologyId.trim()) {
          setError('Face id is required.')
          return null
        }
        return { Face: { face_id: topologyId.trim(), normal: vec(v1) } }
      case 'Edge':
        if (!topologyId.trim()) {
          setError('Edge id is required.')
          return null
        }
        return { Edge: { edge_id: topologyId.trim(), direction: vec(v1) } }
      case 'Point':
        return { Point: { position: vec(v1) } }
      case 'Axis':
        return { Axis: { origin: vec(v1), direction: vec(v2) } }
      case 'Plane':
        return { Plane: { origin: vec(v1), normal: vec(v2) } }
    }
  }

  const submit = async () => {
    const trimmed = name.trim()
    if (!trimmed) {
      setError('Slot name is required.')
      return
    }
    const reference = buildReference()
    if (!reference) return
    setBusy(true)
    setError(null)
    try {
      await registerMateReference(assemblyId, {
        component: component.id,
        name: trimmed,
        reference,
      })
      onCreated()
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
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add mate reference</DialogTitle>
          <DialogDescription>
            Named geometry slot on <span className="font-mono">{component.name}</span> for
            mates to target.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <FormField label="Slot name">
            <Input
              autoFocus
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. axis1, top_face"
            />
          </FormField>

          <FormField label="Reference type">
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as RefKind)}
              className="cad-focus h-8 rounded border border-border bg-background px-2 text-xs"
            >
              {REF_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </FormField>

          {(kind === 'Face' || kind === 'Edge') && (
            <FormField label={kind === 'Face' ? 'Face UUID' : 'Edge UUID'}>
              <Input
                type="text"
                value={topologyId}
                onChange={(e) => setTopologyId(e.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                className="font-mono text-[11px]"
              />
            </FormField>
          )}

          <Vec3Input label={primaryVec3Label(kind)} value={v1} onChange={setV1} />
          {kindUsesV2(kind) && (
            <Vec3Input label={secondaryVec3Label(kind)} value={v2} onChange={setV2} />
          )}

          <DialogError>{error}</DialogError>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? 'Saving…' : 'Register reference'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function kindUsesV2(k: RefKind): boolean {
  return k === 'Axis' || k === 'Plane'
}

function primaryVec3Label(k: RefKind): string {
  switch (k) {
    case 'Face':
      return 'Normal'
    case 'Edge':
      return 'Direction'
    case 'Point':
      return 'Position'
    case 'Axis':
      return 'Origin'
    case 'Plane':
      return 'Origin'
  }
}

function secondaryVec3Label(k: RefKind): string {
  // Only Axis / Plane reach here.
  return k === 'Axis' ? 'Direction' : 'Normal'
}
