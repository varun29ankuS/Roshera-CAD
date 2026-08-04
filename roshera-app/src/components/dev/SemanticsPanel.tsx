import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { ArrowLeft, CircleSlash, RefreshCw, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { CheckpointSummary, EventSummary } from '@/lib/timeline-events'
import {
  EvidencePackHttpError,
  fetchCheckpoints,
  fetchEvidencePack,
  fetchTimelineHistory,
  fetchToolRegistry,
  type EvidencePack,
  type ToolRegistry,
} from '@/lib/tool-registry-api'

/**
 * SEMANTICS — developer panel (#/semantics)
 * =========================================
 * The vocabulary the agent actually sees, and how much of what it does is
 * actually recorded. Audience: the founder/developer. Density and honesty
 * over polish; every number on this page is read from a live endpoint on
 * this render — nothing is compiled in, nothing is estimated client-side.
 *
 * The honesty rule this page is built around: a provenance dimension with
 * no data is rendered as an explicit, labelled row saying so — never
 * omitted, never shown as zero, never faked with a placeholder number.
 * The tone vocabulary is the Timeline strip's (`DurabilityChip`): calm
 * neutral = real recorded data, amber dashed = withheld / not recorded
 * (a true gap, by design or by debt), red = this page could not fetch.
 */

// ─── Fetch-state plumbing (per-source, independent failure) ─────────

type Fetched<T> =
  | { state: 'loading' }
  | { state: 'ok'; data: T }
  | { state: 'error'; message: string; httpStatus?: number }

function useFetched<T>(loader: () => Promise<T>, refreshKey: number): Fetched<T> {
  const [value, setValue] = useState<Fetched<T>>({ state: 'loading' })
  useEffect(() => {
    let cancelled = false
    setValue({ state: 'loading' })
    loader().then(
      (data) => {
        if (!cancelled) setValue({ state: 'ok', data })
      },
      (err: unknown) => {
        if (cancelled) return
        const httpStatus = err instanceof EvidencePackHttpError ? err.status : undefined
        setValue({
          state: 'error',
          message: err instanceof Error ? err.message : String(err),
          httpStatus,
        })
      },
    )
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey])
  return value
}

// ─── Shared chrome ──────────────────────────────────────────────────

/** DurabilityChip's tri-state colour language, reused verbatim:
 *  neutral = real data · amber dashed = withheld/not recorded · red = fetch failed. */
function StateChip({
  tone,
  label,
}: {
  tone: 'recorded' | 'partial' | 'absent' | 'failed'
  label: string
}) {
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1 rounded border px-1.5 py-[2px] text-[10px] leading-none whitespace-nowrap',
        tone === 'recorded' && 'border-border text-foreground/80',
        (tone === 'partial' || tone === 'absent') &&
          'border-dashed border-amber-500/40 bg-amber-500/5 text-amber-800 dark:text-amber-300',
        tone === 'failed' && 'border-red-500/40 bg-red-500/10 text-red-800 dark:text-red-300',
      )}
    >
      {tone === 'failed' ? (
        <X size={10} className="shrink-0" />
      ) : tone === 'recorded' ? null : (
        <CircleSlash size={10} className="shrink-0" />
      )}
      {label}
    </span>
  )
}

function SectionHeader({ title, sub }: { title: string; sub: string }) {
  return (
    <div className="mb-2">
      <div className="text-[11px] uppercase tracking-wide text-muted-foreground/70">{title}</div>
      <div className="text-[11px] text-muted-foreground/50">{sub}</div>
    </div>
  )
}

function FetchErrorRow({ what, message }: { what: string; message: string }) {
  return (
    <div className="flex items-baseline gap-2 rounded border border-red-500/40 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-800 dark:text-red-300">
      <X size={10} className="shrink-0 self-center" />
      <span className="font-medium">{what} unavailable:</span>
      <span className="font-mono">{message}</span>
    </div>
  )
}

// ─── Section 1 — Ontology census ────────────────────────────────────

/** Serve order fixed by agent_registry.rs::Bench. */
const BENCH_ORDER = ['core', 'sketch', 'assembly', 'drawing', 'analysis', 'labels', 'timeline']

function OntologyCensus({ registry }: { registry: ToolRegistry }) {
  const byBench = new Map<string, ToolRegistry['tools']>()
  for (const tool of registry.tools) {
    const list = byBench.get(tool.bench)
    if (list) list.push(tool)
    else byBench.set(tool.bench, [tool])
  }
  // A bench string outside the compiled enum would mean the served payload
  // and this page's expectations have drifted — render it, don't drop it.
  const strayBenches = [...byBench.keys()].filter((b) => !BENCH_ORDER.includes(b))

  return (
    <div className="space-y-1.5">
      <div className="text-[11px] text-muted-foreground">
        <span className="font-medium text-foreground">{registry.tool_count} tools</span> served ·
        experimental in <span className="text-amber-700 dark:text-amber-300">amber</span> · kernel-sourced
        purpose marked <span className="font-mono">k</span> (all others: curated from the MCP zod contract)
      </div>
      {[...BENCH_ORDER, ...strayBenches].map((bench) => {
        const tools = byBench.get(bench) ?? []
        const served = registry.bench_counts[bench]
        const mismatch = typeof served === 'number' && served !== tools.length
        return (
          <div key={bench} className="rounded border border-border/70 px-2 py-1.5">
            <div className="flex items-baseline gap-2">
              <span className="font-mono text-[12px] font-medium">{bench}</span>
              <span className="font-mono text-[11px] text-muted-foreground">{tools.length}</span>
              {!BENCH_ORDER.includes(bench) && (
                <StateChip tone="partial" label="bench outside the compiled enum" />
              )}
              {mismatch && (
                <StateChip
                  tone="failed"
                  label={`served bench_counts says ${served}, tools[] groups to ${tools.length}`}
                />
              )}
            </div>
            {tools.length === 0 ? (
              <div className="mt-0.5 text-[11px] text-muted-foreground/60">
                no tools served under this bench
              </div>
            ) : (
              <div className="mt-0.5 flex flex-wrap gap-x-2 gap-y-0.5">
                {tools.map((t) => (
                  <span
                    key={t.name}
                    title={`${t.purpose}\nstability: ${t.stability} · source: ${t.source} · ~${t.token_estimate} tok`}
                    className={cn(
                      'font-mono text-[11px] leading-tight',
                      t.stability === 'experimental'
                        ? 'text-amber-700 dark:text-amber-300'
                        : 'text-foreground/85',
                    )}
                  >
                    {t.name}
                    {t.source === 'kernel' && (
                      <sup className="text-[8px] text-muted-foreground/70">k</sup>
                    )}
                  </span>
                ))}
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

// ─── Section 2 — Provenance coverage ────────────────────────────────

function CoverageRow({
  dimension,
  tone,
  chip,
  fact,
  gap,
}: {
  dimension: string
  tone: 'recorded' | 'partial' | 'absent' | 'failed'
  chip: string
  /** What the data actually says — real numbers only. */
  fact: string | null
  /** What is missing and why — always visible, never hover-only. */
  gap: string | null
}) {
  return (
    <div className="rounded border border-border/70 px-2 py-1.5">
      <div className="flex items-baseline gap-2">
        <span className="w-24 shrink-0 font-mono text-[12px] font-medium">{dimension}</span>
        <StateChip tone={tone} label={chip} />
        {fact && <span className="text-[11px] text-foreground/85">{fact}</span>}
      </div>
      {gap && (
        <div className="mt-0.5 pl-[104px] text-[11px] leading-snug text-muted-foreground">
          {gap}
        </div>
      )}
    </div>
  )
}

function CoverageSection({
  history,
  checkpoints,
  pack,
}: {
  history: Fetched<EventSummary[]>
  checkpoints: Fetched<CheckpointSummary[]>
  pack: Fetched<EvidencePack>
}) {
  // lineage — GET /api/timeline/history: affected_parts names OUTPUTS only.
  let lineage: ReactNode
  if (history.state === 'loading') {
    lineage = <CoverageRow dimension="lineage" tone="partial" chip="loading…" fact={null} gap={null} />
  } else if (history.state === 'error') {
    lineage = (
      <CoverageRow
        dimension="lineage"
        tone="failed"
        chip="fetch failed"
        fact={null}
        gap={`Could not read /api/timeline/history/main (${history.message}) — no lineage number is shown rather than a guessed one.`}
      />
    )
  } else {
    const events = history.data
    const withOutputs = events.filter(
      (e) => Array.isArray(e.affected_parts) && e.affected_parts.length > 0,
    ).length
    lineage = (
      <CoverageRow
        dimension="lineage"
        tone="partial"
        chip="outputs only"
        fact={`${withOutputs} of ${events.length} recorded events name the parts they produced (affected_parts).`}
        gap="Outputs only: the wire contract excludes consumed operands (EventSummary.affected_parts), so input→output edges are not recorded and a lineage graph cannot be drawn from served data. Events without the field predate it or touched no solid."
      />
    )
  }

  // certificates — GET /api/evidence-pack: certificate AS RECORDED, null + reason when absent.
  let certificates: ReactNode
  if (pack.state === 'loading') {
    certificates = (
      <CoverageRow dimension="certificates" tone="partial" chip="loading…" fact={null} gap={null} />
    )
  } else if (pack.state === 'error') {
    certificates = (
      <CoverageRow
        dimension="certificates"
        tone="failed"
        chip={pack.httpStatus === 401 ? 'auth-gated (401)' : 'fetch failed'}
        fact={null}
        gap={
          pack.httpStatus === 401
            ? '/api/evidence-pack sits behind the global auth layer and this browser session is not authenticated. The recorded-certificate count exists server-side but cannot be read here — shown as unavailable rather than invented.'
            : `Could not read /api/evidence-pack (${pack.message}) — no certificate count is shown rather than a guessed one.`
        }
      />
    )
  } else {
    const ops = pack.data.operations
    const certified = ops.filter((o) => o.certificate !== null && o.certificate !== undefined)
    const firstAbsent = ops.find((o) => o.certificate_absent_reason)
    certificates = (
      <CoverageRow
        dimension="certificates"
        tone={certified.length === ops.length && ops.length > 0 ? 'recorded' : 'partial'}
        chip={`${certified.length}/${ops.length} ops`}
        fact={`${certified.length} of ${ops.length} recorded operations carry a certificate as recorded (evidence-pack, read verbatim from event metadata).`}
        gap={
          certified.length === ops.length
            ? null
            : firstAbsent?.certificate_absent_reason
              ? `Backend's own reason for the null rows: "${firstAbsent.certificate_absent_reason}"`
              : 'The remaining operations carry certificate: null — no recorded verdict, never a fabricated one.'
        }
      />
    )
  }

  // intent — no per-op field exists anywhere in the served shapes.
  const checkpointCount =
    checkpoints.state === 'ok' ? checkpoints.data.length : null
  const intent = (
    <CoverageRow
      dimension="intent"
      tone="absent"
      chip="not recorded"
      fact={null}
      gap={
        'No recorded event carries an intent field — neither EventSummary (/api/timeline/history) nor EvidenceOperation (/api/evidence-pack) has one. ' +
        (checkpointCount !== null
          ? `${checkpointCount} checkpoint${checkpointCount === 1 ? '' : 's'} declare intent over sequence ranges (/api/timeline/checkpoints), but `
          : checkpoints.state === 'error'
            ? `Checkpoints could not be read (${checkpoints.message}); when they can, they declare intent over sequence ranges, but `
            : 'Checkpoints declare intent over sequence ranges, but ') +
        'the op→intent link is a client-side range projection, not recorded provenance: no operation stores which intent it served.'
      }
    />
  )

  // tools used — the timeline records KERNEL operations, not agent tool calls.
  let toolsUsed: ReactNode
  if (history.state === 'loading') {
    toolsUsed = (
      <CoverageRow dimension="tools used" tone="partial" chip="loading…" fact={null} gap={null} />
    )
  } else if (history.state === 'error') {
    toolsUsed = (
      <CoverageRow
        dimension="tools used"
        tone="failed"
        chip="fetch failed"
        fact={null}
        gap={`Could not read /api/timeline/history/main (${history.message}).`}
      />
    )
  } else {
    const events = history.data
    const kinds = new Set(events.map((e) => e.operation_type))
    toolsUsed = (
      <CoverageRow
        dimension="tools used"
        tone="partial"
        chip="kernel ops only"
        fact={`${events.length} kernel operations recorded across ${kinds.size} distinct kinds (operation_type).`}
        gap="The timeline records KERNEL operations, not agent tool calls. MCP-local calls (find_tool, describe_tool, workbench) resolve inside the MCP process and never reach the backend — they are structurally invisible: no endpoint records them, so no count exists to show."
      />
    )
  }

  return (
    <div className="space-y-1.5">
      {lineage}
      {certificates}
      {intent}
      {toolsUsed}
    </div>
  )
}

// ─── Section 3 — Classification drift ───────────────────────────────

function DriftSection({ registry }: { registry: Fetched<ToolRegistry> }) {
  return (
    <div className="space-y-1.5">
      <div className="rounded border border-dashed border-amber-500/40 bg-amber-500/5 px-2 py-1.5 text-[11px] leading-snug text-amber-800 dark:text-amber-300">
        <span className="font-medium">Per-tool comparison is not possible from this panel.</span>{' '}
        Two sources classify the same tools: the backend&apos;s <span className="font-mono">bench</span>{' '}
        column (api-server <span className="font-mono">agent_registry.rs</span>, shown in Section 1)
        and the MCP&apos;s <span className="font-mono">BENCH_OF</span> table (
        <span className="font-mono">roshera-mcp/src/registry.ts</span>). The registry response
        carries only the backend&apos;s classification; the MCP&apos;s is compiled into the MCP
        process and served over no HTTP endpoint this page can reach. Agreement between them is
        therefore unverifiable here — this panel does not pretend they agree.
      </div>
      {registry.state === 'ok' ? (
        <div className="rounded border border-border/70 px-2 py-1.5 text-[11px] leading-snug">
          <div className="text-muted-foreground">
            The drift affordance that does exist, server-side:
          </div>
          <div className="mt-0.5 flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
            <span>
              <span className="text-muted-foreground">registry_hash</span>{' '}
              <span className="font-mono font-medium">{registry.data.registry_hash}</span>
            </span>
            <span>
              <span className="text-muted-foreground">algorithm</span>{' '}
              <span className="font-mono">{registry.data.hash_algorithm}</span>
            </span>
            <span>
              <span className="text-muted-foreground">generated_at</span>{' '}
              <span className="font-mono">{registry.data.generated_at}</span>
            </span>
          </div>
          <div className="mt-0.5 text-muted-foreground">
            The hash is a pure function of the canonicalized tools array; the MCP compares it
            against its compiled snapshot and refuses loudly on mismatch. Detection lives on the
            MCP side — this page can only display the value the server is currently serving.
          </div>
        </div>
      ) : registry.state === 'error' ? (
        <FetchErrorRow what="registry hash" message={registry.message} />
      ) : null}
    </div>
  )
}

// ─── The page ───────────────────────────────────────────────────────

export function SemanticsPanel({ onExit }: { onExit: () => void }) {
  const [refreshKey, setRefreshKey] = useState(0)
  const refresh = useCallback(() => setRefreshKey((k) => k + 1), [])

  const registry = useFetched(fetchToolRegistry, refreshKey)
  const history = useFetched(() => fetchTimelineHistory('main'), refreshKey)
  const checkpoints = useFetched(fetchCheckpoints, refreshKey)
  const pack = useFetched(() => fetchEvidencePack('main'), refreshKey)

  return (
    <div className="h-screen w-screen overflow-y-auto bg-background text-foreground select-text">
      <div className="mx-auto max-w-4xl px-4 py-4">
        <div className="mb-4 flex items-center gap-3">
          <button
            type="button"
            onClick={onExit}
            className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
          >
            <ArrowLeft size={12} /> workspace
          </button>
          <div>
            <div className="text-sm font-medium">Semantics</div>
            <div className="text-[11px] text-muted-foreground">
              what the agent can say, and how much of what it does is actually recorded — every
              number fetched live, gaps rendered as gaps
            </div>
          </div>
          <button
            type="button"
            onClick={refresh}
            className="ml-auto inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-accent/40 hover:text-foreground"
          >
            <RefreshCw size={12} /> refresh
          </button>
        </div>

        <div className="space-y-5">
          <section>
            <SectionHeader
              title="1 · Ontology census"
              sub="GET /api/agent/tool-registry — the vocabulary the agent actually sees, grouped by the backend's bench classification"
            />
            {registry.state === 'loading' ? (
              <div className="text-[11px] text-muted-foreground">loading…</div>
            ) : registry.state === 'error' ? (
              <FetchErrorRow what="tool registry" message={registry.message} />
            ) : (
              <OntologyCensus registry={registry.data} />
            )}
          </section>

          <section>
            <SectionHeader
              title="2 · Provenance coverage"
              sub="one row per dimension, each stating its real state — an absent dimension is a labelled row, never a silent zero"
            />
            <CoverageSection history={history} checkpoints={checkpoints} pack={pack} />
          </section>

          <section>
            <SectionHeader
              title="3 · Classification drift"
              sub="two sources classify one tool surface — what this page can and cannot verify about their agreement"
            />
            <DriftSection registry={registry} />
          </section>
        </div>
      </div>
    </div>
  )
}
