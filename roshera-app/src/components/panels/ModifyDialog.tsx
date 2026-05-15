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

export type ModifyMode =
  | 'fillet'
  | 'fillet-variable'
  | 'fillet-linear'
  | 'fillet-stations'
  | 'chamfer'
  | 'shell'

/**
 * Discriminated apply payload.
 *
 * - `fillet` / `chamfer` / `shell` ship a single value (legacy contract).
 * - `fillet-variable` ships one constant radius per picked edge in
 *   pick order (the radius is constant *along the edge*, but the array
 *   is per-edge).
 * - `fillet-linear` ships one (start, end) pair applied uniformly to
 *   every picked edge — radius interpolates linearly along each edge.
 * - `fillet-stations` ships one parameter-station table applied
 *   uniformly to every picked edge — radius is sampled at the listed
 *   `(station ∈ [0, 1], radius)` pairs and the kernel rolling-ball
 *   solver interpolates between them.
 *
 * The api-server's `fillet_payload` parser routes `radius: number`
 * to `Constant`, `radius: { kind: "linear", … }` to `Linear`, and
 * `radius: { kind: "variable", samples: … }` to `Variable`. The
 * `fillet-variable` mode keeps its bare-number-per-edge `radii` shape;
 * the parser accepts that as N parallel `Constant` profiles.
 */
export type ModifyApplyPayload =
  | { mode: 'fillet' | 'chamfer' | 'shell'; value: number }
  | { mode: 'fillet-variable'; radii: number[] }
  | { mode: 'fillet-linear'; start: number; end: number }
  | { mode: 'fillet-stations'; samples: Array<[number, number]> }

interface ModifyDialogProps {
  open: boolean
  mode: ModifyMode | null
  onOpenChange: (next: boolean) => void
  onApply: (payload: ModifyApplyPayload) => void
}

/**
 * Profile axis (independent of `perEdge`):
 *
 * - `'constant'` — one positive number (single field, or one per edge
 *   when `perEdge` is set). Maps to `BlendRadiusDto::Constant` on the
 *   wire.
 * - `'linear'`   — start + end positive numbers. Radius interpolates
 *   linearly along each edge. Maps to `BlendRadiusDto::Linear`.
 * - `'stations'` — table of `(station ∈ [0, 1], radius > 0)` rows.
 *   Maps to `BlendRadiusDto::Variable`.
 *
 * Only `'constant'` is compatible with `perEdge: true` today —
 * Linear/Stations are uniform across all picked edges. If you ever
 * need per-edge Linear/Stations, prefer a richer dialog over
 * combinatorial mode names.
 */
type FilletProfile = 'constant' | 'linear' | 'stations'

interface ModeSpec {
  title: string
  inputLabel: string
  defaultValue: number
  /** Whether the operation needs picked edges (true) or just a solid (false). */
  needsEdges: boolean
  /**
   * Per-edge mode: when `true`, the dialog renders one numeric input per
   * picked edge instead of a single shared value. Only `fillet-variable`
   * sets this today.
   */
  perEdge: boolean
  /** Profile shape — see [`FilletProfile`]. Non-fillet modes are `'constant'`. */
  profile: FilletProfile
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
    perEdge: false,
    profile: 'constant',
    okLabel: 'OK',
    step: 0.5,
  },
  'fillet-variable': {
    title: 'Fillet (per-edge radii)',
    inputLabel: 'Radius',
    defaultValue: 2,
    needsEdges: true,
    perEdge: true,
    profile: 'constant',
    okLabel: 'OK',
    step: 0.5,
  },
  'fillet-linear': {
    title: 'Fillet (linear start→end)',
    inputLabel: 'Radius',
    defaultValue: 2,
    needsEdges: true,
    perEdge: false,
    profile: 'linear',
    okLabel: 'OK',
    step: 0.5,
  },
  'fillet-stations': {
    title: 'Fillet (per-station)',
    inputLabel: 'Radius',
    defaultValue: 2,
    needsEdges: true,
    perEdge: false,
    profile: 'stations',
    okLabel: 'OK',
    step: 0.5,
  },
  chamfer: {
    title: 'Chamfer',
    inputLabel: 'Distance',
    defaultValue: 1,
    needsEdges: true,
    perEdge: false,
    profile: 'constant',
    okLabel: 'OK',
    step: 0.5,
  },
  shell: {
    title: 'Shell',
    inputLabel: 'Thickness',
    defaultValue: 1,
    needsEdges: false,
    perEdge: false,
    profile: 'constant',
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
  // Per-edge radii — keyed by EdgeId (the `SubElementSelection.index`)
  // so the entry survives selection reordering. Stored raw (as the
  // user typed) mirroring `valueRaw` so the input round-trips and the
  // user can see "1." mid-type without it parsing to 1 prematurely.
  const [radiiRaw, setRadiiRaw] = useState<Map<number, string>>(() => new Map())

  // Linear-profile inputs (`fillet-linear` only). Two scalar fields,
  // start and end radii at the edge endpoints. Defaults are seeded
  // from the spec on each open via the reset effect below.
  const [linearStartRaw, setLinearStartRaw] = useState<string>('')
  const [linearEndRaw, setLinearEndRaw] = useState<string>('')

  // Per-station table (`fillet-stations` only). Each row is a
  // `(station, radius)` pair as the user typed it. Rows are added /
  // removed by the user; reordering is via array index, not a stable
  // key, because (unlike picked edges) there is no external identity
  // to track. The list is constructed with a sensible default on
  // first open (3 evenly-spaced stations with the spec default
  // radius), giving the user something concrete to edit.
  interface StationRow {
    station: string
    radius: string
  }
  const [stationsRaw, setStationsRaw] = useState<StationRow[]>([])
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
    // Linear: same default at both endpoints — the user immediately
    // sees a recognisable starting state and edits whichever endpoint
    // they want to change.
    if (spec.profile === 'linear') {
      setLinearStartRaw(String(spec.defaultValue))
      setLinearEndRaw(String(spec.defaultValue * 1.5))
    }
    // Stations: three evenly-spaced rows with the spec default radius.
    // Three is the smallest table that demonstrates "shaped along the
    // edge" (start, middle, end) without overwhelming the user; they
    // can add or remove rows from there.
    if (spec.profile === 'stations') {
      setStationsRaw([
        { station: '0', radius: String(spec.defaultValue) },
        { station: '0.5', radius: String(spec.defaultValue * 1.5) },
        { station: '1', radius: String(spec.defaultValue) },
      ])
    }
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

  // Live picked-edge list scoped to the currently selected solid —
  // matches the filter `sendDirectFillet` applies before dispatching.
  // We keep the full list (not just the count) because per-edge mode
  // renders one input per edge keyed by `EdgeId`.
  const pickedEdges = useMemo<number[]>(() => {
    const ids = Array.from(selectedIds)
    if (ids.length !== 1) return []
    const [object] = ids
    return subElementSelections
      .filter((s) => s.type === 'edge' && s.objectId === object)
      .map((s) => s.index)
  }, [selectedIds, subElementSelections])
  const edgeCount = pickedEdges.length

  // Sync `radiiRaw` against the live pick set whenever per-edge mode is
  // active: insert default for newly-picked edges, drop entries for
  // unpicked edges. Pick order (= array order of `pickedEdges`) is what
  // we send to the backend, so the map only needs to track presence +
  // value; reordering doesn't lose data because the key is `EdgeId`.
  useEffect(() => {
    if (!open || !spec?.perEdge) return
    setRadiiRaw((prev) => {
      const next = new Map(prev)
      const wanted = new Set(pickedEdges)
      for (const eid of pickedEdges) {
        if (!next.has(eid)) next.set(eid, String(spec.defaultValue))
      }
      for (const eid of Array.from(next.keys())) {
        if (!wanted.has(eid)) next.delete(eid)
      }
      return next
    })
  }, [open, spec, pickedEdges])

  const parsedValue = useMemo(() => {
    const trimmed = valueRaw.trim()
    if (trimmed === '') return Number.NaN
    const n = Number(trimmed)
    return Number.isFinite(n) ? n : Number.NaN
  }, [valueRaw])

  // Parallel array of parsed per-edge radii in pick order. Used both
  // for the apply payload and the canApply gate.
  const parsedRadii = useMemo<number[]>(() => {
    return pickedEdges.map((eid) => {
      const raw = (radiiRaw.get(eid) ?? '').trim()
      if (raw === '') return Number.NaN
      const n = Number(raw)
      return Number.isFinite(n) ? n : Number.NaN
    })
  }, [pickedEdges, radiiRaw])

  const valueValid = Number.isFinite(parsedValue) && parsedValue > 0
  const selectionValid =
    !spec?.needsEdges || (selectedIds.size === 1 && edgeCount > 0)
  const allRadiiValid =
    parsedRadii.length === pickedEdges.length &&
    parsedRadii.every((n) => Number.isFinite(n) && n > 0)

  // Parse the Linear endpoints — fields trim+parse as positive finite
  // numbers. Empty strings parse to NaN so `valid` reflects "user
  // hasn't typed yet" identically to "user typed garbage".
  const parsedLinearStart = useMemo(() => {
    const t = linearStartRaw.trim()
    if (t === '') return Number.NaN
    const n = Number(t)
    return Number.isFinite(n) ? n : Number.NaN
  }, [linearStartRaw])
  const parsedLinearEnd = useMemo(() => {
    const t = linearEndRaw.trim()
    if (t === '') return Number.NaN
    const n = Number(t)
    return Number.isFinite(n) ? n : Number.NaN
  }, [linearEndRaw])
  const linearValid =
    Number.isFinite(parsedLinearStart) &&
    parsedLinearStart > 0 &&
    Number.isFinite(parsedLinearEnd) &&
    parsedLinearEnd > 0

  // Parse the station table. Each row produces a `[station, radius]`
  // tuple; rows with empty / unparseable values keep NaN and the
  // table is rejected as a whole. Station range is [0, 1] inclusive —
  // matching the kernel's `validate_fillet_inputs` + the api-server's
  // `fillet_payload` validator.
  const parsedSamples = useMemo<Array<[number, number]>>(() => {
    return stationsRaw.map((row) => {
      const s = row.station.trim()
      const r = row.radius.trim()
      const sn = s === '' ? Number.NaN : Number(s)
      const rn = r === '' ? Number.NaN : Number(r)
      return [
        Number.isFinite(sn) ? sn : Number.NaN,
        Number.isFinite(rn) ? rn : Number.NaN,
      ]
    })
  }, [stationsRaw])
  const stationsValid =
    parsedSamples.length > 0 &&
    parsedSamples.every(
      ([s, r]) =>
        Number.isFinite(s) &&
        s >= 0 &&
        s <= 1 &&
        Number.isFinite(r) &&
        r > 0,
    )

  // Dispatch validity per profile. `perEdge` is mutually exclusive
  // with Linear/Stations today (see ModeSpec doc), so the branches
  // don't overlap.
  const canApply = (() => {
    if (!spec) return false
    if (!selectionValid) return false
    if (spec.perEdge) return allRadiiValid
    if (spec.profile === 'linear') return linearValid
    if (spec.profile === 'stations') return stationsValid
    return valueValid
  })()

  // Publish a live cross-section preview to the viewport. Only fillet
  // and chamfer participate — shell preview is whole-solid and would
  // need a backend round-trip (see Task #85). Per-edge mode is also
  // skipped: a single cross-section can't represent N different radii.
  // The preview clears automatically when the dialog closes via the
  // cleanup return.
  useEffect(() => {
    // Preview is a single uniform-radius cross-section: it can only
    // represent the constant uniform `fillet` and `chamfer` modes.
    // Per-edge constants, Linear (varying along edge), and Stations
    // (per-station table) all have no single radius to draw, and
    // `shell` is whole-solid (Task #85). Anything else clears the
    // preview so a stale shape doesn't linger.
    if (
      !open ||
      !mode ||
      mode === 'shell' ||
      mode === 'fillet-variable' ||
      mode === 'fillet-linear' ||
      mode === 'fillet-stations' ||
      !valueValid
    ) {
      setModifyPreview(null)
      return
    }
    setModifyPreview({ mode, value: parsedValue })
    return () => setModifyPreview(null)
  }, [open, mode, parsedValue, valueValid, setModifyPreview])

  const close = useCallback(() => onOpenChange(false), [onOpenChange])

  const handleApply = useCallback(() => {
    if (!mode || !spec || !canApply) return
    if (mode === 'fillet-variable') {
      onApply({ mode, radii: parsedRadii })
    } else if (mode === 'fillet-linear') {
      onApply({ mode, start: parsedLinearStart, end: parsedLinearEnd })
    } else if (mode === 'fillet-stations') {
      onApply({ mode, samples: parsedSamples })
    } else {
      onApply({ mode, value: parsedValue })
    }
    close()
  }, [
    mode,
    spec,
    canApply,
    parsedValue,
    parsedRadii,
    parsedLinearStart,
    parsedLinearEnd,
    parsedSamples,
    onApply,
    close,
  ])

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

  // Per-edge stepper: nudges a single row's radius without disturbing
  // the others. Same rounding policy as `stepValue`.
  const stepRadius = (edgeId: number, delta: number) => {
    setRadiiRaw((prev) => {
      const raw = (prev.get(edgeId) ?? '').trim()
      const curr = raw === '' ? Number.NaN : Number(raw)
      const base = Number.isFinite(curr) ? curr : spec.defaultValue
      const next = base + delta
      if (next <= 0) return prev
      const rounded = Math.round(next * 10000) / 10000
      const map = new Map(prev)
      map.set(edgeId, String(rounded))
      return map
    })
  }

  // Linear-endpoint stepper. Reused by both fields via a setter
  // parameter — the logic is identical (positive-only, 4 sig-fig
  // rounding) so factoring it keeps the +/- buttons consistent.
  const stepLinear = (
    raw: string,
    setRaw: (s: string) => void,
    delta: number,
  ) => {
    const trimmed = raw.trim()
    const curr = trimmed === '' ? Number.NaN : Number(trimmed)
    const base = Number.isFinite(curr) ? curr : spec.defaultValue
    const next = base + delta
    if (next <= 0) return
    setRaw(String(Math.round(next * 10000) / 10000))
  }

  // Per-station mutators. Append uses a sensible new station value
  // (midpoint between the last row's station and 1, or 0.5 for an
  // empty table) so the user rarely needs to retype the position.
  // Remove is gated to a minimum of one row — the kernel handles a
  // single-station table as a constant radius, but zero stations is
  // a wire-shape error per `fillet_payload::validate_dto`.
  const updateStation = (
    i: number,
    field: 'station' | 'radius',
    value: string,
  ) => {
    setStationsRaw((prev) => {
      const next = prev.slice()
      next[i] = { ...next[i], [field]: value }
      return next
    })
  }
  const addStation = () => {
    setStationsRaw((prev) => {
      const lastStation = prev.length > 0
        ? Number(prev[prev.length - 1].station.trim())
        : Number.NaN
      const seed = Number.isFinite(lastStation)
        ? Math.min(1, (lastStation + 1) / 2)
        : 0.5
      return [
        ...prev,
        { station: String(seed), radius: String(spec.defaultValue) },
      ]
    })
  }
  const removeStation = (i: number) => {
    setStationsRaw((prev) => {
      if (prev.length <= 1) return prev
      const next = prev.slice()
      next.splice(i, 1)
      return next
    })
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

        {/* Single shared radius — `fillet` / `chamfer` / `shell`. */}
        {!spec.perEdge && spec.profile === 'constant' && (
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
        )}

        {/* Linear profile — start + end radii, uniform across edges. */}
        {spec.profile === 'linear' && (
          <Field label="Radius at endpoints">
            <div className="grid grid-cols-2 gap-2">
              {([
                {
                  label: 'Start',
                  raw: linearStartRaw,
                  setRaw: setLinearStartRaw,
                  parsed: parsedLinearStart,
                  refIt: true,
                },
                {
                  label: 'End',
                  raw: linearEndRaw,
                  setRaw: setLinearEndRaw,
                  parsed: parsedLinearEnd,
                  refIt: false,
                },
              ] as const).map((field) => {
                const rowValid = Number.isFinite(field.parsed) && field.parsed > 0
                return (
                  <div key={field.label} className="flex flex-col gap-1">
                    <span className="text-[10px] uppercase tracking-wider text-muted-foreground/70 font-mono">
                      {field.label}
                    </span>
                    <div className="flex items-stretch overflow-hidden rounded-md border border-input focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
                      <input
                        ref={field.refIt ? inputRef : undefined}
                        type="text"
                        inputMode="decimal"
                        value={field.raw}
                        onChange={(e) => field.setRaw(e.target.value)}
                        className={cn(
                          'h-8 min-w-0 flex-1 bg-transparent px-2 font-mono text-[13px] outline-none',
                          !rowValid && 'text-destructive',
                        )}
                        placeholder={String(spec.defaultValue)}
                        aria-label={`${field.label} radius`}
                      />
                      <span className="flex items-center bg-muted px-2 font-mono text-[11px] text-muted-foreground">
                        mm
                      </span>
                      <div className="flex flex-col border-l border-input">
                        <button
                          type="button"
                          onClick={() => stepLinear(field.raw, field.setRaw, spec.step)}
                          className="flex h-4 items-center justify-center px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                          aria-label={`Increase ${field.label.toLowerCase()} radius`}
                        >
                          <ChevronUp className="h-3 w-3" />
                        </button>
                        <button
                          type="button"
                          onClick={() => stepLinear(field.raw, field.setRaw, -spec.step)}
                          className="flex h-4 items-center justify-center border-t border-input px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                          aria-label={`Decrease ${field.label.toLowerCase()} radius`}
                        >
                          <ChevronDown className="h-3 w-3" />
                        </button>
                      </div>
                    </div>
                  </div>
                )
              })}
            </div>
            {!linearValid && (
              <span className="text-[11px] font-mono text-destructive">
                Both endpoints must be positive numbers.
              </span>
            )}
          </Field>
        )}

        {/* Stations profile — table of (station, radius) rows. */}
        {spec.profile === 'stations' && (
          <Field label="Station radii">
            <div className="flex max-h-[240px] flex-col gap-1 overflow-y-auto pr-1">
              <div className="grid grid-cols-[1fr_1fr_auto] items-center gap-1.5 pb-1 text-[10px] uppercase tracking-wider text-muted-foreground/70 font-mono">
                <span>Station (0–1)</span>
                <span>Radius (mm)</span>
                <span className="w-6" />
              </div>
              {stationsRaw.map((row, i) => {
                const [sp, rp] = parsedSamples[i]
                const sValid = Number.isFinite(sp) && sp >= 0 && sp <= 1
                const rValid = Number.isFinite(rp) && rp > 0
                return (
                  <div
                    key={i}
                    className="grid grid-cols-[1fr_1fr_auto] items-stretch gap-1.5"
                  >
                    <div className="flex items-stretch overflow-hidden rounded-md border border-input focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
                      <input
                        ref={i === 0 ? inputRef : undefined}
                        type="text"
                        inputMode="decimal"
                        value={row.station}
                        onChange={(e) => updateStation(i, 'station', e.target.value)}
                        className={cn(
                          'h-8 min-w-0 flex-1 bg-transparent px-2 font-mono text-[13px] outline-none',
                          !sValid && 'text-destructive',
                        )}
                        placeholder="0.5"
                        aria-label={`Station ${i + 1} parameter`}
                      />
                    </div>
                    <div className="flex items-stretch overflow-hidden rounded-md border border-input focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
                      <input
                        type="text"
                        inputMode="decimal"
                        value={row.radius}
                        onChange={(e) => updateStation(i, 'radius', e.target.value)}
                        className={cn(
                          'h-8 min-w-0 flex-1 bg-transparent px-2 font-mono text-[13px] outline-none',
                          !rValid && 'text-destructive',
                        )}
                        placeholder={String(spec.defaultValue)}
                        aria-label={`Station ${i + 1} radius`}
                      />
                    </div>
                    <button
                      type="button"
                      onClick={() => removeStation(i)}
                      disabled={stationsRaw.length <= 1}
                      className={cn(
                        'flex h-8 w-6 items-center justify-center rounded-md border border-border text-muted-foreground transition-colors',
                        stationsRaw.length <= 1
                          ? 'cursor-not-allowed opacity-40'
                          : 'hover:bg-muted hover:text-foreground',
                      )}
                      aria-label={`Remove station ${i + 1}`}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                )
              })}
            </div>
            <button
              type="button"
              onClick={addStation}
              className="mt-1 self-start rounded-md border border-dashed border-border px-2 py-1 text-[11px] font-mono text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
            >
              + Add station
            </button>
            {!stationsValid && (
              <span className="text-[11px] font-mono text-destructive">
                Every station must be in [0, 1] with a positive radius.
              </span>
            )}
          </Field>
        )}

        {spec.perEdge && (
          <Field label={`${spec.inputLabel} per edge`}>
            {pickedEdges.length === 0 ? (
              <span className="text-[11px] font-mono text-muted-foreground">
                Pick edges in the viewport to set their radii.
              </span>
            ) : (
              <div className="flex max-h-[200px] flex-col gap-1 overflow-y-auto pr-1">
                {pickedEdges.map((eid, i) => {
                  const raw = radiiRaw.get(eid) ?? ''
                  const n = parsedRadii[i]
                  const rowValid = Number.isFinite(n) && n > 0
                  return (
                    <div key={eid} className="flex items-stretch gap-1.5">
                      <span className="flex h-8 w-10 shrink-0 items-center justify-center rounded-md border border-border bg-muted/30 font-mono text-[11px] text-muted-foreground">
                        #{eid}
                      </span>
                      <div className="flex flex-1 items-stretch overflow-hidden rounded-md border border-input focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
                        <input
                          ref={i === 0 ? inputRef : undefined}
                          type="text"
                          inputMode="decimal"
                          value={raw}
                          onChange={(e) => {
                            const v = e.target.value
                            setRadiiRaw((prev) => {
                              const map = new Map(prev)
                              map.set(eid, v)
                              return map
                            })
                          }}
                          className={cn(
                            'h-8 min-w-0 flex-1 bg-transparent px-2 font-mono text-[13px] outline-none',
                            !rowValid && 'text-destructive',
                          )}
                          placeholder={String(spec.defaultValue)}
                          aria-label={`${spec.inputLabel} for edge ${eid}`}
                        />
                        <span className="flex items-center bg-muted px-2 font-mono text-[11px] text-muted-foreground">
                          mm
                        </span>
                        <div className="flex flex-col border-l border-input">
                          <button
                            type="button"
                            onClick={() => stepRadius(eid, spec.step)}
                            className="flex h-4 items-center justify-center px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            aria-label={`Increase radius for edge ${eid}`}
                          >
                            <ChevronUp className="h-3 w-3" />
                          </button>
                          <button
                            type="button"
                            onClick={() => stepRadius(eid, -spec.step)}
                            className="flex h-4 items-center justify-center border-t border-input px-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            aria-label={`Decrease radius for edge ${eid}`}
                          >
                            <ChevronDown className="h-3 w-3" />
                          </button>
                        </div>
                      </div>
                    </div>
                  )
                })}
              </div>
            )}
            {pickedEdges.length > 0 && !allRadiiValid && (
              <span className="text-[11px] font-mono text-destructive">
                Every radius must be a positive number.
              </span>
            )}
          </Field>
        )}
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
