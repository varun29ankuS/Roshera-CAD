import { DraftingCompass } from '@/components/ui/icons'
import type { SheetCard as SheetCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip, Claim, ClaimBadge, KV, type CardAccent } from './card-chrome'
import { CardText } from './CardText'

/**
 * DRAWING SHEET SUMMARY CARD
 * ==========================
 * "Is the sheet ready", answered in one glance: paper, scale, projection,
 * views, dimension count, and the sheet certificate's claims as badges —
 * `sound` (everything the sheet shows is true of the model), `complete`
 * (the sheet shows every model dimension) and the layout check. They are
 * separate claims and stay separate: a sound sheet can still be
 * incomplete, and that is exactly the gap a reader must see. Not checked
 * is a dashed amber badge, never a tick.
 */

const BADGE_WORD = (v: boolean | null) => (v === true ? 'proven' : v === false ? 'FAILED' : 'not checked')

export function SheetCard({ card }: { card: SheetCardData }) {
  const sound = card.sound ?? null
  const complete = card.complete ?? null
  const layout = card.quality ? card.quality.passed : null
  const failed = sound === false || complete === false || layout === false
  const ready = !failed && sound === true && complete === true
  const accent: CardAccent = failed ? 'fail' : ready ? 'pass' : 'warn'
  const title = card.name ?? 'Drawing sheet'
  const issueCount = card.quality?.issues?.length ?? 0

  return (
    <CardShell
      accent={accent}
      icon={DraftingCompass}
      title={title}
      chip={
        <>
          {card.sheet && <Chip accent="neutral">{card.sheet}</Chip>}
          {card.scale && <Chip accent="neutral">{card.scale}</Chip>}
          {ready ? (
            <Chip accent="pass" title="Sound, complete and laid out cleanly — the sheet can be released">
              certified
            </Chip>
          ) : failed ? (
            <Chip accent="fail">not ready</Chip>
          ) : (
            <Chip accent="warn" dashed title="Part of the sheet certificate has not been checked — not checked is not a pass">
              not checked
            </Chip>
          )}
        </>
      }
    >
      <div className="mt-1.5 flex flex-wrap items-center gap-1">
        <ClaimBadge
          status={sound}
          label="sound"
          detail={`sound — every printed fact matches the model: ${BADGE_WORD(sound)}`}
        />
        <ClaimBadge
          status={complete}
          label="complete"
          detail={`complete — every model dimension is on a view or signed off: ${BADGE_WORD(complete)}`}
        />
        <ClaimBadge
          status={layout}
          label="layout"
          detail={`layout check — no collisions or redundant dimensions: ${BADGE_WORD(layout)}`}
        />
      </div>
      <div className="mt-1.5 grid grid-cols-2 gap-x-4 gap-y-0.5">
        {card.projection && (
          <KV label="projection">{card.projection === 'first' ? 'first angle (ISO)' : 'third angle (ASME)'}</KV>
        )}
        {card.views && card.views.length > 0 && <KV label="views">{card.views.join(', ')}</KV>}
        {card.dimensions_placed !== undefined && <KV label="dimensions placed">{card.dimensions_placed}</KV>}
        {card.dimensions_omitted !== undefined && <KV label="dimensions omitted">{card.dimensions_omitted}</KV>}
        {layout === false && <KV label="layout findings">{issueCount}</KV>}
      </div>
      {card.omission_summary && card.omission_summary.length > 0 && (
        <div className="mt-1.5 flex flex-col gap-0.5">
          {card.omission_summary.map((o, i) => (
            <Claim key={i} status={false}>
              {o}
            </Claim>
          ))}
        </div>
      )}
      {card.note && (
        <div className="mt-1.5 text-[11px] text-foreground/75">
          <CardText text={card.note} />
        </div>
      )}
    </CardShell>
  )
}
