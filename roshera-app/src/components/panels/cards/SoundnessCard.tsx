import type { ComponentType } from 'react'
import {
  Box,
  Droplet,
  Eye,
  GitMerge,
  Grid3x3,
  Move3d,
  Ruler,
  ShieldCheck,
  Square,
  Tags,
  Wrench,
} from '@/components/ui/icons'
import {
  invariantState,
  type InvariantValue,
  type SoundnessCard as SoundnessCardData,
} from '@/lib/blackboard-cards'
import { CardShell, CertRow, CertRows, Chip, Claim, KV, type CardAccent } from './card-chrome'
import { eulerGenus, fmtNum } from './format'

/**
 * SOUNDNESS CERTIFICATE CARD
 * ==========================
 * The kernel's per-part certificate, in the projection the MCP layer already
 * builds (roshera-mcp/src/core.ts): each invariant is TRI-STATE — proven,
 * failed, or NOT RUN — and the three are rendered distinctly because a check
 * that did not run is not a check that passed. `sound` is the authoritative
 * conjunction only when the full certificate ran; a partial certificate is
 * shown as partial, never rounded up. The consistency conjuncts have a
 * fourth state, not_applicable (nothing to check on this part): a grey "n/a",
 * neither a tick nor a cross, and not counted as not-run.
 *
 * The invariants render ONE PER LINE (Varun, 2026-11-26, superseding the
 * 2026-08-01 wrapped badge row: "line by line with a right tick at the end
 * with icons ... so that the whole thing looks very professional"): an icon,
 * the full claim name, the measured evidence where the certificate carries
 * it, and the verdict aligned at the right edge. The icon only marks the
 * row; the full name beside it carries the meaning, so no icon has to be
 * learned. A failed line is set heavier, so the one that does not hold still
 * stands out in a column of ticks.
 */
type InvariantKey =
  | 'brep_valid'
  | 'watertight'
  | 'manifold'
  | 'shells_outward'
  | 'self_intersection_free'
  | 'construction_consistent'
  | 'labels_consistent'
  | 'tessellation_clean'
  | 'mesh_quality_clean'
  | 'eyes_consistent'

/** Badge status and hover word per state. `na` is its own state: the check
 *  had nothing to judge on this part, which is neither a pass nor a fail. */
const BADGE: Record<ReturnType<typeof invariantState>, { status: boolean | null | 'na'; word: string }> = {
  pass: { status: true, word: 'proven' },
  fail: { status: false, word: 'FAILED' },
  na: { status: 'na', word: 'not applicable to this part' },
  not_run: { status: null, word: 'not run — not-run is not a pass' },
}

const INVARIANTS: ReadonlyArray<{
  key: InvariantKey
  /** The full claim, shown on its line, never abbreviated. */
  name: string
  /** Hover text: what the claim asserts. */
  long: string
  icon: ComponentType<{ size?: number | string; className?: string }>
  /** The measured evidence for this line, when the card carries it. */
  value?: (card: SoundnessCardData) => string | undefined
}> = [
  {
    key: 'brep_valid',
    name: 'B-Rep valid',
    long: 'B-Rep valid',
    icon: Box,
    value: (c) => (c.face_count != null ? `${fmtNum(c.face_count)} faces` : undefined),
  },
  {
    key: 'watertight',
    name: 'Watertight',
    long: 'watertight: every edge shared by exactly two faces',
    icon: Droplet,
    value: (c) => (c.open_edges != null ? `${fmtNum(c.open_edges)} open edges` : undefined),
  },
  {
    key: 'manifold',
    name: 'Manifold',
    long: 'manifold',
    icon: Square,
    value: (c) =>
      c.nonmanifold_edges != null ? `${fmtNum(c.nonmanifold_edges)} non-manifold` : undefined,
  },
  { key: 'shells_outward', name: 'Shells face outward', long: 'every shell faces out of the material', icon: Move3d },
  { key: 'self_intersection_free', name: 'No self-intersection', long: 'self-intersection-free', icon: GitMerge },
  { key: 'construction_consistent', name: 'Construction consistent', long: 'construction consistent', icon: Wrench },
  { key: 'labels_consistent', name: 'Labels consistent', long: 'labels consistent', icon: Tags },
  { key: 'tessellation_clean', name: 'Tessellation clean', long: 'tessellation clean', icon: Grid3x3 },
  { key: 'mesh_quality_clean', name: 'Mesh quality', long: 'mesh quality clean', icon: Ruler },
  {
    key: 'eyes_consistent',
    name: 'Dual-view consistent',
    long: 'two independent synthetic viewpoints reconcile face coverage',
    icon: Eye,
  },
]

export function SoundnessCard({ card }: { card: SoundnessCardData }) {
  const sound = card.sound ?? null
  const accent: CardAccent = sound === true ? 'pass' : sound === false ? 'fail' : 'neutral'
  const stateOf = (key: InvariantKey) => invariantState(card[key] as InvariantValue)
  const notRun = INVARIANTS.filter((inv) => stateOf(inv.key) === 'not_run').length
  const genus = card.euler_characteristic != null ? eulerGenus(card.euler_characteristic) : null

  return (
    <CardShell
      accent={accent}
      icon={ShieldCheck}
      title={
        card.part ? (
          <>Soundness — {card.part}</>
        ) : card.part_id != null ? (
          <>Soundness — part {card.part_id}</>
        ) : (
          'Soundness certificate'
        )
      }
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
      <CertRows>
        {INVARIANTS.map(({ key, name, long, icon, value }) => {
          const badge = BADGE[stateOf(key)]
          return (
            <CertRow
              key={key}
              status={badge.status}
              name={name}
              icon={icon}
              value={value?.(card)}
              title={`${long}: ${badge.word}`}
            />
          )
        })}
      </CertRows>

      {card.verdict && <div className="mt-1.5 text-[11px] text-foreground/80">{card.verdict}</div>}

      <div className="mt-1.5 grid grid-cols-2 gap-x-4 gap-y-0.5">
        {card.euler_characteristic != null && (
          <KV label="Euler characteristic χ">{fmtNum(card.euler_characteristic)}</KV>
        )}
        {card.volume != null && <KV label="volume">{fmtNum(card.volume)} mm³</KV>}
      </div>
      {card.euler_characteristic != null &&
        card.watertight === true &&
        card.manifold === true &&
        genus !== null && (
          <div className="mt-0.5 text-[11px] text-muted-foreground/80">
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
