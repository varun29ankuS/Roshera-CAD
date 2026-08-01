import type { ComponentType } from 'react'
import { Droplet, ShieldCheck } from 'lucide-react'
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
 * doesn't). `label` is the badge's visible short text; `long` is the full
 * claim, used ONLY for the hover/`aria-label` detail — never abbreviated,
 * so a badge's tooltip is never shorter than the thing it certifies.
 *
 * Only ONE invariant gets a pictograph instead of a label: watertight's
 * droplet, because waterproof/leak iconography is genuinely universal (IP
 * ratings, packaging) — no legend needed. `dual-eye` LOOKED like a second
 * candidate (an eye glyph reads naturally as "vision"), but on reflection
 * it decodes to the wrong thing: a bare eye in a certificate row reads as
 * "visible/viewed," not "two independent synthetic viewpoints reconciled
 * face coverage" — a private mapping this codebase's own rule (never ship
 * a lookalike a reader has to be taught) forbids. Every other invariant is
 * an abstract B-Rep/topology term with no real-world icon at all, so all
 * eight keep short text labels rather than inventing glyphs for them.
 */
const INVARIANTS: ReadonlyArray<{
  key: keyof SoundnessCardData & string
  /** Visible badge text (or nothing, if `glyph` is set). */
  label: string
  /** Full claim name — hover/`aria-label` only, never truncated. */
  long: string
  glyph?: ComponentType<{ size?: number | string; className?: string }>
}> = [
  { key: 'brep_valid', label: 'B-Rep', long: 'B-Rep valid' },
  { key: 'watertight', label: 'watertight', long: 'watertight', glyph: Droplet },
  { key: 'manifold', label: 'manifold', long: 'manifold' },
  { key: 'self_intersection_free', label: 'no self-int.', long: 'self-intersection-free' },
  { key: 'construction_consistent', label: 'construction', long: 'construction consistent' },
  { key: 'labels_consistent', label: 'labels', long: 'labels consistent' },
  { key: 'tessellation_clean', label: 'tessellation', long: 'tessellation clean' },
  { key: 'mesh_quality_clean', label: 'mesh quality', long: 'mesh quality clean' },
  { key: 'eyes_consistent', label: 'dual-eye', long: 'dual-eye consistency' },
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
        {INVARIANTS.map(({ key, label, long, glyph }) => {
          const value = (card[key] ?? null) as boolean | null
          return (
            <ClaimBadge
              key={key}
              status={value}
              label={label}
              glyph={glyph}
              detail={
                value === null
                  ? `${long}: not run — not-run is not a pass`
                  : `${long}: ${value ? 'proven' : 'FAILED'}`
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
