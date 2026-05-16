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
 */

import { useEffect, useState } from 'react'
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

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !busy) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, busy])

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
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose()
      }}
    >
      <div
        className="w-[440px] max-w-[92vw] bg-popover text-popover-foreground border border-border rounded shadow-lg"
        role="dialog"
        aria-label="Add mate reference"
      >
        <div className="px-4 py-3 border-b border-border/60 flex items-center justify-between">
          <div className="flex flex-col">
            <h2 className="text-sm font-medium">Add Mate Reference</h2>
            <span className="text-[10px] text-muted-foreground">
              on {component.name}
            </span>
          </div>
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

          <FieldRow label="Slot name">
            <input
              autoFocus
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. axis1, top_face"
              className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
            />
          </FieldRow>

          <FieldRow label="Reference type">
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as RefKind)}
              className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
            >
              {REF_KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </FieldRow>

          {(kind === 'Face' || kind === 'Edge') && (
            <FieldRow label={kind === 'Face' ? 'Face UUID' : 'Edge UUID'}>
              <input
                type="text"
                value={topologyId}
                onChange={(e) => setTopologyId(e.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background font-mono text-[11px]"
              />
            </FieldRow>
          )}

          <div className="border-t border-border/40 pt-2 space-y-2">
            <Vec3Field label={primaryVec3Label(kind)} value={v1} onChange={setV1} />
            {kindUsesV2(kind) && (
              <Vec3Field label={secondaryVec3Label(kind)} value={v2} onChange={setV2} />
            )}
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
            {busy ? 'Saving…' : 'Register Reference'}
          </button>
        </div>
      </div>
    </div>
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

function FieldRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex items-center gap-2">
      <span className="w-24 text-muted-foreground">{label}</span>
      {children}
    </label>
  )
}

function Vec3Field({
  label,
  value,
  onChange,
}: {
  label: string
  value: [number, number, number]
  onChange: (v: [number, number, number]) => void
}) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-24 text-muted-foreground">{label}</span>
      <div className="flex-1 grid grid-cols-3 gap-1">
        {(['x', 'y', 'z'] as const).map((axis, i) => (
          <label key={axis} className="flex items-center gap-1">
            <span className="w-3 text-muted-foreground">{axis.toUpperCase()}</span>
            <input
              type="number"
              step="0.1"
              value={value[i]}
              onChange={(e) => {
                const n = Number(e.target.value)
                const next: [number, number, number] = [...value]
                next[i] = n
                onChange(next)
              }}
              className="cad-focus flex-1 min-w-0 px-1.5 py-0.5 rounded border border-border bg-background text-[11px]"
            />
          </label>
        ))}
      </div>
    </div>
  )
}
