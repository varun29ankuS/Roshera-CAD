import type { ComponentType } from 'react'
import { Droplet, Eye, ShieldCheck } from 'lucide-react'
import type { SoundnessCard as SoundnessCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip, Claim, ClaimBadge, KV, type CardAccent } from './card-chrome'
import { eulerGenus, fmtNum } from './format'

/**
 * SOUNDNESS CERTIFICATE CARD
 * ==========================
 * The kernel's per-part certificate, in the projection the MCP layer already
 * builds (roshera-mcp/src/core.ts): each invariant is TRI-STATE — proven,
 * failed, or NOT RUN — and the three are rendered distinctly because a check
 * that did not run is not a check that passed. `sound` is the authoritative
 * conjunction only when the full certificate ran; a partial certificate is
 * shown as partial, never rounded up.
 *
 * The nine invariants render as a wrapped row of compact `ClaimBadge`s (not
 * a vertical list — Varun, 2026-08-01: a sound part should read as one
 * glance of green, and nine lines all saying "fine" buries the one that
 * doesn't). A genuinely recognisable pictograph is used where one exists
 * (a droplet for "watertight", an eye for the dual-eye consistency check —
 * both common enough that no legend is needed); every other invariant is an
 * abstract B-Rep/topology term with no real-world icon, so it keeps its
 * short text label rather than inventing a private glyph. Full detail is on
 * hover either way.
 */
const INVARIANTS: ReadonlyArray<{
  key: keyof SoundnessCardData & string
  label: string
  glyph?: ComponentType<{ size?: number | string; className?: string }>
}> = [
  { key: 'brep_valid', label: 'B-Rep' },
  { key: 'watertight', label: 'watertight', glyph: Droplet },
  { key: 'manifold', label: 'manifold' },
  { key: 'self_intersection_free', label: 'self-int.' },
  { key: 'construction_consistent', label: 'construction' },
  { key: 'labels_consistent', label: 'labels' },
  { key: 'tessellation_clean', label: 'tessellation' },
  { key: 'mesh_quality_clean', label: 'mesh quality' },
  { key: 'eyes_consistent', label: 'dual-eye', glyph: Eye },
]

export function SoundnessCard({ card }: { card: SoundnessCardData }) {
  const sound = card.sound ?? null
  const accent: CardAccent = sound === true ? 'pass' : sound === false ? 'fail' : 'neutral'
  const notRun = INVARIANTS.filter((inv) => (card[inv.key] ?? null) === null).length
  const genus = card.euler_characteristic != null ? eulerGenus(card.euler_characteristic) : null

  return (
    <CardShell
      accent={accent}
      icon={ShieldCheck}
      title={card.part ? <>Soundness — {card.part}</> : 'Soundness certificate'}
      chip={
        sound === true ? (
          <Chip accent="pass" title="Every certified invariant holds — the kernel asserts a closed, sound solid">
            sound
          </Chip>
        ) : sound === false ? (
          <Chip accent="fail">unsound</Chip>
        ) : (
          <Chip accent="neutral" dashed title="The full certificate has not run — not-run is not a pass">
            partial · {notRun} not run
          </Chip>
        )
      }
    >
      <div className="mt-1.5 flex flex-wrap items-center gap-1">
        {INVARIANTS.map(({ key, label, glyph }) => {
          const value = (card[key] ?? null) as boolean | null
          return (
            <ClaimBadge
              key={key}
              status={value}
              label={label}
              glyph={glyph}
              detail={
                value === null
                  ? `${label}: not run — not-run is not a pass`
                  : `${label}: ${value ? 'proven' : 'FAILED'}`
              }
            />
          )
        })}
      </div>

      <div className="mt-1.5 grid grid-cols-2 gap-x-4 gap-y-0.5">
        {card.euler_characteristic != null && (
          <KV label="Euler characteristic χ">{fmtNum(card.euler_characteristic)}</KV>
        )}
        {card.open_edges != null && <KV label="open edges">{fmtNum(card.open_edges)}</KV>}
        {card.nonmanifold_edges != null && (
          <KV label="non-manifold edges">{fmtNum(card.nonmanifold_edges)}</KV>
        )}
        {card.face_count != null && <KV label="faces">{fmtNum(card.face_count)}</KV>}
        {card.volume != null && <KV label="volume">{fmtNum(card.volume)} mm³</KV>}
      </div>
      {card.euler_characteristic != null &&
        card.watertight === true &&
        card.manifold === true &&
        genus !== null && (
          <div className="mt-0.5 text-[10px] text-muted-foreground/80">
            genus {genus} — closed orientable surface, g = (2 − χ) / 2
          </div>
        )}

      {card.errors != null && card.errors.length > 0 && (
        <div className="mt-1.5 flex flex-col gap-0.5">
          {card.errors.map((e, i) => (
            <Claim key={i} status={false}>
              {e}
            </Claim>
          ))}
        </div>
      )}
    </CardShell>
  )
}
