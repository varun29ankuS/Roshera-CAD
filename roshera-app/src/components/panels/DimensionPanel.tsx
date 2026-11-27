/**
 * The drawing tab's dimension property panel, and the sheet's completeness
 * chip.
 *
 * Click a dimension on the sheet and this card opens at the top right of the
 * canvas, the way a property manager does in established CAD. From top to
 * bottom it answers what an engineer asks about one dimension:
 *
 *   Value      what it measures. Measured by the kernel, never typed; when a
 *              recorded parameter drives it, a new size here changes the PART.
 *   Tolerance  none / symmetric / two deviations / limits / ISO 286 fit /
 *              basic / reference. A fit shows its limits from the kernel's
 *              tables before it is applied.
 *   Text       what is written before and after the value: 4X, TYP, THRU, a
 *              thread designation. Written as typed; the certificate lists
 *              it as authored text.
 *   Position   drag the dimension on the sheet; this row puts it back.
 *   Suppress   take it off the sheet, signed. The certificate keeps it as a
 *              waived omission naming who suppressed it.
 *
 * Every change is one `PATCH /api/drawings/{id}/dimensions/{dim}`, one
 * history step (Ctrl+Z undoes it), and is available to an agent through the
 * same route. The card shows the kernel's answer after each change: what is
 * now inked, and whether the sheet is still sound and complete.
 *
 * Layout law: a closable card ON the canvas, no backdrop, the header hugs
 * its content, nothing under 11 px.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { X } from '@/components/ui/icons'
import {
  type Drawing,
  type DimensionInfo,
  type DimensionAnnotation,
  type SheetDimension,
  type DimensionDriverResolution,
  DrawingRefusal,
  annotateDimension,
  iso286Limits,
  type Iso286Limits,
} from '@/lib/drawings-api'
import {
  FIT_GRADES,
  FIT_LETTERS,
  TEXT_CHIPS,
  TOLERANCE_FORMS,
  type CertificateSummary,
  type ToleranceDraft,
  certificateChip,
  draftOf,
  fitDesignation,
  omissionReason,
  textRefusal,
  toleranceOf,
  withChip,
} from '@/lib/dimension-annotate'

const card = 'bg-card border border-border rounded-md shadow-md'
const field =
  'cad-focus min-w-0 font-mono px-2 py-1 rounded border border-border bg-background disabled:opacity-60'
const sectionTitle = 'text-[11px] font-medium uppercase tracking-wide text-muted-foreground'

const reasonOf = (e: unknown): string =>
  e instanceof DrawingRefusal || e instanceof Error ? e.message : String(e)

/** `+0.021` / `0` / `-0.007`: a limit deviation as a drawing writes it. */
function deviation(v: number): string {
  if (Math.abs(v) < 1e-9) return '0'
  const s = Number(Math.abs(v).toFixed(4)).toString()
  return `${v > 0 ? '+' : '−'}${s}`
}

/** Remember who signs suppressions on this machine. */
const SIGNER_KEY = 'roshera.drawing.signer'
function rememberedSigner(drawing: Drawing): string {
  try {
    const saved = window.localStorage.getItem(SIGNER_KEY)
    if (saved && saved.trim()) return saved
  } catch {
    /* storage unavailable: fall through to the title block */
  }
  return drawing.title_block?.drawn_by ?? ''
}
function rememberSigner(name: string) {
  try {
    window.localStorage.setItem(SIGNER_KEY, name)
  } catch {
    /* not remembered; the field is still filled for this session */
  }
}

// ── Completeness chip + omissions card ───────────────────────────────────

const TONE: Record<'good' | 'warn' | 'bad', string> = {
  good: 'text-green-700',
  warn: 'text-amber-700',
  bad: 'text-red-700',
}

/**
 * Bottom-right of the canvas (above the zoom bar): is the sheet sound, and does it state every
 * dimension. Click it to see what is left out, sign for a gap, or put a
 * suppressed dimension back.
 */
export function CompletenessChip({
  drawing,
  certificate,
  onChanged,
}: {
  drawing: Drawing
  certificate: CertificateSummary | null
  onChanged: (note: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [signer, setSigner] = useState(() => rememberedSigner(drawing))
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [refusal, setRefusal] = useState<string | null>(null)
  const chip = certificateChip(certificate)
  const omissions = certificate?.omissions ?? []

  const act = useCallback(
    async (key: string, label: string, waive: boolean) => {
      if (waive && !signer.trim()) {
        setRefusal('Enter your name: a suppression is signed.')
        return
      }
      setBusyKey(key)
      setRefusal(null)
      try {
        await annotateDimension(drawing.id, key, {
          suppressed: waive ? { by: signer.trim(), reason: '' } : null,
        })
        if (waive) rememberSigner(signer.trim())
        onChanged(
          waive
            ? `${label} signed off by ${signer.trim()}. Ctrl+Z undoes it.`
            : `${label} is back on the sheet. Ctrl+Z undoes it.`,
        )
      } catch (e) {
        setRefusal(reasonOf(e))
      } finally {
        setBusyKey(null)
      }
    },
    [drawing.id, signer, onChanged],
  )

  return (
    <div className="absolute bottom-14 right-3 z-20 flex flex-col items-end gap-1 text-xs">
      {open && (
        <div className={`${card} w-[340px] p-3`}>
          <div className="mb-2 flex items-center justify-between">
            <div className="text-sm font-medium">Left off this sheet</div>
            <button
              type="button"
              aria-label="Close"
              onClick={() => setOpen(false)}
              className="cad-focus text-muted-foreground hover:text-foreground"
            >
              <X size={14} />
            </button>
          </div>
          {omissions.length === 0 ? (
            <div className="text-muted-foreground">
              Every dimension the part has is stated on a view.
            </div>
          ) : (
            <>
              <div className="max-h-56 space-y-1 overflow-y-auto pr-1">
                {omissions.map((o, i) => {
                  const k = o.key ?? `${o.view}:${o.label}:${i}`
                  return (
                    <div key={k} className="flex items-start gap-2 rounded border border-border/60 px-2 py-1">
                      <span aria-hidden className={o.waiver ? 'text-muted-foreground' : TONE.warn}>
                        {o.waiver ? '✓' : '!'}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="font-mono">
                          {o.label} <span className="text-muted-foreground">· {o.view}</span>
                        </div>
                        <div className="text-[11px] leading-snug text-muted-foreground">
                          {o.waiver
                            ? `Suppressed by ${o.waiver.by}${o.waiver.reason ? `: ${o.waiver.reason}` : ''}`
                            : omissionReason(o.reason)}
                        </div>
                      </div>
                      {o.key ? (
                        <button
                          type="button"
                          disabled={busyKey !== null}
                          onClick={() => void act(o.key as string, o.label, !o.waiver)}
                          className="cad-focus shrink-0 rounded border border-border px-2 py-0.5 hover:bg-accent/20 disabled:opacity-60"
                        >
                          {o.waiver ? 'Restore' : 'Sign off'}
                        </button>
                      ) : (
                        <span
                          className="shrink-0 text-[11px] text-muted-foreground"
                          title="Its faces carry no persistent id, so a sign-off could not be recorded against it."
                        >
                          cannot be signed
                        </span>
                      )}
                    </div>
                  )
                })}
              </div>
              <label className="mt-2 flex items-center gap-2">
                <span className="shrink-0 text-muted-foreground">Signed by</span>
                <input name="your-name"
                  value={signer}
                  onChange={(e) => setSigner(e.target.value)}
                  placeholder="your name"
                  className={`${field} flex-1 font-sans`}
                />
              </label>
              <div className="mt-1 text-[11px] leading-snug text-muted-foreground">
                Sign off keeps a dimension off the sheet on your name. The certificate lists it as waived.
              </div>
            </>
          )}
          {refusal && (
            <div className="mt-2 flex gap-1 text-red-700">
              <span aria-hidden>✗</span>
              <span>{refusal}</span>
            </div>
          )}
        </div>
      )}
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        title="What the sheet certificate says: is every value true, and is every dimension stated"
        className={`${card} cad-focus flex items-center gap-1.5 px-2.5 py-1.5 hover:bg-accent/20`}
      >
        <span aria-hidden className={`font-bold ${TONE[chip.tone]}`}>
          {chip.mark}
        </span>
        <span>{chip.text}</span>
      </button>
    </div>
  )
}

// ── Dimension property panel ─────────────────────────────────────────────

export interface DimensionPanelProps {
  drawing: Drawing
  dim: SheetDimension
  /** The view the dimension is drawn in, e.g. "TOP". */
  view: string
  /** What kind of dimension, in a drafter's words ("diameter"). */
  kindWords: string
  /** The kernel's reading of this dimension (ink, edit); null while loading. */
  info: DimensionInfo | null
  certificate: CertificateSummary | null
  // The value editor (it changes the part): state owned by the workspace.
  driver: DimensionDriverResolution | null
  driverLine: React.ReactNode
  draft: string
  busy: boolean
  startedAt: number | null
  valueRefusal: string | null
  onDraft: (v: string) => void
  onApplyValue: () => void
  onStop: () => void
  onClose: () => void
  /** An annotation was recorded: refresh the sheet and say `note`. */
  onChanged: (note: string) => void
}

export function DimensionPanel(p: DimensionPanelProps) {
  const { drawing, dim, info } = p
  const edit = info?.edit ?? null
  const editable = info ? info.editable : true

  // Drafts are seeded from the kernel's record and re-seeded whenever it
  // changes (an apply, an undo, an agent's edit).
  const seed = JSON.stringify(edit)
  const [tol, setTol] = useState<ToleranceDraft>(() => draftOf(edit?.tolerance))
  const [prefix, setPrefix] = useState(edit?.prefix ?? '')
  const [suffix, setSuffix] = useState(edit?.suffix ?? '')
  const [signer, setSigner] = useState(() => rememberedSigner(drawing))
  const [reason, setReason] = useState('')
  const [suppressOpen, setSuppressOpen] = useState(false)
  const [saving, setSaving] = useState(false)
  const [refusal, setRefusal] = useState<string | null>(null)
  useEffect(() => {
    const e = seed === 'null' ? null : (JSON.parse(seed) as DimensionInfo['edit'])
    setTol(draftOf(e?.tolerance))
    setPrefix(e?.prefix ?? '')
    setSuffix(e?.suffix ?? '')
    setRefusal(null)
  }, [seed, dim.id])

  const send = useCallback(
    async (change: DimensionAnnotation, note: string) => {
      setSaving(true)
      setRefusal(null)
      try {
        const r = await annotateDimension(drawing.id, dim.id, change)
        const ink = typeof r.dimension.ink === 'string' ? r.dimension.ink : dim.label
        p.onChanged(note.replace('{ink}', ink))
        return true
      } catch (e) {
        setRefusal(reasonOf(e))
        return false
      } finally {
        setSaving(false)
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [drawing.id, dim.id, dim.label, p.onChanged],
  )

  // ── Tolerance ──────────────────────────────────────────────────────────
  const applyTolerance = useCallback(() => {
    const t = toleranceOf(tol)
    if (!t.ok) {
      setRefusal(t.reason)
      return
    }
    void send(
      { tolerance: t.tolerance },
      t.tolerance ? 'Tolerance set: {ink}. Ctrl+Z undoes it.' : 'Tolerance removed: {ink}. Ctrl+Z undoes it.',
    )
  }, [tol, send])

  // A fit's limits, from the kernel's tables, while it is being chosen.
  const designation = tol.form === 'fit' ? fitDesignation(tol) : null
  const [fit, setFit] = useState<{ limits: Iso286Limits | null; refusal: string | null } | null>(null)
  useEffect(() => {
    if (!designation) {
      setFit(null)
      return
    }
    const abort = new AbortController()
    setFit(null)
    const t = window.setTimeout(() => {
      iso286Limits(dim.value, designation, abort.signal)
        .then((limits) => setFit({ limits, refusal: null }))
        .catch((e) => {
          if (e instanceof DOMException && e.name === 'AbortError') return
          setFit({ limits: null, refusal: reasonOf(e) })
        })
    }, 150)
    return () => {
      window.clearTimeout(t)
      abort.abort()
    }
  }, [designation, dim.value])

  // ── Text ───────────────────────────────────────────────────────────────
  const lastText = useRef<'prefix' | 'suffix'>('suffix')
  const textDirty = prefix !== (edit?.prefix ?? '') || suffix !== (edit?.suffix ?? '')
  const applyText = useCallback(() => {
    const bad = textRefusal(prefix) ?? textRefusal(suffix)
    if (bad) {
      setRefusal(bad)
      return
    }
    void send({ prefix: prefix.trim(), suffix: suffix.trim() }, 'Text set: {ink}. Ctrl+Z undoes it.')
  }, [prefix, suffix, send])

  // ── Suppress ───────────────────────────────────────────────────────────
  const suppress = useCallback(async () => {
    if (!signer.trim()) {
      setRefusal('Enter your name: a suppression is signed.')
      return
    }
    rememberSigner(signer.trim())
    const done = await send(
      { suppressed: { by: signer.trim(), reason: reason.trim() } },
      `${dim.label} suppressed by ${signer.trim()}. The certificate lists it as waived. Ctrl+Z undoes it.`,
    )
    if (done) p.onClose()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signer, reason, send, dim.label, p.onClose])

  const moved = !!edit?.text_shift_mm
  const driven = p.driver?.verdict === 'driven'
  const chip = useMemo(() => certificateChip(p.certificate), [p.certificate])
  const tolDirty = JSON.stringify(tol) !== JSON.stringify(draftOf(edit?.tolerance))
  const locked = saving || p.busy || !editable

  return (
    <div
      className={`${card} absolute top-14 right-3 z-30 flex max-h-[calc(100%-7.5rem)] w-[320px] flex-col text-xs`}
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          p.onStop()
          p.onClose()
        }
      }}
    >
      <div className="flex items-start justify-between gap-2 px-3 pt-3">
        <div className="min-w-0">
          <div className="truncate font-mono text-sm font-medium" title={info?.ink ?? dim.label}>
            {info?.ink ?? dim.label}
          </div>
          <div className="text-muted-foreground">
            {p.kindWords} · {p.view} view
          </div>
        </div>
        <button
          type="button"
          aria-label="Close"
          onClick={() => {
            p.onStop()
            p.onClose()
          }}
          className="cad-focus shrink-0 text-muted-foreground hover:text-foreground"
        >
          <X size={14} />
        </button>
      </div>

      <div className="mt-2 flex-1 space-y-3 overflow-y-auto px-3 pb-3">
        {/* ── Value ─────────────────────────────────────────────────── */}
        <section>
          <div className="mb-1 flex items-center justify-between">
            <span className={sectionTitle}>Value</span>
            <span
              className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${
                driven ? 'bg-sky-600/15 text-sky-700' : 'bg-muted text-muted-foreground'
              }`}
              title={
                driven
                  ? 'A recorded parameter drives this dimension: a new size changes the part.'
                  : 'Measured from the part. Nothing recorded drives it, so it cannot be changed from the sheet.'
              }
            >
              {p.driver === null ? 'Measured' : driven ? 'Drives the part' : 'Measured, reference'}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <input name="value"
              inputMode="decimal"
              value={driven ? p.draft : String(Number(dim.value.toFixed(4)))}
              readOnly={!driven}
              disabled={p.busy}
              onChange={(e) => p.onDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && driven) {
                  e.preventDefault()
                  p.onApplyValue()
                }
              }}
              aria-label="Value"
              className={`${field} w-28 ${driven ? '' : 'bg-muted/40 text-muted-foreground'}`}
            />
            <span className="text-muted-foreground">{dim.unit}</span>
            {driven &&
              (p.busy ? (
                <button
                  type="button"
                  onClick={p.onStop}
                  className="cad-focus ml-auto shrink-0 rounded border border-border px-2 py-1 hover:bg-accent/20"
                >
                  Stop
                </button>
              ) : (
                <button
                  type="button"
                  onClick={p.onApplyValue}
                  className="cad-focus ml-auto shrink-0 rounded bg-primary px-3 py-1 text-primary-foreground hover:opacity-90"
                >
                  Change part
                </button>
              ))}
          </div>
          <div className="mt-1 text-[11px] leading-snug">{p.driverLine}</div>
          {p.busy && p.startedAt !== null && (
            <div className="mt-1 text-muted-foreground">
              Rebuilding the part and its sheet…{' '}
              <span className="font-mono">{Math.floor((Date.now() - p.startedAt) / 1000)} s</span>
            </div>
          )}
          {p.valueRefusal && (
            <div className="mt-1 flex gap-1 text-red-700">
              <span aria-hidden>✗</span>
              <span>{p.valueRefusal}</span>
            </div>
          )}
        </section>

        {!editable && (
          <div className="flex gap-1 rounded border border-amber-500/40 bg-amber-500/10 px-2 py-1 text-amber-800">
            <span aria-hidden>!</span>
            <span>
              This dimension has no persistent identity (its faces carry none), so a tolerance or text on it could
              not be kept through a rebuild. It cannot be annotated.
            </span>
          </div>
        )}

        {/* ── Tolerance ─────────────────────────────────────────────── */}
        <section>
          <div className={`mb-1 ${sectionTitle}`}>Tolerance</div>
          <div role="radiogroup" aria-label="Tolerance type" className="flex overflow-hidden rounded border border-border">
            {TOLERANCE_FORMS.map((f) => (
              <button
                key={f.key}
                type="button"
                role="radio"
                aria-checked={tol.form === f.key}
                title={f.hint}
                disabled={locked}
                onClick={() => {
                  setTol({ ...tol, form: f.key })
                  setRefusal(null)
                }}
                className={`cad-focus flex-1 px-1 py-1 text-[11px] disabled:opacity-60 ${
                  tol.form === f.key ? 'bg-accent/50 font-medium' : 'hover:bg-accent/20'
                }`}
              >
                {f.label}
              </button>
            ))}
          </div>
          <div className="mt-2 space-y-1.5">
            {tol.form === 'symmetric' && (
              <label className="flex items-center gap-2">
                <span className="w-16 shrink-0 text-muted-foreground">± value</span>
                <input name="0-1"
                  autoFocus
                  inputMode="decimal"
                  value={tol.pm}
                  disabled={locked}
                  placeholder="0.1"
                  onChange={(e) => setTol({ ...tol, pm: e.target.value })}
                  onKeyDown={(e) => e.key === 'Enter' && applyTolerance()}
                  className={`${field} w-24`}
                />
                <span className="text-muted-foreground">{dim.unit}</span>
              </label>
            )}
            {(tol.form === 'bilateral' || tol.form === 'limits') && (
              <>
                {(['upper', 'lower'] as const).map((k) => (
                  <label key={k} className="flex items-center gap-2">
                    <span className="w-16 shrink-0 text-muted-foreground">
                      {k === 'upper' ? 'Upper' : 'Lower'}
                    </span>
                    <input name="dimension-panel-field-1"
                      autoFocus={k === 'upper'}
                      inputMode="decimal"
                      value={tol[k]}
                      disabled={locked}
                      placeholder={
                        tol.form === 'bilateral'
                          ? k === 'upper'
                            ? '+0.05'
                            : '-0.10'
                          : String(Number((dim.value + (k === 'upper' ? 0.15 : -0.15)).toFixed(2)))
                      }
                      onChange={(e) => setTol({ ...tol, [k]: e.target.value })}
                      onKeyDown={(e) => e.key === 'Enter' && applyTolerance()}
                      className={`${field} w-24`}
                    />
                    <span className="text-muted-foreground">
                      {dim.unit}
                      {tol.form === 'bilateral' ? ' deviation' : ' limit'}
                    </span>
                  </label>
                ))}
              </>
            )}
            {tol.form === 'fit' && (
              <>
                <div className="flex items-center gap-2">
                  <div role="radiogroup" aria-label="Hole or shaft" className="flex overflow-hidden rounded border border-border">
                    {(['hole', 'shaft'] as const).map((k) => (
                      <button
                        key={k}
                        type="button"
                        role="radio"
                        aria-checked={tol.fitKind === k}
                        disabled={locked}
                        onClick={() => setTol({ ...tol, fitKind: k })}
                        className={`cad-focus px-2 py-1 ${tol.fitKind === k ? 'bg-accent/50 font-medium' : 'hover:bg-accent/20'}`}
                      >
                        {k === 'hole' ? 'Hole' : 'Shaft'}
                      </button>
                    ))}
                  </div>
                  <select name="fundamental-deviation"
                    aria-label="Fundamental deviation"
                    value={tol.fitLetter}
                    disabled={locked}
                    onChange={(e) => setTol({ ...tol, fitLetter: e.target.value })}
                    className={`${field} py-[5px]`}
                  >
                    {FIT_LETTERS.map((l) => (
                      <option key={l} value={l}>
                        {tol.fitKind === 'hole' ? l.toUpperCase() : l}
                      </option>
                    ))}
                  </select>
                  <select name="tolerance-grade"
                    aria-label="Tolerance grade"
                    value={tol.fitGrade}
                    disabled={locked}
                    onChange={(e) => setTol({ ...tol, fitGrade: e.target.value })}
                    className={`${field} py-[5px]`}
                  >
                    {FIT_GRADES.map((g) => (
                      <option key={g} value={g}>
                        {g}
                      </option>
                    ))}
                  </select>
                  <span className="font-mono font-medium">{designation}</span>
                </div>
                <div className="min-h-[2.2em] text-[11px] leading-snug">
                  {fit === null ? (
                    <span className="text-muted-foreground">Reading ISO 286…</span>
                  ) : fit.limits ? (
                    <span className="font-mono">
                      {deviation(fit.limits.upper_deviation_mm)} / {deviation(fit.limits.lower_deviation_mm)}
                      <span className="text-muted-foreground">
                        {' '}
                        → {fit.limits.lower_limit_mm.toFixed(3)} … {fit.limits.upper_limit_mm.toFixed(3)} mm
                      </span>
                    </span>
                  ) : (
                    <span className="flex gap-1 text-amber-800">
                      <span aria-hidden>!</span>
                      <span>{fit.refusal}</span>
                    </span>
                  )}
                </div>
              </>
            )}
            {tol.form === 'basic' && (
              <div className="text-[11px] text-muted-foreground">
                Theoretically exact: drawn in a box, toleranced by a geometric tolerance elsewhere.
              </div>
            )}
            {tol.form === 'reference' && (
              <div className="text-[11px] text-muted-foreground">
                For information only: drawn in parentheses, not to be inspected.
              </div>
            )}
            {tol.form === 'none' &&
              (info?.tolerance?.feature_pid && !edit?.tolerance ? (
                <div className="text-[11px] text-muted-foreground">
                  Shown with the size tolerance authored on its feature. A tolerance set here takes its place on
                  this sheet.
                </div>
              ) : (
                <div className="text-[11px] text-muted-foreground">
                  The sheet's general tolerance applies ({drawing.tolerance_note ?? 'see the notes'}).
                </div>
              ))}
          </div>
          <div className="mt-2 flex justify-end">
            <button
              type="button"
              disabled={locked || !tolDirty || (tol.form === 'fit' && !fit?.limits)}
              onClick={applyTolerance}
              className="cad-focus rounded bg-primary px-3 py-1 text-primary-foreground hover:opacity-90 disabled:opacity-40"
            >
              {tol.form === 'none' ? 'Remove tolerance' : 'Apply tolerance'}
            </button>
          </div>
        </section>

        {/* ── Text ──────────────────────────────────────────────────── */}
        <section>
          <div className={`mb-1 ${sectionTitle}`}>Dimension text</div>
          <div className="flex items-center gap-1 font-mono">
            <input name="text-before-the-value"
              aria-label="Text before the value"
              value={prefix}
              disabled={locked}
              placeholder="4X"
              onFocus={() => (lastText.current = 'prefix')}
              onChange={(e) => setPrefix(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && applyText()}
              className={`${field} w-[72px]`}
            />
            <span
              className="shrink-0 rounded bg-muted px-1.5 py-1 text-muted-foreground"
              title="The measured value: it cannot be typed"
            >
              {dim.label}
            </span>
            <input name="text-after-the-value"
              aria-label="Text after the value"
              value={suffix}
              disabled={locked}
              placeholder="TYP"
              onFocus={() => (lastText.current = 'suffix')}
              onChange={(e) => setSuffix(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && applyText()}
              className={`${field} flex-1`}
            />
          </div>
          <div className="mt-1.5 flex flex-wrap gap-1">
            {TEXT_CHIPS.map((c) => (
              <button
                key={c.label}
                type="button"
                title={c.title}
                disabled={locked}
                onClick={() =>
                  lastText.current === 'prefix'
                    ? setPrefix((t) => withChip(t, c.insert))
                    : setSuffix((t) => withChip(t, c.insert))
                }
                className="cad-focus min-w-[26px] rounded border border-border px-1.5 py-0.5 font-mono text-[11px] hover:bg-accent/20 disabled:opacity-60"
              >
                {c.label}
              </button>
            ))}
          </div>
          <div className="mt-1.5 flex items-center justify-between gap-2">
            <span className="text-[11px] leading-snug text-muted-foreground">
              Written as typed. The certificate lists it as authored text.
            </span>
            <button
              type="button"
              disabled={locked || !textDirty}
              onClick={applyText}
              className="cad-focus shrink-0 rounded bg-primary px-3 py-1 text-primary-foreground hover:opacity-90 disabled:opacity-40"
            >
              Apply text
            </button>
          </div>
        </section>

        {/* ── Position + suppress ───────────────────────────────────── */}
        <section>
          <div className={`mb-1 ${sectionTitle}`}>On the sheet</div>
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted-foreground">
              {moved
                ? `Moved ${edit?.text_shift_mm?.[0] ?? 0}, ${edit?.text_shift_mm?.[1] ?? 0} mm`
                : 'Drag the dimension to move it.'}
            </span>
            <button
              type="button"
              disabled={locked || !moved}
              onClick={() => void send({ text_shift_mm: null }, `${dim.label} is back where the layout places it.`)}
              className="cad-focus shrink-0 rounded border border-border px-2 py-1 hover:bg-accent/20 disabled:opacity-40"
            >
              Reset position
            </button>
          </div>
          {suppressOpen ? (
            <div className="mt-2 space-y-1.5 rounded border border-border p-2">
              <div className="text-[11px] leading-snug text-muted-foreground">
                The dimension leaves the sheet. The certificate keeps it as a waived omission on your name.
              </div>
              <label className="flex items-center gap-2">
                <span className="w-16 shrink-0 text-muted-foreground">Signed by</span>
                <input name="dimension-panel-field-2"
                  autoFocus
                  value={signer}
                  disabled={locked}
                  onChange={(e) => setSigner(e.target.value)}
                  className={`${field} flex-1 font-sans`}
                />
              </label>
              <label className="flex items-center gap-2">
                <span className="w-16 shrink-0 text-muted-foreground">Reason</span>
                <input name="optional"
                  value={reason}
                  disabled={locked}
                  placeholder="optional"
                  onChange={(e) => setReason(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && void suppress()}
                  className={`${field} flex-1 font-sans`}
                />
              </label>
              <div className="flex justify-end gap-2">
                <button
                  type="button"
                  onClick={() => setSuppressOpen(false)}
                  className="cad-focus rounded border border-border px-2 py-1 hover:bg-accent/20"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  disabled={locked}
                  onClick={() => void suppress()}
                  className="cad-focus rounded bg-red-600 px-3 py-1 text-white hover:opacity-90 disabled:opacity-40"
                >
                  Suppress
                </button>
              </div>
            </div>
          ) : (
            <button
              type="button"
              disabled={locked}
              onClick={() => setSuppressOpen(true)}
              className="cad-focus mt-2 rounded border border-border px-2 py-1 hover:bg-accent/20 disabled:opacity-40"
            >
              Suppress dimension…
            </button>
          )}
        </section>

        {info?.unresolved && (
          <div className="flex gap-1 text-amber-800">
            <span aria-hidden>!</span>
            <span>{info.unresolved}</span>
          </div>
        )}
        {refusal && (
          <div role="alert" className="flex gap-1 text-red-700">
            <span aria-hidden>✗</span>
            <span>{refusal}</span>
          </div>
        )}
      </div>

      <div className="flex items-center gap-1.5 border-t border-border px-3 py-1.5">
        <span aria-hidden className={`font-bold ${TONE[chip.tone]}`}>
          {chip.mark}
        </span>
        <span className="text-muted-foreground">{chip.text}</span>
      </div>
    </div>
  )
}
