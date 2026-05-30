/**
 * AddMateDialog — modal for creating a new mate constraint.
 *
 * Inputs:
 *   - Mate type (dropdown across all twelve `MateType` variants)
 *   - Component A + reference name (text field; matches a slot
 *     registered via `POST /api/assemblies/{id}/references`)
 *   - Component B + reference name
 *   - Parameter input shown only for parameterised mate types
 *     (Distance / Angle / Gear ratio)
 *
 * On submit, calls `addMate(...)` and notifies the caller via
 * `onCreated(newMateId)`. The parent (`AssemblyWorkspace`) is
 * responsible for refreshing the snapshot.
 */

import { useEffect, useId, useMemo, useState } from 'react'
import {
  addMate,
  makeMateType,
  mateTypeLabel,
  mateTypeNeedsParameter,
  MATE_TYPE_TAGS,
  type ComponentSummary,
  type MateTypeTag,
} from '@/lib/assembly-api'

interface Props {
  /** UUID of the assembly receiving the mate. */
  assemblyId: string
  /** Components available as endpoints. The dialog needs at least two
   *  to be usable; the parent should not render this if fewer. */
  components: ComponentSummary[]
  /** Close handler — also called after a successful create. */
  onClose: () => void
  /** Fired with the newly-created mate id so the parent can refresh. */
  onCreated: (mateId: string) => void
}

export function AddMateDialog({ assemblyId, components, onClose, onCreated }: Props) {
  // Default-pick the first two components if available. The user can
  // still change them; `useMemo` ensures the defaults only re-pick on
  // a real component-list change, not on every keystroke.
  const defaultA = components[0]?.id ?? ''
  const defaultB = components[1]?.id ?? defaultA
  const [componentA, setComponentA] = useState<string>(defaultA)
  const [componentB, setComponentB] = useState<string>(defaultB)
  const [referenceA, setReferenceA] = useState<string>('')
  const [referenceB, setReferenceB] = useState<string>('')
  const [tag, setTag] = useState<MateTypeTag>('Coincident')
  const [parameter, setParameter] = useState<number>(0)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Pre-fill reference fields from the first registered slot on the
  // chosen component, when one exists. Cheap quality-of-life — the
  // user is free to overwrite.
  const refsByComponent = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const c of components) {
      map.set(
        c.id,
        c.mate_references.map((r) => r.name),
      )
    }
    return map
  }, [components])

  useEffect(() => {
    const refs = refsByComponent.get(componentA) ?? []
    if (refs.length > 0 && !refs.includes(referenceA)) {
      setReferenceA(refs[0])
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [componentA, refsByComponent])

  useEffect(() => {
    const refs = refsByComponent.get(componentB) ?? []
    if (refs.length > 0 && !refs.includes(referenceB)) {
      setReferenceB(refs[0])
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [componentB, refsByComponent])

  // Dismiss on Escape, no matter where focus is inside the modal.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !busy) onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose, busy])

  const submit = async () => {
    if (!componentA || !componentB) {
      setError('Both components must be selected.')
      return
    }
    if (componentA === componentB) {
      setError('Component A and B must be different.')
      return
    }
    if (!referenceA.trim() || !referenceB.trim()) {
      setError('Both reference names are required.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const mate_id = await addMate(assemblyId, {
        mate_type: makeMateType(tag, parameter),
        component1: componentA,
        reference1: referenceA.trim(),
        component2: componentB,
        reference2: referenceB.trim(),
      })
      onCreated(mate_id)
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setBusy(false)
    }
  }

  return (
    // Backdrop: clicking outside the panel cancels (mirrors the dialog
    // UX in Drawing's Add View flow).
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose()
      }}
    >
      <div
        className="w-[420px] max-w-[92vw] bg-popover text-popover-foreground border border-border rounded shadow-lg"
        role="dialog"
        aria-label="Add mate"
      >
        <div className="px-4 py-3 border-b border-border/60 flex items-center justify-between">
          <h2 className="text-sm font-medium">Add Mate</h2>
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="cad-focus text-muted-foreground hover:text-foreground disabled:opacity-50"
            aria-label="Close"
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

          <FieldRow label="Mate type">
            <select
              value={tag}
              onChange={(e) => setTag(e.target.value as MateTypeTag)}
              className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
            >
              {MATE_TYPE_TAGS.map((t) => (
                <option key={t} value={t}>
                  {mateTypeLabel(t)}
                </option>
              ))}
            </select>
          </FieldRow>

          {mateTypeNeedsParameter(tag) && (
            <FieldRow label={mateTypeLabel(tag)}>
              <input
                type="number"
                value={parameter}
                onChange={(e) => setParameter(Number(e.target.value))}
                step="0.1"
                className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
              />
            </FieldRow>
          )}

          <div className="border-t border-border/40 pt-2 space-y-2">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
              Component A
            </div>
            <FieldRow label="Component">
              <ComponentPicker
                value={componentA}
                components={components}
                onChange={setComponentA}
              />
            </FieldRow>
            <FieldRow label="Reference">
              <ReferenceInput
                value={referenceA}
                slots={refsByComponent.get(componentA) ?? []}
                onChange={setReferenceA}
              />
            </FieldRow>
          </div>

          <div className="border-t border-border/40 pt-2 space-y-2">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
              Component B
            </div>
            <FieldRow label="Component">
              <ComponentPicker
                value={componentB}
                components={components}
                onChange={setComponentB}
              />
            </FieldRow>
            <FieldRow label="Reference">
              <ReferenceInput
                value={referenceB}
                slots={refsByComponent.get(componentB) ?? []}
                onChange={setReferenceB}
              />
            </FieldRow>
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
            {busy ? 'Adding…' : 'Add Mate'}
          </button>
        </div>
      </div>
    </div>
  )
}

function FieldRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex items-center gap-2">
      <span className="w-24 text-muted-foreground">{label}</span>
      {children}
    </label>
  )
}

function ComponentPicker({
  value,
  components,
  onChange,
}: {
  value: string
  components: ComponentSummary[]
  onChange: (id: string) => void
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
    >
      {components.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </select>
  )
}

/**
 * Reference-slot input. When the picked component has any registered
 * `MateReference` slots we render a `<datalist>`-backed combobox so
 * users can either pick a slot or type a free-form name (slots are
 * lazily registered server-side; the picker stays useful before the
 * registration round-trip completes).
 */
function ReferenceInput({
  value,
  slots,
  onChange,
}: {
  value: string
  slots: string[]
  onChange: (s: string) => void
}) {
  // Unique datalist id to avoid cross-row collision when both A and B
  // inputs are on screen. `useId` is the pure, SSR-safe replacement for
  // the previous `Math.random()` (which made render impure —
  // react-hooks/purity).
  const listId = `ref-slots-${useId()}`
  return (
    <>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        list={slots.length > 0 ? listId : undefined}
        placeholder="slot name"
        className="cad-focus flex-1 px-2 py-1 rounded border border-border bg-background"
      />
      {slots.length > 0 && (
        <datalist id={listId}>
          {slots.map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
      )}
    </>
  )
}
