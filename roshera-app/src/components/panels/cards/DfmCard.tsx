import {
  Check,
  CircleSlash,
  FlaskConical,
  X,
} from '@/components/ui/icons'
import {
  isDfmReport,
  type DfmCard as DfmCardData,
  type DfmCardSummary,
  type DfmCardValue,
  type DfmCardVerdict,
  type DfmReportCard,
  type DfmRuleProvenance,
  type DfmUnverifiableReason,
} from '@/lib/blackboard-cards'
import { CardShell, Chip, KV, type CardAccent } from './card-chrome'
import { fmtNum } from './format'

/**
 * DFM CARD
 * ========
 * A whole `DfmReport` (summary, then one row per rule) or a single rule's
 * `RuleVerdict` (geometry-engine/src/dfm/report.rs), without reinterpretation:
 *   - Pass       → the proven margin, with its derivation;
 *   - Violation  → measured vs limit (both provenance-carrying) + the
 *                  witness faces;
 *   - Unverifiable → an honest refusal, never an error — and for
 *     `bound_not_separating`, the proven enclosure [lo, hi] is shown against
 *     the limit: the kernel cannot separate them and says so WITH the bound.
 * The provenance footer keeps a violation-of-a-standard distinguishable from
 * a violation-of-a-shop-heuristic — they are different findings.
 */

function derivationLine(v: DfmCardValue): string {
  const d = v.derivation
  if (d.kind === 'analytic') {
    return `${d.method} (closed-form on ${d.surface_type})`
  }
  return `${d.method} (proven enclosure, ${d.refinement_depth} refinement ${
    d.refinement_depth === 1 ? 'sweep' : 'sweeps'
  }${d.converged ? '' : ', budget exhausted before requested tightness'})`
}

function boundSuffix(v: DfmCardValue, unit: string): string | null {
  if (!v.bound) return null
  return `proven ∈ [${fmtNum(v.bound.lo)}, ${fmtNum(v.bound.hi)}]${unit}`
}

function provenanceLine(p: DfmRuleProvenance): { label: string; text: string } {
  switch (p.kind) {
    case 'standard':
      return {
        label: 'standard',
        text: `${p.body.toUpperCase()} ${p.designation} (${p.edition}${
          p.clause ? `, ${p.clause}` : ''
        })`,
      }
    case 'handbook':
      return { label: 'handbook', text: p.citation }
    case 'material_datasheet':
      return { label: 'datasheet', text: p.source }
    case 'shop_practice':
      return { label: 'shop practice', text: `no governing standard — ${p.note}` }
  }
}

function reasonBlock(reason: DfmUnverifiableReason, unit: string) {
  switch (reason.kind) {
    case 'unsupported_surface':
      return (
        <div>
          No closed-form method for a{' '}
          <span className="cad-readout">{reason.surface_type}</span> surface in{' '}
          <span className="cad-readout">{reason.analyzer}</span> — refused rather than
          approximated.
        </div>
      )
    case 'unsound_precondition':
      return <div>Soundness precondition failed before measurement: {reason.detail}</div>
    case 'unsupported_topology':
      return <div>Boundary topology defeats the exact reconstruction: {reason.detail}</div>
    case 'bound_not_separating':
      return (
        <div>
          Proven bound{' '}
          <span className="cad-readout">
            [{fmtNum(reason.lo)}, {fmtNum(reason.hi)}]{unit}
          </span>{' '}
          straddles the limit <span className="cad-readout">{fmtNum(reason.limit)}{unit}</span>{' '}
          after {reason.refinement_depth} refinement{' '}
          {reason.refinement_depth === 1 ? 'sweep' : 'sweeps'}
          {reason.converged ? '' : ' (budget exhausted)'}. The bound is a theorem; the
          kernel cannot separate it from the limit and says so rather than picking a side.
        </div>
      )
  }
}

/** Witness / region face chips. Geometry-anchoring slice: these face ids are
 *  the attach point for viewport highlighting once persistent-ID selection
 *  integration lands — a chip click would select the face. */
function FaceChips({ label, faces }: { label: string; faces: number[] }) {
  if (faces.length === 0) return null
  return (
    <div className="mt-1 flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground">
      <span>{label}:</span>
      {faces.map((f) => (
        <span key={f} className="cad-readout rounded border border-border px-1 py-px">
          face {f}
        </span>
      ))}
    </div>
  )
}

function accentOf(v: DfmCardVerdict): CardAccent {
  return v.kind === 'pass' ? 'pass' : v.kind === 'violation' ? 'fail' : 'warn'
}

function VerdictChip({ verdict }: { verdict: DfmCardVerdict }) {
  if (verdict.kind === 'pass') {
    return (
      <Chip accent="pass">
        <Check size={9} /> pass — proven
      </Chip>
    )
  }
  if (verdict.kind === 'violation') {
    return (
      <Chip accent="fail">
        <X size={9} /> violation — proven
      </Chip>
    )
  }
  return (
    <Chip accent="warn" title="An honest refusal to decide — not an error, and never folded to a pass">
      <CircleSlash size={9} /> unverifiable
    </Chip>
  )
}

function VerdictBody({ verdict: v, unit }: { verdict: DfmCardVerdict; unit: string }) {
  return (
    <div className="mt-1.5 space-y-0.5">
      {v.kind === 'pass' && (
        <>
          <KV label="margin">
            ≥ {fmtNum(v.margin.value)}
            {unit}
          </KV>
          {boundSuffix(v.margin, unit) !== null && (
            <div className="text-[11px] text-muted-foreground">{boundSuffix(v.margin, unit)}</div>
          )}
          <div className="text-[11px] text-muted-foreground">{derivationLine(v.margin)}</div>
        </>
      )}

      {v.kind === 'violation' && (
        <>
          <KV label="measured">
            {fmtNum(v.measured.value)}
            {unit}
          </KV>
          <KV label="limit">
            {fmtNum(v.limit.value)}
            {unit}
          </KV>
          {boundSuffix(v.measured, unit) !== null && (
            <div className="text-[11px] text-muted-foreground">
              measured {boundSuffix(v.measured, unit)}
            </div>
          )}
          <div className="text-[11px] text-muted-foreground">
            measured: {derivationLine(v.measured)} · limit: {derivationLine(v.limit)}
          </div>
          <FaceChips label="witnesses" faces={v.witnesses} />
        </>
      )}

      {v.kind === 'unverifiable' && (
        <>
          <div className="text-[11px] text-foreground/85">{reasonBlock(v.reason, unit)}</div>
          <FaceChips label="regions" faces={v.regions} />
        </>
      )}
    </div>
  )
}

function ProvenanceFooter({ provenance }: { provenance: DfmRuleProvenance }) {
  const prov = provenanceLine(provenance)
  return (
    <div className="mt-1.5 flex flex-wrap items-baseline gap-1.5 border-t border-border/50 pt-1 text-[11px] text-muted-foreground">
      <span className="rounded border border-border px-1 py-px font-mono uppercase tracking-wide">
        {prov.label}
      </span>
      <span>{prov.text}</span>
    </div>
  )
}

/** The pack-level fold, verbatim: inconclusive keeps its count and is never a pass. */
function SummaryChip({ summary }: { summary: DfmCardSummary }) {
  if (summary.kind === 'pass') {
    return (
      <Chip accent="pass" title="Every rule in the pack is proven to pass">
        <Check size={9} /> pass — every rule proven
      </Chip>
    )
  }
  if (summary.kind === 'violations') {
    return (
      <Chip accent="fail">
        <X size={9} /> {summary.count} {summary.count === 1 ? 'violation' : 'violations'}
      </Chip>
    )
  }
  return (
    <Chip accent="warn" title="No violation was proven, but some rules could not be checked — not a pass">
      <CircleSlash size={9} /> inconclusive · {summary.unverifiable} unverifiable
    </Chip>
  )
}

function summaryAccent(summary: DfmCardSummary): CardAccent {
  return summary.kind === 'pass' ? 'pass' : summary.kind === 'violations' ? 'fail' : 'warn'
}

function DfmReportView({ card }: { card: DfmReportCard }) {
  return (
    <CardShell
      accent={summaryAccent(card.summary)}
      icon={FlaskConical}
      title={
        <>
          DFM — <span className="cad-readout">{card.pack}</span>
        </>
      }
      chip={<SummaryChip summary={card.summary} />}
    >
      <div className="mt-1.5 flex flex-col gap-1.5">
        {card.verdicts.map((rv, i) => (
          <div key={`${rv.rule}-${i}`} className="rounded border border-border/60 px-1.5 py-1">
            <div className="flex flex-wrap items-center justify-between gap-1">
              <span className="cad-readout text-[11px] text-foreground/90">{rv.rule}</span>
              <VerdictChip verdict={rv.verdict} />
            </div>
            <VerdictBody verdict={rv.verdict} unit="" />
            <ProvenanceFooter provenance={rv.provenance} />
          </div>
        ))}
      </div>
      {card.note && <div className="mt-1 text-[11px] text-foreground/70">{card.note}</div>}
    </CardShell>
  )
}

export function DfmCard({ card }: { card: DfmCardData }) {
  if (isDfmReport(card)) return <DfmReportView card={card} />
  const unit = card.unit ? ` ${card.unit}` : ''
  return (
    <CardShell
      accent={accentOf(card.verdict)}
      icon={FlaskConical}
      title={<span className="cad-readout">{card.rule}</span>}
      chip={<VerdictChip verdict={card.verdict} />}
    >
      <VerdictBody verdict={card.verdict} unit={unit} />
      <ProvenanceFooter provenance={card.provenance} />
      {card.note && <div className="mt-1 text-[11px] text-foreground/70">{card.note}</div>}
    </CardShell>
  )
}
