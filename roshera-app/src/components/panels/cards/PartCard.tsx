import { Box } from '@/components/ui/icons'
import type { Exactness, PartCard as PartCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip, KV, type CardAccent } from './card-chrome'
import { CardText } from './CardText'
import { fmtNum } from './format'

/**
 * PART SUMMARY CARD
 * =================
 * "What did you build", answered in one glance: the part's size, volume,
 * mass and material, and the certificate's verdict in the corner. Numbers
 * are the wire's own (mass properties, placement, verify_part); a quantity
 * the kernel computed approximately says so beside the value, and one it
 * could not compute says "unavailable" — never a bare number presented as
 * exact. `sound` absent or null is "not certified": a dashed chip, never a
 * tick.
 */

/** The exactness word shown beside a value, or nothing when exact/absent. */
function exactnessDetail(e: Exactness | undefined): string | null {
  if (e === undefined || e.exactness === 'exact') return null
  if (e.exactness === 'unavailable') return 'unavailable'
  return `±${fmtNum(e.rel_error_bound * 100)}% (${e.method.replace(/_/g, ' ')})`
}

export function PartCard({ card }: { card: PartCardData }) {
  const sound = card.sound ?? null
  const accent: CardAccent = sound === true ? 'pass' : sound === false ? 'fail' : 'neutral'
  const name = card.part ?? `part ${card.part_id}`
  const volumeDetail = exactnessDetail(card.provenance?.volume)
  const comDetail = exactnessDetail(card.provenance?.center_of_mass)

  return (
    <CardShell
      accent={accent}
      icon={Box}
      title={name}
      chip={
        sound === true ? (
          <Chip accent="pass" title="The kernel certified this part sound — a closed, manifold solid">
            sound
          </Chip>
        ) : sound === false ? (
          <Chip accent="fail">unsound</Chip>
        ) : (
          <Chip accent="neutral" dashed title="No certificate on this summary — not certified is not sound">
            not certified
          </Chip>
        )
      }
    >
      {card.dimensions_world && (
        <div className="mt-1.5 font-mono text-[13px] tabular-nums text-foreground/90">
          {card.dimensions_world.map(fmtNum).join(' × ')}
          <span className="ml-1 text-[11px] text-muted-foreground">mm</span>
        </div>
      )}
      <div className="mt-1 grid grid-cols-2 gap-x-4 gap-y-0.5">
        {card.volume !== undefined && (
          <KV label="volume">
            {fmtNum(card.volume)} mm³
            {volumeDetail && <span className="ml-1 text-muted-foreground/80">{volumeDetail}</span>}
          </KV>
        )}
        {card.mass !== undefined && (
          <KV label="mass">
            {fmtNum(card.mass)} kg
            {volumeDetail && <span className="ml-1 text-muted-foreground/80">{volumeDetail}</span>}
          </KV>
        )}
        {card.material && (
          <KV label="material">
            {card.material.name}
            {card.material.density !== undefined && (
              <span className="ml-1 text-muted-foreground/80">{fmtNum(card.material.density)} kg/m³</span>
            )}
          </KV>
        )}
        {card.surface_area !== undefined && <KV label="surface area">{fmtNum(card.surface_area)} mm²</KV>}
        {card.center_of_mass && (
          <KV label="centre of mass">
            {card.center_of_mass.map(fmtNum).join(', ')} mm
            {comDetail && <span className="ml-1 text-muted-foreground/80">{comDetail}</span>}
          </KV>
        )}
      </div>
      {card.note && (
        <div className="mt-1.5 text-[11px] text-foreground/75">
          <CardText text={card.note} />
        </div>
      )}
    </CardShell>
  )
}
