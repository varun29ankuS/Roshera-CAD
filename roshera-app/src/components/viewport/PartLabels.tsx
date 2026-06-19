import { Html } from '@react-three/drei'
import { Tag, AlertTriangle } from 'lucide-react'
import { useSceneStore } from '@/stores/scene-store'
import { usePartLabels } from './use-part-labels'
import type { Label } from '@/lib/labels-api'

/**
 * LABELLER overlay — renders each of the active part's labels as a
 * billboard callout anchored at its world point, so the USER can read
 * the part's named features (and frame questions around them) the same
 * way the agent can.
 *
 * Lives inside the R3F `<Canvas>` and uses drei's `<Html>` (the overlay
 * primitive already used by `SketchOverlay`), so each callout tracks its
 * world anchor through orbit / pan / zoom without us re-projecting by
 * hand.
 *
 * ## Verified vs stale
 * A label whose named entity still resolves carries a world `anchor` and
 * renders in the normal blueprint tone. A label whose entity was deleted
 * or regenerated away comes back with `anchor === null` — the kernel
 * could not place it — so we mark it stale: amber, struck-through, and
 * pinned at the world origin with a warning glyph, never silently
 * dropped. This is Varun's "different colour for unverified": the user
 * sees that the name is held but its assertion no longer holds.
 *
 * Mounts only when the labels overlay is toggled on (`labelsVisible`).
 */
export function PartLabels() {
  const labelsVisible = useSceneStore((s) => s.labelsVisible)
  const { labels } = usePartLabels(labelsVisible)

  if (!labelsVisible) return null

  // Anchored (verified) labels billboard at their world point; stale
  // ones have no world point, so they are collected into a single
  // origin-pinned stack rather than overlapping at [0,0,0].
  const anchored = labels.filter(
    (l): l is Label & { anchor: [number, number, number] } =>
      l.anchor !== null,
  )
  const stale = labels.filter((l) => l.anchor === null)

  return (
    <group name="part-labels">
      {anchored.map((label) => (
        <Html
          key={label.name}
          position={label.anchor}
          center
          pointerEvents="none"
          zIndexRange={[90, 0]}
        >
          <LabelChip label={label} />
        </Html>
      ))}
      {stale.length > 0 && (
        <Html
          position={[0, 0, 0]}
          center
          pointerEvents="none"
          zIndexRange={[90, 0]}
        >
          <div className="flex flex-col items-center gap-0.5 select-none">
            {stale.map((label) => (
              <LabelChip key={label.name} label={label} />
            ))}
          </div>
        </Html>
      )}
    </group>
  )
}

/**
 * One callout chip. Verified labels read in the blueprint foreground
 * tone with a tag glyph; stale labels switch to amber with a warning
 * glyph and a struck-through name so "this name no longer points at
 * anything" is legible at a glance.
 */
function LabelChip({ label }: { label: Label }) {
  const tone = label.stale
    ? 'border-amber-400/60 text-amber-300 bg-amber-950/50'
    : 'border-border/70 text-foreground bg-background/85'
  const Icon = label.stale ? AlertTriangle : Tag
  return (
    <div
      title={
        label.stale
          ? `${label.name} — stale: the ${label.kind} this name pinned no longer exists`
          : label.description
            ? `${label.name} — ${label.description}`
            : `${label.name} (${label.kind})`
      }
      className={`inline-flex items-center gap-1 px-1.5 h-5 whitespace-nowrap text-[10px] font-medium tracking-tight border ${tone} backdrop-blur-sm shadow-sm`}
    >
      <Icon className="w-3 h-3 shrink-0" aria-hidden />
      <span className={label.stale ? 'line-through decoration-amber-400/70' : ''}>
        {label.name}
      </span>
    </div>
  )
}
