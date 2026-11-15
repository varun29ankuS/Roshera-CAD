import { useEffect } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useLensStore } from '@/stores/lens-store'
import { fetchLensFace, lensStoryRows, type LensStatus } from '@/lib/lens'
import { StatusPill, type StatusTone } from '@/components/ui/status-pill'
import { InSlot } from './overlay-slots'

const TONE: Record<LensStatus, StatusTone> = {
  as_written: 'positive',
  repaired: 'caution',
  blocking: 'negative',
}
const LABEL: Record<LensStatus, string> = {
  as_written: 'as written',
  repaired: 'repaired',
  blocking: 'blocking',
}
const MARK = { tick: '✓', amber: '▲', cross: '✕' } as const
const MARK_CLASS = { tick: 'text-positive', amber: 'text-caution', cross: 'text-destructive' } as const

/**
 * Kernel Lens story card. In Lens mode, picking a face of the part opens
 * what the kernel did to that face and why: blockers first, then repairs,
 * each with the rule it followed and the numbers it measured. A transparent,
 * closable overlay on the canvas.
 */
export function LensStoryCard() {
  const objectId = useLensStore((s) => s.objectId)
  const solidId = useLensStore((s) => s.solidId)
  const face = useLensStore((s) => s.face)
  const setFace = useLensStore((s) => s.setFace)
  const picks = useSceneStore((s) => s.subElementSelections)

  // The newest face picked on the Lens part opens its story.
  const picked = [...picks].reverse().find((p) => p.objectId === objectId && p.type === 'face')
  const pickedIndex = picked?.index ?? null
  useEffect(() => {
    if (solidId === null || pickedIndex === null) return
    let live = true
    fetchLensFace(solidId, pickedIndex)
      .then((f) => {
        if (live) setFace(f)
      })
      .catch(() => {
        if (live) setFace(null)
      })
    return () => {
      live = false
    }
  }, [solidId, pickedIndex, setFace])

  if (!objectId || !face) return null
  const rows = lensStoryRows(face.events)

  return (
    <InSlot name="right">
      <div className="pointer-events-auto cad-panel w-[300px] rounded border text-[11px]">
        <div className="flex items-center gap-2 border-b border-border px-2 py-1.5">
          <span className="font-semibold">Face {face.face_id}</span>
          <StatusPill tone={TONE[face.status]} label={LABEL[face.status]} />
          <button
            onClick={() => setFace(null)}
            className="ml-auto rounded px-1.5 py-0.5 hover:bg-accent"
            title="Close the face's story"
          >
            ✕
          </button>
        </div>
        {rows.length === 0 ? (
          <div className="px-2 py-2 text-muted-foreground">
            <span className={MARK_CLASS.tick}>{MARK.tick}</span> Imported as written. The kernel changed nothing here.
          </div>
        ) : (
          <ul className="flex flex-col">
            {rows.map((r, i) => (
              <li key={i} className="flex gap-2 border-b border-border px-2 py-1.5 last:border-b-0">
                <span className={`shrink-0 ${MARK_CLASS[r.mark]}`}>{MARK[r.mark]}</span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-baseline gap-2">
                    <span className="font-medium">{r.title}</span>
                    {r.source && <span className="font-mono text-muted-foreground">{r.source}</span>}
                  </div>
                  <div className="text-muted-foreground">{r.why}</div>
                  {r.numbers && <div className="font-mono tabular-nums">{r.numbers}</div>}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </InSlot>
  )
}
