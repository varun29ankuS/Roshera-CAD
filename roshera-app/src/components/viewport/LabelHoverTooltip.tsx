import { useEffect, useState } from 'react'
import { Tag, AlertTriangle } from 'lucide-react'
import { useSceneStore } from '@/stores/scene-store'
import { usePartLabels } from './use-part-labels'
import type { Label, LabelKind } from '@/lib/labels-api'

/**
 * Hover tooltip that surfaces the part's named features so the user can
 * see — and frame questions around — what they are pointing at, the same
 * vocabulary the agent reads.
 *
 * Reuses the EXISTING hover path: `hoveredId` (set by every `CADMesh`'s
 * `onPointerOver`) and `hoveredSubElement` (set by the same mesh's
 * `onPointerMove` in face/edge/vertex selection mode). It builds no new
 * picking system — it only reads the hover state those handlers already
 * maintain and overlays the matching label names.
 *
 * Like `ExtrudeHoverTooltip`, it lives OUTSIDE the R3F canvas and tracks
 * the raw window pointer, so orbit / pan camera motion never desyncs the
 * panel from the cursor.
 *
 * ## What it shows
 *   * Object hover (no sub-element) → every label on the part. Labels are
 *     part-scoped, so a body hover is the right scope to list them.
 *   * Face / edge / vertex hover → narrows to labels of the hovered kind.
 *     The kernel doesn't ship a client-side picked-entity → label map, so
 *     we narrow by kind (honest: "labels of this kind on this part")
 *     rather than guessing a single match.
 *
 * Stale labels (their entity gone — `anchor === null`) render amber +
 * struck-through here too, consistent with the in-viewport callouts.
 */
export function LabelHoverTooltip() {
  const hoveredId = useSceneStore((s) => s.hoveredId)
  const hoveredSubElement = useSceneStore((s) => s.hoveredSubElement)

  // A hover is active if either the whole object or a sub-element is
  // hovered. We only fetch labels while something is hovered.
  const hoverActive = hoveredId !== null || hoveredSubElement !== null
  const { labels } = usePartLabels(hoverActive)

  const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null)

  useEffect(() => {
    if (!hoverActive) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setPointer(null)
      return
    }
    const onMove = (e: PointerEvent) => {
      setPointer({ x: e.clientX, y: e.clientY })
    }
    window.addEventListener('pointermove', onMove)
    return () => window.removeEventListener('pointermove', onMove)
  }, [hoverActive])

  if (!hoverActive || !pointer) return null
  if (labels.length === 0) return null

  // Narrow by the hovered sub-element kind when one is hovered; section
  // labels never correspond to a picked sub-element, so they only show
  // on a whole-object hover.
  const subKind: LabelKind | null = hoveredSubElement?.type ?? null
  const shown = subKind
    ? labels.filter((l) => l.kind === subKind)
    : labels
  if (shown.length === 0) return null

  // Offset so the tooltip never sits under the cursor (which would
  // intercept further raycasts and unhover the body). Matches
  // ExtrudeHoverTooltip's 14px diagonal.
  const left = pointer.x + 14
  const top = pointer.y + 14

  const heading = subKind ? `Labels · ${subKind}` : 'Labels'

  return (
    <div
      className="fixed z-40 pointer-events-none cad-panel px-2.5 py-1.5 text-[10px] uppercase tracking-wider min-w-[140px] max-w-[260px] shadow-lg"
      style={{ left, top }}
      role="tooltip"
    >
      <div className="text-muted-foreground mb-1">
        {heading} · {shown.length}
      </div>
      <ul className="space-y-0.5">
        {shown.map((label) => (
          <LabelRow key={label.name} label={label} />
        ))}
      </ul>
    </div>
  )
}

function LabelRow({ label }: { label: Label }) {
  const Icon = label.stale ? AlertTriangle : Tag
  return (
    <li className="flex items-center gap-1.5 normal-case tracking-normal text-[11px]">
      <Icon
        className={`w-3 h-3 shrink-0 ${label.stale ? 'text-amber-400' : 'text-muted-foreground'}`}
        aria-hidden
      />
      <span
        className={
          label.stale
            ? 'text-amber-300 line-through decoration-amber-400/70'
            : 'text-foreground'
        }
        title={label.stale ? 'stale: this name no longer points at a live entity' : undefined}
      >
        {label.name}
      </span>
      {label.description && (
        <span className="text-muted-foreground/70 truncate">
          {label.description}
        </span>
      )}
    </li>
  )
}
