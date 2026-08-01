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
 *
 * Uses the shared `ui/dialog` primitives (focus trap, Escape, overlay)
 * and the `dialog-form` vocabulary so it matches every other dialog.
 */

import { useEffect, useId, useMemo, useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { DialogError, FormField } from './dialog-form'
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
    <Dialog
      open
      onOpenChange={(next) => {
        if (!next && !busy) onClose()
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add mate</DialogTitle>
          <DialogDescription>
            Constrain two component references; the solver pins the pose.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <FormField label="Mate type">
            <select
              value={tag}
              onChange={(e) => setTag(e.target.value as MateTypeTag)}
              className="cad-focus h-8 rounded border border-border bg-background px-2 text-xs"
            >
              {MATE_TYPE_TAGS.map((t) => (
                <option key={t} value={t}>
                  {mateTypeLabel(t)}
                </option>
              ))}
            </select>
          </FormField>

          {mateTypeNeedsParameter(tag) && (
            <FormField label={mateTypeLabel(tag)}>
              <input
                type="number"
                value={parameter}
                onChange={(e) => setParameter(Number(e.target.value))}
                step="0.1"
                className="cad-focus cad-readout h-8 rounded border border-border bg-background px-2 text-xs"
              />
            </FormField>
          )}

          {(['A', 'B'] as const).map((side) => {
            const componentId = side === 'A' ? componentA : componentB
            const setComponent = side === 'A' ? setComponentA : setComponentB
            const reference = side === 'A' ? referenceA : referenceB
            const setReference = side === 'A' ? setReferenceA : setReferenceB
            return (
              <div key={side} className="grid grid-cols-2 gap-2">
                <FormField label={`Component ${side}`}>
                  <select
                    value={componentId}
                    onChange={(e) => setComponent(e.target.value)}
                    className="cad-focus h-8 w-full rounded border border-border bg-background px-2 text-xs"
                  >
                    {components.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name}
                      </option>
                    ))}
                  </select>
                </FormField>
                <FormField label="Reference">
                  <ReferenceInput
                    value={reference}
                    slots={refsByComponent.get(componentId) ?? []}
                    onChange={setReference}
                  />
                </FormField>
              </div>
            )
          })}

          <DialogError>{error}</DialogError>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? 'Adding…' : 'Add mate'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
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
        className="cad-focus h-8 w-full rounded border border-border bg-background px-2 text-xs"
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
