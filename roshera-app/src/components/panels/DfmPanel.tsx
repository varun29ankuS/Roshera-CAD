import { useMemo, useState } from 'react'
import { Check, CircleHelp, Clapperboard, Play, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useDfmStore } from '@/stores/dfm-store'
import {
  DFM_PROCESSES,
  dfmRows,
  dfmSummary,
  runDfm,
  verifyPart,
  type DfmPackParams,
  type DfmState,
} from '@/lib/dfm'
import { animationFor } from '@/lib/dfm-animation'

const DIRECTIONS: { label: string; v: [number, number, number] }[] = [
  { label: '+Z (up)', v: [0, 0, 1] },
  { label: '−Z', v: [0, 0, -1] },
  { label: '+X', v: [1, 0, 0] },
  { label: '−X', v: [-1, 0, 0] },
  { label: '+Y', v: [0, 1, 0] },
  { label: '−Y', v: [0, -1, 0] },
]

/** CNC setup plans offered (unit vectors from the part toward the spindle). */
const SETUP_PLANS: { label: string; dirs: [number, number, number][] }[] = [
  { label: 'Top + flip (±Z)', dirs: [[0, 0, 1], [0, 0, -1]] },
  { label: 'Top only (+Z)', dirs: [[0, 0, 1]] },
  { label: 'All six sides', dirs: [[0, 0, 1], [0, 0, -1], [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0]] },
]

const STATE_STYLE: Record<DfmState, { mark: string; text: string; wash: string }> = {
  fix: { mark: 'text-red-600 dark:text-red-400', text: 'text-red-700 dark:text-red-300', wash: 'bg-red-500/10' },
  unknown: { mark: 'text-amber-600 dark:text-amber-400', text: 'text-amber-700 dark:text-amber-300', wash: 'bg-amber-500/12' },
  pass: { mark: 'text-emerald-600 dark:text-emerald-400', text: 'text-emerald-700 dark:text-emerald-300', wash: 'bg-emerald-500/10' },
}

function StateMark({ state }: { state: DfmState }) {
  const Icon = state === 'fix' ? X : state === 'unknown' ? CircleHelp : Check
  return <Icon size={13} className={cn('shrink-0', STATE_STYLE[state].mark)} aria-hidden />
}

/** The part's kernel solid id (what the DFM route is keyed on). */
const solidOf = (o: CADObject) => o.analyticalGeometry?.solidId

type Phase =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'unverified'; reason: string }
  | { kind: 'refused'; reason: string }

/**
 * The DFM workspace's panel: pick a part and a process, run the kernel's
 * rules, read the findings. Clicking a finding brings its faces forward in
 * the viewport. A part changed since it was last verified is not judged:
 * the kernel says so, and the panel offers to verify and run again.
 */
export function DfmPanel() {
  const objects = useSceneStore((s) => s.objects)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const report = useDfmStore((s) => s.report)
  const reportFor = useDfmStore((s) => s.objectId)
  const focusRule = useDfmStore((s) => s.focusRule)
  const setResult = useDfmStore((s) => s.setResult)
  const focus = useDfmStore((s) => s.focus)
  const playingRule = useDfmStore((s) => s.playingRule)
  const play = useDfmStore((s) => s.play)

  const parts = useMemo(
    () => [...objects.values()].filter((o) => typeof solidOf(o) === 'number' && o.visible !== false),
    [objects],
  )
  const firstSelected = parts.find((p) => selectedIds.has(p.id))
  const [chosenId, setChosenId] = useState<string | null>(null)
  const partId = chosenId && objects.has(chosenId) ? chosenId : (firstSelected ?? parts[0])?.id ?? null
  const part = partId ? objects.get(partId) : undefined

  const [pack, setPack] = useState<DfmPackParams['pack']>('cnc_3_axis')
  const [toolDia, setToolDia] = useState('3')
  const [planIndex, setPlanIndex] = useState(0)
  const [gauge, setGauge] = useState('')
  const [nozzle, setNozzle] = useState('0.4')
  const [draft, setDraft] = useState('1')
  const [dirIndex, setDirIndex] = useState(0)
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })

  const params = (): DfmPackParams | null => {
    const v = DIRECTIONS[dirIndex].v
    const positive = (x: string) => {
      const n = Number(x)
      return Number.isFinite(n) && n > 0 ? n : null
    }
    switch (pack) {
      case 'fdm': {
        const n = positive(nozzle)
        return n === null ? null : { pack, nozzle_diameter: n, build_direction: v }
      }
      case 'injection_molding':
      case 'casting': {
        const d = Number(draft)
        return Number.isFinite(d) && d >= 0 ? { pack, pull_direction: v, min_draft_deg: d } : null
      }
      case 'cnc_3_axis': {
        const t = positive(toolDia)
        return t === null ? null : { pack, setup_directions: SETUP_PLANS[planIndex].dirs, min_tool_diameter: t }
      }
      case 'cnc_turning':
        // The spindle axis is inferred by the kernel (or refused by name).
        return { pack }
      case 'sheet_metal': {
        if (gauge.trim() === '') return { pack }
        const g = positive(gauge)
        return g === null ? null : { pack, thickness: g }
      }
    }
  }

  const run = async (verifyFirst: boolean) => {
    const p = params()
    const solidId = part ? solidOf(part) : undefined
    if (!part || typeof solidId !== 'number' || !p) return
    setPhase({ kind: 'running' })
    try {
      if (verifyFirst && !(await verifyPart(solidId))) {
        setPhase({ kind: 'refused', reason: 'The part could not be verified.' })
        return
      }
      const out = await runDfm(solidId, p)
      if (out.kind === 'report') {
        setResult(part.id, out.report)
        setPhase({ kind: 'idle' })
      } else {
        setPhase(out)
      }
    } catch (e) {
      setPhase({ kind: 'refused', reason: e instanceof Error ? e.message : String(e) })
    }
  }

  const shown = report && reportFor === partId ? report : null
  const rows = shown ? dfmRows(shown) : []
  const summary = shown ? dfmSummary(shown.summary) : null
  const canRun = !!part && params() !== null && phase.kind !== 'running'

  return (
    <section aria-label="Design for manufacturing" className="flex h-full min-h-0 flex-col text-xs">
      <div className="border-b border-border/70 px-3 py-2">
        <div className="text-[13px] font-semibold">Manufacturability</div>
        <div className="text-[11px] text-muted-foreground">Checks the part against a process's rules</div>
      </div>

      <div className="flex flex-col gap-2 border-b border-border/70 px-3 py-2.5">
        <label className="flex items-center gap-2">
          <span className="w-16 text-muted-foreground">Part</span>
          <select name="dfm-panel-field-1"
            className="cad-focus h-7 min-w-0 flex-1 rounded border border-border/70 bg-background/70 px-1.5"
            value={partId ?? ''}
            onChange={(e) => setChosenId(e.target.value)}
            disabled={parts.length === 0}
          >
            {parts.length === 0 && <option value="">No parts</option>}
            {parts.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-2">
          <span className="w-16 text-muted-foreground">Process</span>
          <select name="dfm-panel-field-2"
            className="cad-focus h-7 min-w-0 flex-1 rounded border border-border/70 bg-background/70 px-1.5 font-medium"
            value={pack}
            onChange={(e) => setPack(e.target.value as DfmPackParams['pack'])}
          >
            {DFM_PROCESSES.map((p) => (
              <option key={p.pack} value={p.pack}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
        {pack === 'fdm' && (
          <label className="flex items-center gap-2">
            <span className="w-16 text-muted-foreground">Nozzle Ø</span>
            <input name="dfm-panel-field-3"
              className="cad-focus h-7 w-16 rounded border border-border/70 bg-background/70 px-1.5 font-mono"
              value={nozzle}
              onChange={(e) => setNozzle(e.target.value)}
              inputMode="decimal"
            />
            <span className="text-muted-foreground">mm</span>
          </label>
        )}
        {(pack === 'injection_molding' || pack === 'casting') && (
          <label className="flex items-center gap-2">
            <span className="w-16 text-muted-foreground">Min draft</span>
            <input name="dfm-panel-field-4"
              className="cad-focus h-7 w-16 rounded border border-border/70 bg-background/70 px-1.5 font-mono"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              inputMode="decimal"
            />
            <span className="text-muted-foreground">°</span>
          </label>
        )}
        {pack === 'cnc_3_axis' && (
          <>
            <label className="flex items-center gap-2">
              <span className="w-16 text-muted-foreground">Setups</span>
              <select name="dfm-panel-field-5"
                className="cad-focus h-7 min-w-0 flex-1 rounded border border-border/70 bg-background/70 px-1.5"
                value={planIndex}
                onChange={(e) => setPlanIndex(Number(e.target.value))}
              >
                {SETUP_PLANS.map((p, i) => (
                  <option key={p.label} value={i}>
                    {p.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-2">
              <span className="w-16 text-muted-foreground">Min tool Ø</span>
              <input name="dfm-panel-field-6"
                className="cad-focus h-7 w-16 rounded border border-border/70 bg-background/70 px-1.5 font-mono"
                value={toolDia}
                onChange={(e) => setToolDia(e.target.value)}
                inputMode="decimal"
              />
              <span className="text-muted-foreground">mm</span>
            </label>
          </>
        )}
        {pack === 'sheet_metal' && (
          <label className="flex items-center gap-2">
            <span className="w-16 text-muted-foreground">Gauge</span>
            <input name="auto"
              className="cad-focus h-7 w-16 rounded border border-border/70 bg-background/70 px-1.5 font-mono"
              value={gauge}
              placeholder="auto"
              onChange={(e) => setGauge(e.target.value)}
              inputMode="decimal"
            />
            <span className="text-muted-foreground">mm</span>
          </label>
        )}
        {(pack === 'fdm' || pack === 'injection_molding' || pack === 'casting') && (
          <label className="flex items-center gap-2">
            <span className="w-16 text-muted-foreground">{pack === 'fdm' ? 'Build' : 'Pull'}</span>
            <select name="dfm-panel-field-7"
              className="cad-focus h-7 w-24 rounded border border-border/70 bg-background/70 px-1.5"
              value={dirIndex}
              onChange={(e) => setDirIndex(Number(e.target.value))}
            >
              {DIRECTIONS.map((d, i) => (
                <option key={d.label} value={i}>
                  {d.label}
                </option>
              ))}
            </select>
          </label>
        )}
        <button
          type="button"
          disabled={!canRun}
          onClick={() => void run(false)}
          className="cad-focus mt-0.5 flex h-7 items-center justify-center gap-1.5 rounded bg-foreground font-medium text-background disabled:opacity-50"
        >
          <Play size={11} aria-hidden />
          {phase.kind === 'running' ? 'Checking…' : 'Run check'}
        </button>
      </div>

      {phase.kind === 'unverified' && (
        <div className="flex flex-col gap-2 border-b border-border/70 bg-amber-500/10 px-3 py-2.5">
          <span>The part changed since it was last verified, and the kernel won't judge unverified geometry.</span>
          <button
            type="button"
            onClick={() => void run(true)}
            className="cad-focus h-7 self-start rounded border border-border bg-background px-2.5 font-medium"
          >
            Verify and check
          </button>
        </div>
      )}
      {phase.kind === 'refused' && (
        <div className="border-b border-border/70 bg-red-500/10 px-3 py-2.5 text-red-700 dark:text-red-300">
          {phase.reason}
        </div>
      )}

      {summary && (
        <div className={cn('flex items-center gap-2 px-3 py-2 font-semibold', STATE_STYLE[summary.state].wash, STATE_STYLE[summary.state].text)}>
          <StateMark state={summary.state} />
          {summary.text}
        </div>
      )}

      <ul className="min-h-0 flex-1 overflow-y-auto px-1.5 py-1.5" aria-label="Findings">
        {rows.map((r) => {
          const on = focusRule === r.rule
          return (
            <li key={r.rule}>
              <button
                type="button"
                onClick={() => focus(on ? null : r.rule)}
                aria-pressed={on}
                className={cn(
                  'cad-focus flex w-full flex-col gap-0.5 rounded px-2 py-1.5 text-left',
                  on ? STATE_STYLE[r.state].wash : 'hover:bg-accent/60',
                )}
              >
                <span className="flex items-center gap-2">
                  <StateMark state={r.state} />
                  <span className="flex-1 font-medium">{r.title}</span>
                  {r.state === 'fix' && (
                    <span className="font-mono text-[11px]">
                      {r.measured} <span className="text-muted-foreground">{r.limit}</span>
                    </span>
                  )}
                  {r.state === 'pass' && r.measured && (
                    <span className="font-mono text-[11px] text-muted-foreground">{r.measured}</span>
                  )}
                </span>
                {r.state === 'unknown' && (
                  <span className={cn('pl-5 text-[11px] text-muted-foreground', !on && 'line-clamp-2')}>
                    Could not be checked: {r.why}
                  </span>
                )}
                {r.faces.length > 0 && (
                  <span className="pl-5 text-[11px] text-muted-foreground">
                    {r.faces.length} face{r.faces.length === 1 ? '' : 's'}
                    {on ? ' · shown on the part' : ''}
                  </span>
                )}
                {on && <span className="pl-5 text-[11px] text-muted-foreground">Rule: {r.source}</span>}
              </button>
              {r.state === 'fix' && animationFor(shown?.animations, r.rule) && (
                <button
                  type="button"
                  onClick={() => play(playingRule === r.rule ? null : r.rule)}
                  aria-pressed={playingRule === r.rule}
                  className="cad-focus mb-1 ml-7 flex h-6 items-center gap-1.5 rounded border border-border/70 px-2 text-[11px] hover:bg-accent/60"
                  title="Play the kernel's explanation of this finding on the part"
                >
                  <Clapperboard size={11} aria-hidden />
                  {playingRule === r.rule ? 'Stop' : 'Show why'}
                </button>
              )}
            </li>
          )
        })}
        {!shown && phase.kind === 'idle' && (
          <li className="px-2 py-3 text-[11px] text-muted-foreground">
            Choose a process and run the check. Findings appear here and on the part: red to fix,
            amber where a check could not run.
          </li>
        )}
      </ul>
    </section>
  )
}
