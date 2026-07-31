import katex from 'katex'
import { Check, CircleSlash, PenLine, ShieldCheck, X } from 'lucide-react'
import type { FcfCard as FcfCardData } from '@/lib/blackboard-cards'
import {
  characteristicSymbol,
  isKernelCertifiable,
  modifierGlyph,
  modifierSymbol,
} from '@/lib/gdt-symbols'
import { CardShell, Chip, type CardAccent } from './card-chrome'
import { fmtNum } from './format'

/**
 * FEATURE CONTROL FRAME CARD
 * ==========================
 * Renders a GD&T frame as boxed tabular notation — a KaTeX array with
 * vertical rules and \hline, exactly the convention the policy doc fixes for
 * Blackboard LaTeX (⌖ | ⌀0.10 | A | B). Symbols come from the verified
 * gdt-symbols table, never spelled-out words.
 *
 * ★ THE CERTIFICATION BOUNDARY IS THE PRIMARY VISUAL DISTINCTION.
 * Only ⏥ ⊥ ∥ ⌖ are kernel-certified, at RFS, and the kernel schema carries
 * no material-condition modifier. So:
 *   - certified  = certified characteristic + no modifier + an actual kernel
 *     verdict attached → solid "kernel-certified · RFS" chip and the verdict
 *     (measured / residual / DRF disclosure) rendered as evidence;
 *   - everything else → dashed "design intent — uncertified" chip with the
 *     specific reason (characteristic outside the certified set, modifier
 *     outside the schema, or no verdict). Comprehensive NOTATION must never
 *     read as comprehensive CERTIFICATION.
 *
 * Dialects are not mixed silently: concentricity/symmetry render for legacy
 * drawings but carry the Y14.5-2018 removal marker unless the frame states
 * ISO GPS as its governing standard (where coaxiality is retained).
 */

/** Escape user-authored text for a KaTeX \text{} cell. */
function texText(s: string): string {
  return `\\text{${s.replace(/([\\{}$%&#_^~])/g, '\\$1')}}`
}

function frameLatex(card: FcfCardData): string {
  const sym = characteristicSymbol(card.characteristic)
  const glyph = sym?.glyph ?? card.characteristic

  const zoneParts: string[] = []
  if (card.tolerance.spherical_diameter) zoneParts.push(texText('S⌀'))
  else if (card.tolerance.diameter) zoneParts.push(texText('⌀'))
  const tol =
    card.tolerance.label ??
    card.verdict?.tolerance_label ??
    (card.tolerance.value_mm !== undefined
      ? fmtNum(card.tolerance.value_mm)
      : card.verdict?.tolerance_mm !== undefined
        ? fmtNum(card.verdict.tolerance_mm)
        : '')
  if (tol) zoneParts.push(texText(tol))
  if (card.tolerance.modifier) {
    const g = modifierGlyph(card.tolerance.modifier)
    if (g !== null) zoneParts.push(texText(g))
  }

  const datums =
    card.verdict?.datum_statuses?.map((d) => d.label) ?? card.datums ?? []

  const cells = [texText(glyph), zoneParts.join('\\,') || '\\;', ...datums.map(texText)]
  const colSpec = `|${cells.map(() => 'c').join('|')}|`
  return `\\begin{array}{${colSpec}}\\hline ${cells.join(' & ')} \\\\ \\hline\\end{array}`
}

export function FcfCard({ card }: { card: FcfCardData }) {
  const sym = characteristicSymbol(card.characteristic)
  const modifier = card.tolerance.modifier
    ? modifierSymbol(card.tolerance.modifier)
    : null

  const certifiable =
    isKernelCertifiable(card.characteristic, card.tolerance.modifier) &&
    !card.tolerance.spherical_diameter
  const verdict = card.verdict
  const certified = certifiable && verdict !== undefined

  // No manual useMemo — the React Compiler (enabled repo-wide) memoizes
  // this; katex.renderToString is pure on `card`.
  const html = katex.renderToString(frameLatex(card), {
    throwOnError: false,
    strict: false,
    displayMode: false,
  })

  // Why this frame is NOT a certificate — the specific reason, stated.
  let intentReason: string | null = null
  if (!certified) {
    if (sym === null) {
      intentReason = `"${card.characteristic}" is not a recognised characteristic`
    } else if (!sym.certified) {
      intentReason = `${sym.name.toLowerCase()} is not kernel-evaluated`
    } else if (modifier !== null || card.tolerance.spherical_diameter) {
      const g = modifier ? (modifier.glyph ?? modifier.fallback ?? modifier.name) : 'S⌀'
      intentReason = `modifier ${g} is outside the kernel schema (evaluation is RFS-only)`
    } else {
      intentReason = 'no kernel verdict attached'
    }
  }

  // Y14.5-2018 dialect marker for the removed characteristics.
  let dialectNote: string | null = null
  if (sym !== null && sym.asme === 'removed-2018') {
    dialectNote =
      card.standard === 'iso-gps'
        ? `ISO GPS: retained${sym.isoName ? ` as ${sym.isoName.toLowerCase()}` : ''} (ISO 1101)`
        : `removed in ASME Y14.5-2018 — use position, runout, or profile${
            card.standard === undefined && sym.isoName
              ? `; ISO retains ${sym.isoName.toLowerCase()}`
              : ''
          }`
  }

  const conforms = verdict?.conforms
  const verdictAccent: CardAccent =
    conforms === 'in_spec' ? 'pass' : conforms === 'out_of_spec' ? 'fail' : 'warn'
  const accent: CardAccent = certified ? verdictAccent : 'neutral'

  const measured =
    verdict?.measured_label ??
    (verdict?.measured_mm !== undefined ? `${fmtNum(verdict.measured_mm)} mm` : null)
  const residual =
    verdict?.fit_residual_mm !== undefined
      ? verdict.fit_residual_mm.toExponential(0)
      : null
  const danglingDatums =
    verdict?.datum_statuses?.filter((d) => d.status?.toLowerCase() === 'dangling') ?? []

  return (
    <CardShell
      accent={accent}
      icon={certified ? ShieldCheck : PenLine}
      title={sym ? sym.name : card.characteristic}
      chip={
        certified ? (
          <Chip accent="pass" title="Evaluated exactly against the B-Rep by the kernel, at RFS">
            <ShieldCheck size={9} /> kernel-certified · RFS
          </Chip>
        ) : (
          <Chip
            accent="neutral"
            dashed
            title={`Design intent — the kernel has not verified this frame: ${intentReason ?? ''}`}
          >
            <PenLine size={9} /> design intent — uncertified
          </Chip>
        )
      }
    >
      {/* The frame itself — boxed tabular notation, glyphs pinned to the
          symbol font stack (.gdt-glyph / .fcf-frame CSS). */}
      <div
        className="fcf-frame mt-1.5 text-sm"
        dangerouslySetInnerHTML={{ __html: html }}
      />

      {verdict !== undefined && (
        <div className="mt-1.5 space-y-0.5 text-[11px]">
          {conforms === 'in_spec' && (
            <div className="flex items-baseline gap-1.5 text-emerald-600 dark:text-emerald-400">
              <Check size={11} className="shrink-0 translate-y-px" />
              <span>
                IN SPEC
                {measured !== null && (
                  <span className="text-foreground/80">
                    {' '}
                    — measured <span className="cad-readout">{measured}</span>
                    {residual !== null && <> , fit residual {residual}</>}
                  </span>
                )}
              </span>
            </div>
          )}
          {conforms === 'out_of_spec' && (
            <div className="flex items-baseline gap-1.5 text-red-600 dark:text-red-400">
              <X size={11} className="shrink-0 translate-y-px" />
              <span>
                OUT OF SPEC
                {measured !== null && (
                  <span className="text-foreground/80">
                    {' '}
                    — measured <span className="cad-readout">{measured}</span>
                    {residual !== null && <> , fit residual {residual}</>}
                  </span>
                )}
              </span>
            </div>
          )}
          {conforms === 'not_evaluable' && (
            <div className="flex items-baseline gap-1.5 text-amber-600 dark:text-amber-400">
              <CircleSlash size={11} className="shrink-0 translate-y-px" />
              <span>
                NOT EVALUABLE
                {verdict.reason && (
                  <span className="text-foreground/80"> — {verdict.reason}</span>
                )}
              </span>
            </div>
          )}
          {danglingDatums.length > 0 && (
            <div className="text-amber-400/90">
              {danglingDatums.map((d) => (
                // Geometry-anchoring slice: a dangling datum names a consumed
                // face — the natural place to highlight lineage in the
                // viewport once persistent-ID selection integration lands.
                <span key={d.label}>
                  datum {d.label} — DANGLING (source face consumed){' '}
                </span>
              ))}
            </div>
          )}
          {verdict.frame && (
            <div className="text-muted-foreground">
              DRF origin{' '}
              <span className="cad-readout">
                ({verdict.frame.origin.map((v) => fmtNum(v)).join(', ')})
              </span>
              {verdict.frame.derivation && <> — {verdict.frame.derivation}</>}
            </div>
          )}
        </div>
      )}

      {!certified && intentReason !== null && (
        <div className="mt-1 text-[11px] text-muted-foreground">{intentReason}</div>
      )}
      {dialectNote !== null && (
        <div className="mt-0.5 text-[11px] text-amber-400/80">{dialectNote}</div>
      )}
      {card.note && <div className="mt-1 text-[11px] text-foreground/70">{card.note}</div>}
    </CardShell>
  )
}
