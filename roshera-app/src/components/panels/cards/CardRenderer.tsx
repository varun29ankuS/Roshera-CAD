import { parseCard, type CardKind } from '@/lib/blackboard-cards'
import { ChoicesCard } from './ChoicesCard'
import { DfmCard } from './DfmCard'
import { FcfCard } from './FcfCard'
import { MergeCard } from './MergeCard'
import { RefusalCard } from './RefusalCard'
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
  const result = parseCard(kind, trimmedSource)

  if (!result.ok) {
    return (
      <div className="my-1 rounded border border-border/70 bg-foreground/5 p-2">
        <div className="mb-1 text-[10px] text-amber-400/80">
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
  }
}
