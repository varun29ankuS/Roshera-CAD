import { Check, CircleSlash, Ruler, X } from '@/components/ui/icons'
import type { MeasuresCard as MeasuresCardData, MeasureRow } from '@/lib/blackboard-cards'
import { cn } from '@/lib/utils'
import { CardShell, Chip, type CardAccent } from './card-chrome'
import { CardText } from './CardText'
import { fmtNum } from './format'

/**
 * MEASURED-VALUES CARD
 * ====================
 * Several numbers at once, as a table an engineer scans down: quantity,
 * value + unit, the limit it is judged against, where it comes from, and a
 * per-row tick / cross. A row with no `pass` is a reported value (no glyph);
 * `pass: null` is a check that could not be decided (amber slash) — never
 * rounded to a tick. Labels, limits and sources typeset their inline math.
 */

function RowGlyph({ pass }: { pass: MeasureRow['pass'] }) {
  if (pass === undefined) return null
  return (
    <span
      className={cn(
        'inline-flex',
        pass === true && 'text-emerald-600 dark:text-emerald-400',
        pass === false && 'text-red-600 dark:text-red-400',
        pass === null && 'text-caution',
      )}
      title={pass === true ? 'within the limit' : pass === false ? 'outside the limit' : 'could not be decided'}
    >
      {pass === true ? <Check size={11} /> : pass === false ? <X size={11} /> : <CircleSlash size={11} />}
    </span>
  )
}

export function MeasuresCard({ card }: { card: MeasuresCardData }) {
  const votes = card.rows.map((r) => r.pass).filter((p) => p !== undefined)
  const failed = votes.filter((p) => p === false).length
  const undecided = votes.filter((p) => p === null).length
  const accent: CardAccent =
    votes.length === 0 ? 'neutral' : failed > 0 ? 'fail' : undecided > 0 ? 'warn' : 'pass'
  const showLimit = card.rows.some((r) => r.limit !== undefined)
  const showSource = card.rows.some((r) => r.source !== undefined)
  const title = card.title ?? (card.part ? `Measured — ${card.part}` : 'Measured values')

  return (
    <CardShell
      accent={accent}
      icon={Ruler}
      title={<CardText text={title} />}
      chip={
        votes.length === 0 ? undefined : failed > 0 ? (
          <Chip accent="fail">{failed} out of limit</Chip>
        ) : undecided > 0 ? (
          <Chip accent="warn" dashed>
            {undecided} undecided
          </Chip>
        ) : (
          <Chip accent="pass">all within limits</Chip>
        )
      }
    >
      <div className="mt-1.5 overflow-x-auto">
        <table className="w-full border-collapse text-[11px] tabular-nums">
          <thead>
            <tr className="border-b border-border/50">
              <th className="w-4" />
              <th className="py-0.5 pr-2 text-left font-mono text-[11px] font-normal uppercase tracking-wide text-muted-foreground">
                quantity
              </th>
              <th className="py-0.5 pr-2 text-right font-mono text-[11px] font-normal uppercase tracking-wide text-muted-foreground">
                value
              </th>
              {showLimit && (
                <th className="py-0.5 pr-2 text-left font-mono text-[11px] font-normal uppercase tracking-wide text-muted-foreground">
                  limit
                </th>
              )}
              {showSource && (
                <th className="py-0.5 text-left font-mono text-[11px] font-normal uppercase tracking-wide text-muted-foreground">
                  source
                </th>
              )}
            </tr>
          </thead>
          <tbody>
            {card.rows.map((r, i) => (
              <tr key={i} className="border-b border-border/30 last:border-b-0">
                <td className="py-0.5 align-baseline">
                  <RowGlyph pass={r.pass} />
                </td>
                <td className="py-0.5 pr-2 align-baseline text-foreground/90">
                  <CardText text={r.quantity} />
                </td>
                <td className="py-0.5 pr-2 text-right align-baseline font-mono text-foreground/90">
                  {typeof r.value === 'number' ? fmtNum(r.value) : <CardText text={r.value} />}
                  {r.unit && <span className="ml-1 text-muted-foreground">{r.unit}</span>}
                </td>
                {showLimit && (
                  <td className="py-0.5 pr-2 align-baseline text-foreground/75">
                    {r.limit === undefined ? null : typeof r.limit === 'number' ? fmtNum(r.limit) : <CardText text={r.limit} />}
                  </td>
                )}
                {showSource && (
                  <td className="py-0.5 align-baseline text-muted-foreground">
                    {r.source && <CardText text={r.source} />}
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {card.note && (
        <div className="mt-1.5 text-[11px] text-foreground/75">
          <CardText text={card.note} />
        </div>
      )}
    </CardShell>
  )
}
