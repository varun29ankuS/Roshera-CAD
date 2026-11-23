import { useEffect, useMemo, useState } from 'react'
import { Check, CircleHelp, Play, Plus, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useSimStore } from '@/stores/sim-store'
import { verifyPart } from '@/lib/dfm'
import { faceNormal } from '@/lib/face-normal'
import {
  LOAD_DIRECTIONS,
  SUPPORT_KINDS,
  deformScale,
  engineLabel,
  fetchLatest,
  fieldRange,
  runStatic,
  setupOf,
  setupGaps,
  stressColor,
  studyOf,
  verdictView,
  type LoadDirection,
  type SimState,
  type Spread,
  type SupportKind,
} from '@/lib/sim'
import { LOAD_COLOR, SUPPORT_COLOR } from '@/components/viewport/SimView'

const STATE_STYLE: Record<SimState, { mark: string; text: string; wash: string }> = {
  fix: { mark: 'text-red-600 dark:text-red-400', text: 'text-red-700 dark:text-red-300', wash: 'bg-red-500/10' },
  unknown: { mark: 'text-amber-600 dark:text-amber-400', text: 'text-amber-700 dark:text-amber-300', wash: 'bg-amber-500/12' },
  pass: { mark: 'text-emerald-600 dark:text-emerald-400', text: 'text-emerald-700 dark:text-emerald-300', wash: 'bg-emerald-500/10' },
}

function Mark({ state }: { state: SimState }) {
  const Icon = state === 'fix' ? X : state === 'unknown' ? CircleHelp : Check
  return <Icon size={13} className={cn('shrink-0', STATE_STYLE[state].mark)} aria-hidden />
}

const solidOf = (o: CADObject) => o.analyticalGeometry?.solidId

const field = 'cad-focus h-7 rounded border border-border/70 bg-background/70 px-1.5'

type Phase =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'unverified'; reason: string }
  | { kind: 'can_move'; motions: string[] }
  | { kind: 'engine_missing'; reason: string }
  | { kind: 'refused'; reason: string }

/** A numbered step heading, ticked once the step is complete. */
function Step({ n, title, done, children }: { n: number; title: string; done: boolean; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5 border-b border-border/70 px-3 py-2.5">
      <div className="flex items-center gap-2">
        <span
          className={cn(
            'flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold',
            done ? 'bg-emerald-600 text-white' : 'border border-border text-muted-foreground',
          )}
        >
          {done ? <Check size={10} aria-hidden /> : n}
        </span>
        <span className="font-semibold">{title}</span>
      </div>
      {children}
    </div>
  )
}

/**
 * The Simulate workspace's panel: a linear static study, one step at a
 * time -- the part, how it is held, what loads it, what it is made of, what
 * it must achieve -- then the kernel's verdict with the evidence for it.
 * Faces are picked on the part (face selection is on while this panel is
 * open) and added to a step with one button. Nothing is assumed for the
 * person: a material is theirs to give, and no safety factor is shown
 * without a yield strength and where it comes from.
 */
export function SimPanel() {
  const objects = useSceneStore((s) => s.objects)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const picks = useSceneStore((s) => s.subElementSelections)
  const clearPicks = useSceneStore((s) => s.clearSubElementSelections)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)

  const sim = useSimStore()
  const { setup, report } = sim

  // Faces are what a study is set up on: face picking while this is open.
  useEffect(() => {
    const was = useSceneStore.getState().selectionMode
    setSelectionMode('face')
    return () => setSelectionMode(was)
  }, [setSelectionMode])

  const parts = useMemo(
    () => [...objects.values()].filter((o) => typeof solidOf(o) === 'number' && o.visible !== false),
    [objects],
  )
  const fallback = (parts.find((p) => selectedIds.has(p.id)) ?? parts[0])?.id ?? null
  const partId = sim.objectId && objects.has(sim.objectId) ? sim.objectId : fallback
  const part = partId ? objects.get(partId) : undefined
  const { setPart } = sim
  useEffect(() => {
    if (partId !== sim.objectId) setPart(partId)
  }, [partId, sim.objectId, setPart])

  const picked = useMemo(
    () => [...new Set(picks.filter((p) => p.type === 'face' && p.objectId === partId).map((p) => p.index))],
    [picks, partId],
  )

  // A study run elsewhere -- by an agent through `sim_static` -- is shown
  // here as it lands: its part, its set-up and its result on the part. A
  // study on a part that has changed since is not shown (its numbers belong
  // to a shape that no longer exists).
  const { loadRun, markSeen } = sim
  useEffect(() => {
    let stop = false
    const tick = async () => {
      for (const p of parts) {
        const sid = solidOf(p)
        if (typeof sid !== 'number') continue
        const rec = await fetchLatest(sid).catch(() => null)
        if (stop) return
        if (!rec || rec.stale || useSimStore.getState().seenRuns.has(rec.run_id)) continue
        loadRun(rec.run_id, p.id, setupOf(rec.study), rec.report)
      }
    }
    void tick()
    const timer = setInterval(() => void tick(), 3000)
    return () => {
      stop = true
      clearInterval(timer)
    }
  }, [parts, loadRun])

  const [supportKind, setSupportKind] = useState<SupportKind>('fixed')
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })

  const addSupport = () => {
    if (picked.length === 0) return
    sim.addSupport({ kind: supportKind, faces: picked })
    clearPicks()
  }
  const addLoad = () => {
    if (picked.length === 0 || !partId) return
    const mesh = meshRefs.get(partId)
    const ids = objects.get(partId)?.mesh.faceIds
    const normal = mesh && ids ? faceNormal(mesh, ids, new Set(picked)) : null
    sim.addLoad({ faces: picked, direction: normal ? 'into' : '-z', newtons: 100, spread: 'even', normal })
    clearPicks()
  }

  const gaps = setupGaps(setup)
  const study = studyOf(setup)

  const run = async (verifyFirst: boolean) => {
    const solidId = part ? solidOf(part) : undefined
    if (!part || typeof solidId !== 'number' || !study) return
    setPhase({ kind: 'running' })
    try {
      if (verifyFirst && !(await verifyPart(solidId))) {
        setPhase({ kind: 'refused', reason: 'The part could not be verified.' })
        return
      }
      const out = await runStatic(solidId, study)
      if (out.kind === 'report') {
        sim.setReport(out.report)
        setPhase({ kind: 'idle' })
        // This run is already on screen with the set-up as entered.
        const rec = await fetchLatest(solidId).catch(() => null)
        if (rec) markSeen(rec.run_id)
      } else if (out.kind === 'can_move') {
        setPhase({ kind: 'can_move', motions: out.motions })
      } else {
        setPhase(out)
      }
    } catch (e) {
      setPhase({ kind: 'refused', reason: e instanceof Error ? e.message : String(e) })
    }
  }

  const supportsDone = setup.supports.some((s) => s.faces.length > 0)
  const loadsDone = setup.loads.length > 0 && setup.loads.every((l) => l.newtons > 0)
  const m = setup.material
  const materialDone = m.name.trim() !== '' && Number(m.youngs_modulus_mpa) > 0 && Number(m.poisson_ratio) > 0
  const goalDone = Number(setup.minSafetyFactor) > 0

  return (
    <section aria-label="Simulate" className="flex h-full min-h-0 flex-col text-xs">
      <div className="border-b border-border/70 px-3 py-2">
        <div className="text-[13px] font-semibold">Static strength</div>
        <div className="text-[11px] text-muted-foreground">Will the part hold its loads?</div>
      </div>

      {report ? (
        <Results onEdit={() => sim.setReport(null)} />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <Step n={1} title="Part" done={!!part}>
            <select
              className={cn(field, 'min-w-0')}
              value={partId ?? ''}
              onChange={(e) => sim.setPart(e.target.value)}
              disabled={parts.length === 0}
              aria-label="Part"
            >
              {parts.length === 0 && <option value="">No parts</option>}
              {parts.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </Step>

          <Step n={2} title="How it is held" done={supportsDone}>
            {setup.supports.map((s, i) => (
              <div
                key={i}
                className={cn(
                  'flex items-center gap-1.5 rounded px-1 py-0.5',
                  sim.focus?.kind === 'support' && sim.focus.index === i && 'bg-accent/60',
                )}
                onMouseEnter={() => sim.setFocus({ kind: 'support', index: i })}
                onMouseLeave={() => sim.setFocus(null)}
              >
                <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: SUPPORT_COLOR }} aria-hidden />
                <select
                  className={cn(field, 'h-6 flex-1')}
                  value={s.kind}
                  onChange={(e) => sim.updateSupport(i, { kind: e.target.value as SupportKind })}
                  aria-label="Support kind"
                >
                  {SUPPORT_KINDS.map((k) => (
                    <option key={k.id} value={k.id}>
                      {k.label}
                      {k.solved ? '' : ' (motion check only)'}
                    </option>
                  ))}
                </select>
                <span className="text-muted-foreground">
                  {s.faces.length} face{s.faces.length === 1 ? '' : 's'}
                </span>
                <button type="button" className="cad-focus p-0.5 text-muted-foreground hover:text-foreground" onClick={() => sim.removeSupport(i)} aria-label="Remove support">
                  <Trash2 size={12} />
                </button>
              </div>
            ))}
            <div className="text-[11px] text-muted-foreground">
              {SUPPORT_KINDS.find((k) => k.id === supportKind)?.hint}
            </div>
            <div className="flex items-center gap-1.5">
              <select className={cn(field, 'h-6 w-24')} value={supportKind} onChange={(e) => setSupportKind(e.target.value as SupportKind)} aria-label="New support kind">
                {SUPPORT_KINDS.map((k) => (
                  <option key={k.id} value={k.id}>
                    {k.label}
                  </option>
                ))}
              </select>
              <button
                type="button"
                disabled={picked.length === 0}
                onClick={addSupport}
                className="cad-focus flex h-6 flex-1 items-center justify-center gap-1 rounded border border-border bg-background font-medium disabled:opacity-50"
              >
                <Plus size={11} aria-hidden />
                {picked.length === 0 ? 'Pick faces on the part' : `Hold ${picked.length} picked face${picked.length === 1 ? '' : 's'}`}
              </button>
            </div>
          </Step>

          <Step n={3} title="What loads it" done={loadsDone}>
            {setup.loads.map((l, i) => (
              <div
                key={i}
                className={cn(
                  'flex flex-col gap-1 rounded px-1 py-1',
                  sim.focus?.kind === 'load' && sim.focus.index === i && 'bg-accent/60',
                )}
                onMouseEnter={() => sim.setFocus({ kind: 'load', index: i })}
                onMouseLeave={() => sim.setFocus(null)}
              >
                <div className="flex items-center gap-1.5">
                  <span className="h-2.5 w-2.5 shrink-0 rounded-sm" style={{ background: LOAD_COLOR }} aria-hidden />
                  <input
                    className={cn(field, 'h-6 w-16 font-mono')}
                    value={Number.isFinite(l.newtons) ? String(l.newtons) : ''}
                    onChange={(e) => {
                      const n = Number(e.target.value)
                      const g = l.given
                      const k = g && l.newtons > 0 ? n / l.newtons : 0
                      sim.updateLoad(i, g ? { newtons: n, given: [g[0] * k, g[1] * k, g[2] * k] } : { newtons: n })
                    }}
                    inputMode="decimal"
                    aria-label="Force (N)"
                  />
                  <span className="text-muted-foreground">N</span>
                  <span className="flex-1 text-right text-muted-foreground">
                    {l.faces.length} face{l.faces.length === 1 ? '' : 's'}
                  </span>
                  <button type="button" className="cad-focus p-0.5 text-muted-foreground hover:text-foreground" onClick={() => sim.removeLoad(i)} aria-label="Remove load">
                    <Trash2 size={12} />
                  </button>
                </div>
                <div className="flex items-center gap-1.5 pl-4">
                  <select
                    className={cn(field, 'h-6 flex-1')}
                    value={l.direction}
                    onChange={(e) => sim.updateLoad(i, { direction: e.target.value as LoadDirection })}
                    aria-label="Direction"
                  >
                    {l.given && (
                      <option value="given">
                        As given ({l.given.map((c) => Math.round(c * 10) / 10).join(', ')} N)
                      </option>
                    )}
                    {LOAD_DIRECTIONS.filter((d) => d.id !== 'into' || l.normal).map((d) => (
                      <option key={d.id} value={d.id}>
                        {d.label}
                      </option>
                    ))}
                  </select>
                  <select
                    className={cn(field, 'h-6 w-24')}
                    value={l.spread}
                    onChange={(e) => sim.updateLoad(i, { spread: e.target.value as Spread })}
                    aria-label="Spread"
                    title="Even: the same everywhere on the faces. Pin in hole: pressing on the half of the hole facing the pull."
                  >
                    <option value="even">Even</option>
                    <option value="bearing">Pin in hole</option>
                  </select>
                </div>
              </div>
            ))}
            <button
              type="button"
              disabled={picked.length === 0}
              onClick={addLoad}
              className="cad-focus flex h-6 items-center justify-center gap-1 rounded border border-border bg-background font-medium disabled:opacity-50"
            >
              <Plus size={11} aria-hidden />
              {picked.length === 0 ? 'Pick faces on the part' : `Load ${picked.length} picked face${picked.length === 1 ? '' : 's'}`}
            </button>
          </Step>

          <Step n={4} title="What it is made of" done={materialDone}>
            <input className={field} placeholder="Material, e.g. Al 6061-T6" value={m.name} onChange={(e) => sim.setMaterial({ name: e.target.value })} aria-label="Material" />
            <div className="grid grid-cols-[1fr_auto] items-center gap-x-1.5 gap-y-1">
              <input className={cn(field, 'font-mono')} placeholder="Young's modulus" value={m.youngs_modulus_mpa} onChange={(e) => sim.setMaterial({ youngs_modulus_mpa: e.target.value })} inputMode="decimal" aria-label="Young's modulus (MPa)" />
              <span className="text-muted-foreground">MPa</span>
              <input className={cn(field, 'font-mono')} placeholder="Poisson ratio" value={m.poisson_ratio} onChange={(e) => sim.setMaterial({ poisson_ratio: e.target.value })} inputMode="decimal" aria-label="Poisson ratio" />
              <span className="text-muted-foreground">—</span>
              <input className={cn(field, 'font-mono')} placeholder="Yield strength" value={m.yield_strength_mpa} onChange={(e) => sim.setMaterial({ yield_strength_mpa: e.target.value })} inputMode="decimal" aria-label="Yield strength (MPa)" />
              <span className="text-muted-foreground">MPa</span>
            </div>
            <input className={field} placeholder="Where the values come from" value={m.source} onChange={(e) => sim.setMaterial({ source: e.target.value })} aria-label="Source" />
            <div className="text-[11px] text-muted-foreground">No safety factor is given without a yield strength and its source.</div>
          </Step>

          <Step n={5} title="What it must achieve" done={goalDone}>
            <label className="flex items-center gap-2">
              <span className="w-24 text-muted-foreground">Safety factor ≥</span>
              <input className={cn(field, 'w-16 font-mono')} placeholder="e.g. 2" value={setup.minSafetyFactor} onChange={(e) => sim.setGoal(e.target.value)} inputMode="decimal" />
            </label>
            <label className="flex items-center gap-2">
              <span className="w-24 text-muted-foreground">Mesh size</span>
              <input className={cn(field, 'w-16 font-mono')} placeholder="auto" value={setup.meshSizeMm} onChange={(e) => sim.setMeshSize(e.target.value)} inputMode="decimal" />
              <span className="text-muted-foreground">mm</span>
            </label>
          </Step>

          <div className="flex flex-col gap-2 px-3 py-2.5">
            <button
              type="button"
              disabled={!part || !study || phase.kind === 'running'}
              onClick={() => void run(false)}
              className="cad-focus flex h-7 items-center justify-center gap-1.5 rounded bg-foreground font-medium text-background disabled:opacity-50"
            >
              <Play size={11} aria-hidden />
              {phase.kind === 'running' ? 'Meshing and solving twice…' : 'Run study'}
            </button>
            {!study && gaps.length > 0 && (
              // The steps above show what is done; here, only what to do next.
              <span className="text-[11px] text-muted-foreground" aria-label="Next">
                Next: {gaps[0]}
                {gaps.length > 1 ? ` (and ${gaps.length - 1} more)` : ''}
              </span>
            )}
          </div>

          {phase.kind === 'unverified' && (
            <div className="flex flex-col gap-2 border-t border-border/70 bg-amber-500/10 px-3 py-2.5">
              <span>The part changed since it was last verified, and the kernel won't study unverified geometry.</span>
              <button type="button" onClick={() => void run(true)} className="cad-focus h-7 self-start rounded border border-border bg-background px-2.5 font-medium">
                Verify and run
              </button>
            </div>
          )}
          {phase.kind === 'can_move' && (
            <div className="flex flex-col gap-1 border-t border-border/70 bg-amber-500/10 px-3 py-2.5 text-amber-800 dark:text-amber-200">
              <span className="font-semibold">The part is not held: it can still move.</span>
              {phase.motions.map((mo) => (
                <span key={mo}>· {mo}</span>
              ))}
              <span className="text-[11px]">Hold more faces, or a different kind of support, so nothing is free.</span>
            </div>
          )}
          {phase.kind === 'engine_missing' && (
            <div className="border-t border-border/70 bg-amber-500/10 px-3 py-2.5">
              The mesher or solver is not installed on the server: {phase.reason}
            </div>
          )}
          {phase.kind === 'refused' && (
            <div className="border-t border-border/70 bg-red-500/10 px-3 py-2.5 text-red-700 dark:text-red-300">{phase.reason}</div>
          )}
        </div>
      )}
    </section>
  )
}

const fmt = (v: number, unit: string) =>
  `${Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 1 ? v.toFixed(2) : v.toPrecision(3)} ${unit}`

/** The kernel's answer: the verdict, the numbers behind it, the checks it
 *  passed or failed, how the part is coloured, and which engines ran. */
function Results({ onEdit }: { onEdit: () => void }) {
  const report = useSimStore((s) => s.report)
  const shown = useSimStore((s) => s.shown)
  const show = useSimStore((s) => s.show)
  const deformed = useSimStore((s) => s.deformed)
  const setDeformed = useSimStore((s) => s.setDeformed)
  const [assumptionsOpen, setAssumptionsOpen] = useState(false)
  if (!report) return null
  const v = verdictView(report.verdict)
  const range = fieldRange(report.field, shown)
  const scale = deformScale(report.field)
  const gradient = `linear-gradient(to right, ${[0, 0.25, 0.5, 0.75, 1]
    .map((t) => `rgb(${stressColor(t).map((c) => Math.round(c * 255)).join(',')})`)
    .join(', ')})`

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className={cn('flex flex-col gap-0.5 px-3 py-2', STATE_STYLE[v.state].wash)}>
        <span className={cn('flex items-center gap-2 font-semibold', STATE_STYLE[v.state].text)}>
          <Mark state={v.state} />
          {v.title}
        </span>
        <span className="pl-5 text-[11px]">{v.detail}</span>
      </div>

      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 border-b border-border/70 px-3 py-2.5">
        <dt className="text-muted-foreground">Peak stress</dt>
        <dd className="text-right font-mono">{fmt(report.fine.peak_von_mises_mpa, 'MPa')}</dd>
        <dt className="text-muted-foreground">Largest movement</dt>
        <dd className="text-right font-mono">{fmt(report.fine.max_displacement_mm, 'mm')}</dd>
        <dt className="text-muted-foreground">Mesh (proving)</dt>
        <dd className="text-right font-mono">{report.fine.mesh.nodes.toLocaleString()} nodes</dd>
      </dl>

      <div className="flex flex-col gap-1.5 border-b border-border/70 px-3 py-2.5">
        <div className="flex gap-1" role="radiogroup" aria-label="Colour the part by">
          {(['stress', 'displacement'] as const).map((k) => (
            <button
              key={k}
              type="button"
              role="radio"
              aria-checked={shown === k}
              onClick={() => show(k)}
              className={cn('cad-focus h-6 flex-1 rounded border border-border/70 font-medium', shown === k ? 'bg-foreground text-background' : 'bg-background')}
            >
              {k === 'stress' ? 'Stress' : 'Movement'}
            </button>
          ))}
        </div>
        <div className="h-2.5 rounded-sm" style={{ background: gradient }} aria-hidden />
        <div className="flex justify-between font-mono text-[11px] text-muted-foreground">
          <span>{fmt(range.min, range.unit)}</span>
          <span>{fmt(range.max, range.unit)}</span>
        </div>
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={deformed} onChange={(e) => setDeformed(e.target.checked)} />
          <span>Show the bent shape{scale > 1 ? ` (movement ×${scale} so it can be seen)` : ''}</span>
        </label>
      </div>

      <ul className="flex flex-col gap-1 border-b border-border/70 px-3 py-2.5" aria-label="Checks">
        {report.checks.map((c) => (
          <li key={c.name} className="flex flex-col">
            <span className="flex items-center gap-2 font-medium">
              <Mark state={c.passed ? 'pass' : 'fix'} />
              {c.name}
            </span>
            <span className="pl-5 text-[11px] text-muted-foreground">{c.detail}</span>
          </li>
        ))}
      </ul>

      <div className="flex flex-col gap-1 px-3 py-2.5 text-[11px] text-muted-foreground">
        <span>Mesher: {engineLabel(report.engines.mesher)}</span>
        <span>Solver: {engineLabel(report.engines.solver)}</span>
        {report.assumptions.length > 0 && (
          <button type="button" className="cad-focus self-start underline" onClick={() => setAssumptionsOpen((o) => !o)}>
            {assumptionsOpen ? 'Hide' : 'Show'} the {report.assumptions.length} assumption{report.assumptions.length === 1 ? '' : 's'}
          </button>
        )}
        {assumptionsOpen && (
          <ul className="flex flex-col gap-0.5">
            {report.assumptions.map((a) => (
              <li key={a}>· {a}</li>
            ))}
          </ul>
        )}
        <button type="button" onClick={onEdit} className="cad-focus mt-1 h-7 self-start rounded border border-border bg-background px-2.5 font-medium text-foreground">
          Change the set-up
        </button>
      </div>
    </div>
  )
}
