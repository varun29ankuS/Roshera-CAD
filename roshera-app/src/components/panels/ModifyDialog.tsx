/**
 * Fusion 360-style modify dialog for fillet / chamfer / shell.
 *
 * Replaces the old `window.prompt` flow (Task #81). Three behaviours:
 *
 *   - Fillet  / Chamfer : floating, non-modal panel anchored top-left
 *     of the viewport. While open, the dialog auto-switches the scene
 *     into 'edge' selection mode so the user can keep clicking edges
 *     in the canvas; the panel reflects the live count and stays open
 *     until OK / Cancel. On close it restores the prior selection mode.
 *
 *   - Shell : same panel, but no edge selection — operates on the
 *     currently selected solid. The "edges (N)" row is suppressed.
 *
 * Numeric input: text field with up/down steppers and a "mm" suffix,
 * matching Fusion's distance fields. The OK button is disabled while
 * the value isn't a positive finite number, or (fillet/chamfer) while
 * no edges are picked.
 *
 * The dialog calls the supplied `onApply(value)` and closes itself.
 * The backend call (sendDirectFillet/Chamfer/Shell) lives in ToolBar.tsx
 * — this component is purely presentational + selection-mode plumbing.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { X, ChevronUp, ChevronDown, MousePointerClick } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useSceneStore, type SelectionMode } from '@/stores/scene-store'
import { cn } from '@/lib/utils'

export type ModifyMode = 'fillet' | 'chamfer' | 'shell'

interface ModifyDialogProps {
  open: boolean
  mode: ModifyMode | null
  onOpenChange: (next: boolean) => void
  onApply: (mode: ModifyMode, value: number) => void
}

interface ModeSpec {
  title: string
  inputLabel: string
  defaultValue: number
  /** Whether the operation needs picked edges (true) or just a solid (false). */
  needsEdges: boolean
  okLabel: string
  /** Step for the +/- buttons in the numeric input. */
  step: number
}

const MODE_SPECS: Record<ModifyMode, ModeSpec> = {
  fillet: {
    title: 'Fillet',
    inputLabel: 'Radius',
    defaultValue: 2,
    needsEdges: true,
    okLabel: 'OK',
    step: 0.5,
  },
  chamfer: {
    title: 'Chamfer',
    inputLabel: 'Distance',
    defaultValue: 1,
    needsEdges: true,
    okLabel: 'OK',
    step: 0.5,
  },
  shell: {
    title: 'Shell',
    inputLabel: 'Thickness',
    defaultValue: 1,
    needsEdges: false,
    okLabel: 'OK',
    step: 0.25,
  },
}

export function ModifyDialog({ open, mode, onOpenChange, onApply }: ModifyDialogProps) {
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const subElementSelections = useSceneStore((s) => s.subElementSelections)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const setModifyPreview = useSceneStore((s) => s.setModifyPreview)

  const spec = mode ? MODE_SPECS[mode] : null
  const [valueRaw, setValueRaw] = useState<string>(
    spec ? String(spec.defaultValue) : '',
  )
  const inputRef = useRef<HTMLInputElement | null>(null)

  // Remember the prior selection mode so we can restore it on close —
  // matches Fusion's behaviour where leaving the modify command drops
  // you back into whatever selection filter you had before.
  const priorModeRef = useRef<SelectionMode | null>(null)

  // Reset numeric value + flip to edge mode on each open. We intentionally
  // do this in an effect (not a useState lazy initialiser) so re-opening
  // the same mode restores the default value rather than retaining stale
  // input from a previous session.
  useEffect(() => {
    if (!open || !spec || !mode) return
    setValueRaw(String(spec.defaultValue))
    if (spec.needsEdges) {
      priorModeRef.current = selectionMode
      if (selectionMode !== 'edge') setSelectionMode('edge')
    }
    // Focus + select the input for fast keyboard entry.
    const id = window.setTimeout(() => {
      inputRef.current?.focus()
      inputRef.current?.select()
    }, 0)
    return () => window.clearTimeout(id)
    // selectionMode intentionally omitted — we capture it once at open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode])

  // Restore the prior selection mode when the dialog closes.
  useEffect(() => {
    if (open || !spec) return
    if (priorModeRef.current && spec.needsEdges) {
      setSelectionMode(priorModeRef.current)
      priorModeRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  // Live count of picked edges scoped to the currently selected solid —
  // matches the filter `sendDirectFillet` applies before dispatching.
  const edgeCount = useMemo(() => {
    const ids = Array.from(selectedIds)
    if (ids.length !== 1) return 0
    const [object] = ids
    return subElementSelections.filter(
      (s) => s.type === 'edge' && s.objectId === object,
    ).length
  }, [selectedIds, subElementSelections])

  const parsedValue = useMemo(() => {
    const trimmed = valueRaw.trim()
    if (trimmed === '') return Number.NaN
    const n = Number(trimmed)
    return Number.isFinite(n) ? n : Number.NaN
  }, [valueRaw])

  const valueValid = Number.isFinite(parsedValue) && parsedValue > 0
  const selectionValid =
    !spec?.needsEdges || (selectedIds.size === 1 && edgeCount > 0)
  const canApply = valueValid && selectionValid

  // Publish a live cross-section preview to the viewport. Only fillet
  // and chamfer participate — shell preview is whole-solid and would
  // need a backend round-trip (see Task #85). The preview clears
  // automatically when the dialog closes via the cleanup return.
  useEffect(() => {
    if (!open || !mode || mode === 'shell' || !valueValid) {
      setModifyPreview(null)
      return
    }
    setModifyPreview({ mode, value: parsedValue })
    return () => setModifyPreview(null)
  }, [open, mode, parsedValue, valueValid, setModifyPreview])

  const close = useCallback(() => onOpenChange(false), [onOpenChange])

  const handleApply = useCallback(() => {
    if (!mode || !spec || !canApply) return
    onApply(mode, parsedValue)
    close()
  }, [mode, spec, canApply, parsedValue, onApply, close])

  // Keyboard: Enter applies, Escape cancels.
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      } else if (e.key === 'Enter') {
        // Enter while focused inside the input still works because we
        // listen on window — but bail if the user is typing in some
        // other input on the page.
        const tag = (e.target as HTMLElement | null)?.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || tag == null) {
          e.preventDefault()
          handleApply()
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, handleApply, close])

  if (!open || !spec || !mode) return null

  const stepValue = (delta: number) => {
    const next = Number.isFinite(parsedValue) ? parsedValue + delta : spec.defaultValue
    if (next <= 0) return
    // Trim float noise (`1.1 - 0.5 = 0.6000000000000001`) to 4 sig figs.
    const rounded = Math.round(next * 10000) / 10000
    setValueRaw(String(rounded))
  }

  const selectionHint = (() => {
    if (!spec.needsEdges) {
      return selectedIds.size === 1
        ? 'Solid selected.'
        : 'Select exactly one solid before applying.'
    }
    if (selectedIds.size !== 1) return 'Select one solid, then click edges.'
    if (edgeCount === 0) return 'Click edges in the viewport.'
    return `${edgeCount} edge${edgeCount === 1 ? '' : 's'} selected.`
  })()

  return (
    <div
      role="dialog"
      aria-label={`${spec.title} settings`}
      // Top-left, just under the menubar. Non-modal, so no backdrop —
      // user can still pan/orbit + pick edges in the canvas.
      className="fixed left-4 top-20 z-40 w-[280px] select-none rounded-lg border border-border bg-card/95 shadow-xl backdrop-blur"
    >
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border px-3 py-2">
        <span className="text-[13px] font-semibold tracking-wide">
          {spec.title}
        </span>
        <button
          type="button"
          onClick={close}
          className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
          aria-label="Close"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Body */}
      <div className="flex flex-col gap-3 p-3">
        {spec.needsEdges && (
          <Field label="Edges">
            <div
              className={cn(
                'flex items-center gap-2 rounded-md border px-2 py-1.5 text-[12px] font-mono',
                edgeCount > 0
                  ? 'border-primary/40 bg-primary/5 text-foreground'
                  : 'border-border text-muted-foreground',
              )}
            >
              <MousePointerClick className="h-3.5 w-3.5 shrink-0" />
              <span>{selectionHint}</span>
            </div>
          </Field>
        )}

        {!spec.needsEdges && (
          <div
            className={cn(
              'rounded-md border px-2 py-1.5 text-[12px] font-mono',
              selectedIds.size === 1
                ? 'border-primary/40 bg-primary/5 text-foreground'
                : 'border-destructive/40 bg-destructive/5 text-destructive',
            )}
          >
            {selectionHint}
          </div>
        )}

        <Field label={spec.inputLabel}>
          <div className="flex items-stretch overflow-hidden rounded-md border border-input focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
            <input
              ref={inputRef}
              type="text"
              inputMode="decimal"
              value={valueRaw}
              onChange={(e) => setValueRaw(e.target.value)}
              className={cn(
                'h-8 min-w-0 flex-1 bg-transparent px-2 font-mono text-[13px] outline-none',
                !valueValid && 'text-destructive',
              )}
              placeholder={String(spec.defaultValue)}
            />
            <span className="flex items-center bg-muted px-2 font-mono text-[11px] text-muted-foreground">
              mm
            </span>
            <div className="flex flex-col border-l border-input">
              <button
                type="button"
                onClick={() => stepValue(spec.step)}
                className="flex h-4 items-center justify-center px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                aria-label="Increase"
              >
                <ChevronUp className="h-3 w-3" />
              </button>
              <button
                type="button"
                onClick={() => stepValue(-spec.step)}
                className="flex h-4 items-center justify-center border-t border-input px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                aria-label="Decrease"
              >
                <ChevronDown className="h-3 w-3" />
              </button>
            </div>
          </div>
          {!valueValid && (
            <span className="text-[11px] font-mono text-destructive">
              Must be a positive number.
            </span>
          )}
        </Field>
      </div>

      {/* Footer */}
      <div className="flex justify-end gap-2 border-t border-border px-3 py-2">
        <Button variant="outline" size="sm" onClick={close}>
          Cancel
        </Button>
        <Button size="sm" onClick={handleApply} disabled={!canApply}>
          {spec.okLabel}
        </Button>
      </div>
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[10px] uppercase tracking-wider text-muted-foreground/70 font-mono">
        {label}
      </span>
      {children}
    </label>
  )
}
