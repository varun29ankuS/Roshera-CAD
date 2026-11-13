import { useRef, useState } from 'react'
import { X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { ToolIcons } from '@/components/ui/icons'
import { useExtrudeStore } from '@/stores/extrude-store'
import {
  commitFaceExtrude,
  faceExtrudeBody,
  parseExtrudeDistance,
  previewFaceExtrude,
  type ExtrudeDirection,
} from '@/lib/face-extrude'

/** Wait this long after the last keystroke before asking for a preview. */
const PREVIEW_DELAY_MS = 180

/**
 * The Extrude tool: pick a face, type a distance, choose out (add) or in
 * (cut), see the result as a ghost, OK. The arrow on the face still drags;
 * both go through lib/face-extrude.ts, so a typed extrude and a dragged one
 * are the same request.
 */
export function ExtrudePanel() {
  const open = useExtrudeStore((s) => s.open)
  const target = useExtrudeStore((s) => s.target)
  const setOpen = useExtrudeStore((s) => s.setOpen)
  const setPreview = useExtrudeStore((s) => s.setPreview)

  const [text, setText] = useState('5')
  const [direction, setDirection] = useState<ExtrudeDirection>('out')
  const [status, setStatus] = useState<'idle' | 'previewing' | 'no-shape' | 'committing'>('idle')
  const [error, setError] = useState<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const abort = useRef<AbortController | null>(null)
  const seq = useRef(0)

  if (!open) return null

  const distance = parseExtrudeDistance(text, direction)

  /** Ask for the ghost of `d`, after a short pause; a newer request wins. */
  const schedulePreview = (d: number | null) => {
    if (timer.current) clearTimeout(timer.current)
    abort.current?.abort()
    const mine = ++seq.current
    if (d === null || !target) {
      setPreview(null, null)
      setStatus('idle')
      return
    }
    timer.current = setTimeout(async () => {
      const controller = new AbortController()
      abort.current = controller
      setStatus('previewing')
      try {
        const mesh = await previewFaceExtrude(
          faceExtrudeBody(target.objectId, target.faceId, d, target.axis),
          controller.signal,
        )
        if (mine !== seq.current) return
        setPreview(d, mesh)
        setStatus(mesh ? 'idle' : 'no-shape')
      } catch {
        if (mine === seq.current) setStatus('idle')
      }
    }, PREVIEW_DELAY_MS)
  }

  const close = () => {
    if (timer.current) clearTimeout(timer.current)
    abort.current?.abort()
    seq.current++
    setError(null)
    setStatus('idle')
    setOpen(false)
  }

  const commit = async () => {
    if (!target || distance === null) return
    setStatus('committing')
    setError(null)
    const result = await commitFaceExtrude(faceExtrudeBody(target.objectId, target.faceId, distance, target.axis))
    if (result.ok) {
      close()
    } else {
      setStatus('idle')
      setError(result.reason)
    }
  }

  const ExtrudeIcon = ToolIcons.extrude

  return (
    <section
      aria-label="Extrude"
      className="pointer-events-auto w-64 overflow-hidden rounded-lg border border-border bg-card/95 text-xs shadow-md"
      onKeyDown={(e) => {
        if (e.key === 'Escape') close()
        if (e.key === 'Enter') void commit()
      }}
    >
      <div className="flex items-center gap-2 border-b border-border/70 px-3 py-2">
        <ExtrudeIcon size={18} />
        <span className="flex-1 text-[13px] font-semibold">Extrude</span>
        <button type="button" aria-label="Cancel" onClick={close} className="cad-focus rounded p-0.5 text-muted-foreground hover:text-foreground">
          <X size={14} />
        </button>
      </div>

      {!target ? (
        <p className="px-3 py-3 text-muted-foreground">Pick a face on the part to push or pull.</p>
      ) : (
        <div className="flex flex-col gap-2.5 px-3 py-2.5">
          <div role="radiogroup" aria-label="Direction" className="grid grid-cols-2 gap-1 rounded-md bg-muted/70 p-0.5">
            {(
              [
                ['out', 'Out · add'],
                ['in', 'In · cut'],
              ] as const
            ).map(([d, label]) => (
              <button
                key={d}
                type="button"
                role="radio"
                aria-checked={direction === d}
                onClick={() => {
                  setDirection(d)
                  schedulePreview(parseExtrudeDistance(text, d))
                }}
                className={cn(
                  'cad-focus rounded px-2 py-1',
                  direction === d ? 'bg-background font-semibold shadow-sm' : 'text-muted-foreground',
                )}
              >
                {label}
              </button>
            ))}
          </div>
          <label className="flex items-center gap-2">
            <span className="w-16 text-muted-foreground">Distance</span>
            <input
              autoFocus
              value={text}
              inputMode="decimal"
              onChange={(e) => {
                setText(e.target.value)
                schedulePreview(parseExtrudeDistance(e.target.value, direction))
              }}
              onFocus={() => schedulePreview(distance)}
              className={cn(
                'cad-focus h-7 w-20 rounded border bg-background/70 px-2 font-mono',
                distance === null ? 'border-red-500/60' : 'border-border/70',
              )}
            />
            <span className="text-muted-foreground">mm</span>
          </label>
          <p className="min-h-4 text-[11px] text-muted-foreground">
            {distance === null
              ? 'Type a non-zero distance.'
              : status === 'previewing'
                ? 'Previewing…'
                : status === 'no-shape'
                  ? 'The kernel has no shape at this distance.'
                  : 'The ghost shows the result.'}
          </p>
          {error && <p className="text-[11px] text-red-700 dark:text-red-300">{error}</p>}
          <div className="flex justify-end gap-1.5">
            <button type="button" onClick={close} className="cad-focus rounded border border-border px-3 py-1">
              Cancel
            </button>
            <button
              type="button"
              disabled={distance === null || status === 'committing'}
              onClick={() => void commit()}
              className="cad-focus rounded bg-[#2F6FDB] px-4 py-1 font-semibold text-white disabled:opacity-50"
            >
              {status === 'committing' ? 'Extruding…' : 'OK'}
            </button>
          </div>
        </div>
      )}
    </section>
  )
}
