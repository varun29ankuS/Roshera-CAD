import { useCallback, useEffect, useRef, useState } from 'react'
import { refreshSceneFromServer } from '@/lib/ws-bridge'
import { entityIssueLine, schemaLabel } from '@/lib/import-report'
import { cn } from '@/lib/utils'
import { importCertificateLine, type ImportPerception } from '@/lib/import-certificate'
import { useBlackboardStore } from '@/stores/blackboard-store'

const API_HOST = import.meta.env.VITE_API_URL || ''

// ─── Backend contract (POST /api/geometry/import_step) ──────────────────────
//
// Request: { content: "ISO-10303-21;…", name?: string }
// Response: { success: bool, objects: [...], report: ImportReport }
//
// `report` mirrors `export_engine::formats::step::ImportReport` verbatim — we
// surface it HONESTLY rather than collapsing it to a green check. The agent
// and the human see the same reconstruct-coverage matrix the kernel produced.

interface EntityCounts {
  resolved: Record<string, number>
  skipped: Record<string, number>
  unsupported: Record<string, number>
  failed: Record<string, number>
}

interface SolidValidation {
  solid_id: number
  valid: boolean
  error_count: number
  errors: string[]
}

interface ImportReport {
  ok: boolean
  counts: EntityCounts
  schema: string | null
  source_unit: string | null
  roots_resolved: number
  solids_in_roots: number
  validation: SolidValidation[]
  unsupported: Array<{ entity: string; instance: number; reason?: string }>
}

interface ImportResponse {
  success: boolean
  objects: Array<{ id: string; name: string; solid_id: number }>
  report: ImportReport
}

interface ApiError {
  // Axum `error_catalog::ApiError` serialises `{ error_code, error, retryable,
  // hint?, details? }` — `error` is the human-readable message, `hint` the
  // optional remediation pointer.
  error?: string
  hint?: string
}

type ActivePhase =
  | { kind: 'loading'; filename: string }
  | { kind: 'done'; filename: string; res: ImportResponse }
  | { kind: 'error'; filename: string; message: string }

type Phase = { kind: 'idle' } | ActivePhase

const STEP_EXT = /\.(step|stp)$/i

function distinctTypes(m: Record<string, number>): number {
  return Object.keys(m).length
}

function sumCounts(m: Record<string, number>): number {
  return Object.values(m).reduce((a, b) => a + b, 0)
}

async function readError(r: Response): Promise<string> {
  try {
    const body = (await r.json()) as ApiError
    if (typeof body.error === 'string') {
      return body.hint ? `${body.error} — ${body.hint}` : body.error
    }
    return `import failed (HTTP ${r.status})`
  } catch {
    return `import failed (HTTP ${r.status})`
  }
}

/**
 * Drag-and-drop STEP importer overlaid on the viewport.
 *
 * Drop any `.step` / `.stp` file (or use the "Import STEP" button) → the file
 * text is POSTed to `/api/geometry/import_step`, which reconstructs a real
 * B-Rep, validates every solid, splices them into the live session model and
 * broadcasts each as an `ObjectCreated` frame (so the viewport updates via the
 * existing WS path). We additionally force a `refreshSceneFromServer()` so the
 * scene reflects the import immediately even if a frame is missed.
 *
 * The result panel shows the HONEST coverage report — N solids VALID/INVALID,
 * the AP schema, "understood X entity types, skipped Y", and connectivity
 * error counts for any invalid solid. Nothing is hidden behind a green check.
 */
export function StepImportDropzone() {
  const [dragging, setDragging] = useState(false)
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const fileInput = useRef<HTMLInputElement | null>(null)
  const rootRef = useRef<HTMLDivElement | null>(null)
  // Nested dragenter/dragleave events (child elements) would otherwise flicker
  // the highlight; track depth so it clears only when the cursor truly leaves.
  const dragDepth = useRef(0)

  const runImport = useCallback(async (file: File) => {
    if (!STEP_EXT.test(file.name)) {
      setPhase({
        kind: 'error',
        filename: file.name,
        message: 'not a STEP file — expected a .step or .stp extension',
      })
      return
    }
    setPhase({ kind: 'loading', filename: file.name })
    try {
      const content = await file.text()
      if (!content.trim().toUpperCase().startsWith('ISO-10303-21')) {
        setPhase({
          kind: 'error',
          filename: file.name,
          message: 'file is not a valid STEP exchange structure (missing ISO-10303-21 header)',
        })
        return
      }
      const name = file.name.replace(STEP_EXT, '')
      const r = await fetch(`${API_HOST}/api/geometry/import_step`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ content, name }),
      })
      if (!r.ok) {
        setPhase({ kind: 'error', filename: file.name, message: await readError(r) })
        return
      }
      const res = (await r.json()) as ImportResponse
      // Reflect the spliced solids immediately (belt-and-suspenders alongside
      // the server's per-solid ObjectCreated broadcast).
      await refreshSceneFromServer()
      setPhase({ kind: 'done', filename: file.name, res })
      // Put the kernel's certificate for each imported solid on the board, so
      // the import's evidence is the kernel's verdict, not "no certificate
      // shown". Read from the route verify_part uses; a read that fails is
      // posted as not-run, never as a pass.
      const solids = res.report.validation
      for (const s of solids) {
        const label = solids.length > 1 ? `${name} #${s.solid_id}` : name
        let perception: ImportPerception | null = null
        try {
          const pr = await fetch(`${API_HOST}/api/agent/parts/${s.solid_id}/perception?full=1`)
          if (pr.ok) perception = (await pr.json()) as ImportPerception
        } catch {
          perception = null
        }
        useBlackboardStore.getState().addLine(importCertificateLine(label, s.solid_id, perception), 'system')
      }
    } catch (e) {
      setPhase({
        kind: 'error',
        filename: file.name,
        message: e instanceof Error ? e.message : String(e),
      })
    }
  }, [])

  // Drag handling is bound at the document level, scoped to this component's
  // bounding box. This keeps the viewport canvas fully interactive at rest (no
  // overlay intercepts orbit/select) while still catching a file dropped
  // anywhere over the viewport. The visual highlight overlay is purely
  // `pointer-events-none` — it never participates in hit-testing.
  useEffect(() => {
    const hasFiles = (e: DragEvent) =>
      !!e.dataTransfer && Array.from(e.dataTransfer.types).includes('Files')

    const overViewport = (e: DragEvent): boolean => {
      const el = rootRef.current?.parentElement
      if (!el) return false
      const r = el.getBoundingClientRect()
      return e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom
    }

    const onEnter = (e: DragEvent) => {
      if (!hasFiles(e) || !overViewport(e)) return
      e.preventDefault()
      dragDepth.current += 1
      setDragging(true)
    }
    const onOver = (e: DragEvent) => {
      if (!hasFiles(e)) return
      if (overViewport(e)) {
        // Required so the browser fires `drop` here rather than opening the file.
        e.preventDefault()
        if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy'
        if (!dragging) setDragging(true)
      }
    }
    const onLeave = (e: DragEvent) => {
      if (!hasFiles(e)) return
      dragDepth.current = Math.max(0, dragDepth.current - 1)
      if (dragDepth.current === 0) setDragging(false)
    }
    const onDropDoc = (e: DragEvent) => {
      if (!hasFiles(e)) return
      const inside = overViewport(e)
      // Always prevent the browser's default "navigate to file" for file drops
      // anywhere, but only import when dropped over the viewport.
      e.preventDefault()
      dragDepth.current = 0
      setDragging(false)
      if (!inside) return
      const file = e.dataTransfer?.files?.[0]
      if (file) void runImport(file)
    }

    document.addEventListener('dragenter', onEnter)
    document.addEventListener('dragover', onOver)
    document.addEventListener('dragleave', onLeave)
    document.addEventListener('drop', onDropDoc)
    return () => {
      document.removeEventListener('dragenter', onEnter)
      document.removeEventListener('dragover', onOver)
      document.removeEventListener('dragleave', onLeave)
      document.removeEventListener('drop', onDropDoc)
    }
  }, [dragging, runImport])

  // The toolbar's Export flyout (Import ▸ STEP) dispatches this event to open the
  // native file picker — the import affordance lives under Export now, not as a
  // floating overlay button. Drag-and-drop onto the viewport still works.
  useEffect(() => {
    const open = () => fileInput.current?.click()
    window.addEventListener('roshera:open-step-import', open)
    return () => window.removeEventListener('roshera:open-step-import', open)
  }, [])

  const onPick = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0]
      if (file) void runImport(file)
      // Allow re-importing the same file (onChange won't fire twice otherwise).
      e.target.value = ''
    },
    [runImport],
  )

  return (
    <>
      {/* Visual drop affordance ONLY — pointer-events-none so it never
          participates in hit-testing. Drag/drop is handled by the document
          listeners above; this just highlights the viewport while a file is
          dragged over it. */}
      <div ref={rootRef} className="pointer-events-none absolute inset-0 z-30">
        {dragging && (
          <div className="flex h-full w-full items-center justify-center bg-primary/10 backdrop-blur-[1px]">
            <div className="rounded-lg border-2 border-dashed border-primary bg-background/90 px-8 py-6 text-center shadow-xl">
              <div className="text-2xl">⤓</div>
              <div className="mt-1 text-sm font-semibold">Drop STEP file to import</div>
              <div className="text-[11px] text-muted-foreground">.step / .stp — reconstructed as a B-Rep</div>
            </div>
          </div>
        )}
      </div>

      {/* Toolbar affordance + hidden file input. Sits top-right, clear of the
          ModelTree (top-left) and the AgentEyePanel (bottom-right). */}
      {/* The import trigger now lives in the toolbar's Export flyout (Import ▸
          STEP); this overlay keeps only the hidden file input + the transient
          result card, so nothing floats over the viewport at rest. */}
      <div className="absolute right-2 top-2 z-30 flex flex-col items-end gap-2">
        <input name="step-import-dropzone-field-1"
          ref={fileInput}
          type="file"
          accept=".step,.stp"
          className="hidden"
          onChange={onPick}
        />

        {phase.kind !== 'idle' && (
          <ImportReportCard phase={phase} onDismiss={() => setPhase({ kind: 'idle' })} />
        )}
      </div>
    </>
  )
}

function ImportReportCard({ phase, onDismiss }: { phase: ActivePhase; onDismiss: () => void }) {
  return (
    <div className="w-[280px] overflow-hidden rounded-md border border-border bg-background/95 shadow-lg backdrop-blur">
      <div className="flex items-center justify-between border-b border-border px-2.5 py-1.5">
        <span className="truncate text-xs font-semibold" title={phase.filename}>
          STEP import · {phase.filename}
        </span>
        <button
          onClick={onDismiss}
          className="ml-2 rounded px-1.5 py-0.5 text-[11px] hover:bg-accent"
          title="Dismiss"
        >
          ✕
        </button>
      </div>

      {phase.kind === 'loading' && (
        <div className="flex items-center gap-2 px-2.5 py-3 text-xs text-muted-foreground">
          <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-caution-wash" />
          Reconstructing B-Rep…
        </div>
      )}

      {phase.kind === 'error' && (
        <div className="px-2.5 py-2.5 text-xs">
          <div className="font-semibold text-red-500">✗ Import failed</div>
          <div className="mt-1 break-words text-muted-foreground">{phase.message}</div>
        </div>
      )}

      {phase.kind === 'done' && <ReportBody res={phase.res} />}
    </div>
  )
}

function ReportBody({ res }: { res: ImportResponse }) {
  const { report } = res
  const solids = report.validation
  const validCount = solids.filter((s) => s.valid).length
  const allValid = solids.length > 0 && validCount === solids.length
  const resolvedTypes = distinctTypes(report.counts.resolved)
  const unsupportedTypes = distinctTypes(report.counts.unsupported)
  const unsupportedTotal = sumCounts(report.counts.unsupported)
  const issues = entityIssueLine(report.counts.failed, report.ok && allValid)

  const clean = report.ok && allValid
  const schema = schemaLabel(report.schema)
  return (
    <div className="px-3 py-2.5 text-xs">
      {/* The verdict first -- driven by report.ok, which folds in per-solid
          kernel validation. We never paint a green check on an invalid import. */}
      <div className={cn('text-sm font-semibold', clean ? 'text-green-600 dark:text-green-500' : 'text-red-500')}>
        {clean
          ? `✓ ${solids.length === 1 ? 'Solid' : `${solids.length} solids`} imported, sound`
          : `⚠ ${solids.length - validCount} of ${solids.length} solid${solids.length === 1 ? '' : 's'} not sound`}
      </div>
      {(schema || report.source_unit) && (
        <div className="mt-0.5 text-muted-foreground">
          {[schema, report.source_unit].filter(Boolean).join(' · ')}
        </div>
      )}

      {/* Per-solid validity -- shown when there is more than one solid or any
          is not sound; each solid is VALID or INVALID. */}
      {(solids.length > 1 || !allValid) && (
      <div className="mt-2 border-t border-border pt-1.5">
        <ul className="space-y-0.5">
          {solids.map((s) => (
            <li key={s.solid_id} className="flex items-start justify-between gap-2">
              <span className={s.valid ? 'text-green-600' : 'text-red-500'}>
                {s.valid ? '✓' : '✗'} solid #{s.solid_id}
              </span>
              {!s.valid && (
                <span className="text-right text-red-500" title={s.errors.join('\n')}>
                  {s.error_count} connectivity error{s.error_count === 1 ? '' : 's'}
                </span>
              )}
            </li>
          ))}
        </ul>
        {/* Surface the first concrete error message for any invalid solid so the
            failure is legible, not just a count. */}
        {solids.some((s) => !s.valid && s.errors.length > 0) && (
          <div className="mt-1 max-h-16 overflow-auto rounded bg-red-500/5 px-1.5 py-1 text-[11px] text-red-500">
            {solids
              .filter((s) => !s.valid && s.errors.length > 0)
              .flatMap((s) => s.errors.slice(0, 2))
              .slice(0, 4)
              .map((m, i) => (
                <div key={i} className="truncate" title={m}>
                  · {m}
                </div>
              ))}
          </div>
        )}
      </div>
      )}

      {/* Coverage -- understood vs skipped, mirroring the backend counts. One
          click away, never removed. */}
      <details className="mt-2 border-t border-border pt-1.5 text-muted-foreground">
        <summary className="cursor-pointer select-none hover:text-foreground">Import details</summary>
        <div className="mt-1 font-mono text-[11px]">
        <div>
          understood <span className="text-foreground">{resolvedTypes}</span> entity type
          {resolvedTypes === 1 ? '' : 's'}
        </div>
        {unsupportedTotal > 0 ? (
          <div className="text-caution">
            skipped {unsupportedTotal} ({unsupportedTypes} type{unsupportedTypes === 1 ? '' : 's'}) — unsupported
          </div>
        ) : (
          <div className="text-green-600">skipped 0 — full entity coverage</div>
        )}
        {issues && (
          <div className={issues.tone === 'caution' ? 'text-caution' : 'text-red-500'} title={issues.title}>
            {issues.text}
          </div>
        )}
        <div className="mt-0.5">
          roots resolved {report.roots_resolved} · solids in roots {report.solids_in_roots}
        </div>
        <div className="mt-0.5">schema {report.schema ?? 'not stated'}</div>
        </div>
      </details>
    </div>
  )
}
