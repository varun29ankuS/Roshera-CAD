import { parseCard, type CardKind } from '@/lib/blackboard-cards'
import { isStructuredPayload } from '@/lib/card-payload'
import { ChoicesCard } from './ChoicesCard'
import { ConceptCard } from './ConceptCard'
import { DfmCard } from './DfmCard'
import { FcfCard } from './FcfCard'
import { MeasuresCard } from './MeasuresCard'
import { MergeCard } from './MergeCard'
import { PartCard } from './PartCard'
import { RefusalCard } from './RefusalCard'
import { SheetCard } from './SheetCard'
import { SoundnessCard } from './SoundnessCard'

/**
 * Dispatch a `roshera:*` fence payload to its typed card renderer. A payload
 * that fails validation renders as the RAW fence with the validation error —
 * an honest fallback, never a half-card built from fields that were not
 * there. (The raw source also remains reachable by clicking the line into
 * edit mode, as with any Blackboard line.)
 */
export function CardRenderer({ kind, source }: { kind: CardKind; source: string }) {
  const trimmedSource = source.replace(/\n$/, '')
  // A tool's readable result line in a card fence is shown as that line,
  // not as a failed card (an engineer reads it; there is nothing to fix).
  if (!isStructuredPayload(trimmedSource)) {
    return (
      <pre className="my-1 overflow-x-auto whitespace-pre-wrap rounded border border-border/70 bg-foreground/5 p-2 font-mono text-[0.85em] text-foreground/85">
        {trimmedSource}
      </pre>
    )
  }
  const result = parseCard(kind, trimmedSource)

  if (!result.ok) {
    return (
      <div className="my-1 rounded border border-border/70 bg-foreground/5 p-2">
        <div className="mb-1 text-[11px] text-caution">
          roshera:{kind} payload failed validation — {result.error}
        </div>
        <pre className="overflow-x-auto font-mono text-[0.8em] text-foreground/70">{source}</pre>
      </div>
    )
  }

  const card = result.card
  switch (card.kind) {
    case 'dfm':
      return <DfmCard card={card.card} />
    case 'fcf':
      return <FcfCard card={card.card} />
    case 'refusal':
      return <RefusalCard card={card.card} />
    case 'merge':
      return <MergeCard card={card.card} />
    case 'soundness':
      return <SoundnessCard card={card.card} />
    case 'choices':
      return <ChoicesCard card={card.card} source={trimmedSource} />
    case 'part':
      return <PartCard card={card.card} />
    case 'sheet':
      return <SheetCard card={card.card} />
    case 'measures':
      return <MeasuresCard card={card.card} />
    case 'concept':
      return <ConceptCard card={card.card} />
  }
}
