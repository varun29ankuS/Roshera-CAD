import { ShieldCheck } from 'lucide-react'
import type { SoundnessCard as SoundnessCardData } from '@/lib/blackboard-cards'
import { CardShell, Chip, Claim, KV, type CardAccent } from './card-chrome'
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
 */

const INVARIANTS: ReadonlyArray<[keyof SoundnessCardData & string, string]> = [
  ['brep_valid', 'B-Rep'],
  ['watertight', 'watertight'],
  ['manifold', 'manifold'],
  ['self_intersection_free', 'self-intersection-free'],
  ['construction_consistent', 'construction'],
  ['labels_consistent', 'labels'],
  ['tessellation_clean', 'tessellation'],
  ['mesh_quality_clean', 'mesh quality'],
  ['eyes_consistent', 'dual-eye'],
]

export function SoundnessCard({ card }: { card: SoundnessCardData }) {
  const sound = card.sound ?? null
  const accent: CardAccent = sound === true ? 'pass' : sound === false ? 'fail' : 'neutral'
  const notRun = INVARIANTS.filter(([k]) => (card[k] ?? null) === null).length
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
      <div className="mt-1.5 flex flex-col gap-0.5">
        {INVARIANTS.map(([key, label]) => {
          const value = (card[key] ?? null) as boolean | null
          return (
            <Claim
              key={key}
              status={value}
              title={value === null ? `${label}: not run — not-run is not a pass` : `${label}: ${value ? 'proven' : 'FAILED'}`}
            >
              {label}
              {value === null && <span className="text-muted-foreground/70"> — not run</span>}
            </Claim>
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
