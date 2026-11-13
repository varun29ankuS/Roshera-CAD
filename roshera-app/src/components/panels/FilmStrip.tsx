/**
 * The history track, built the way video editors build a timeline (Premiere,
 * Resolve, Final Cut -- Varun 2026-11-13): one row of clips, a clip per
 * feature (a real picture of the part right after it, rendered by the
 * kernel from its frame cache, with the tool icon and the name), and a
 * PLAYHEAD line across them. Drag the playhead or click a clip to travel; the
 * viewport is the viewer -- it shows that step as the playhead moves (no
 * pop-up preview). Clips after the playhead fade back, as below a rollback
 * bar. Transport: start, back, play / pause, forward, end (live). Right-click
 * a clip for the actions on the step that defines it.
 *
 * Motion (iOS-grade): the playhead glides on an ease-out spring curve when it
 * jumps and follows the pointer exactly while dragging; the current clip
 * lifts; the track scrolls smoothly to keep the playhead in view. All motion
 * stops for users who ask the system for reduced motion.
 */
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, framePictureUrl, type Chapter } from '@/lib/chapters-api'
import { Box, ChevronRight, Pause, Play, ToolIcons, X } from '@/components/ui/icons'
import { frameIndexAt, stepTo } from '@/lib/history-view'
import { nextPlayIndex } from '@/lib/slider'
import { prefetchStep, useHistoryView } from '@/stores/history-view-store'

// Compact clips (Varun 2026-11-13): the viewport is the viewer; the track is
// an index.
const FRAME_PX = 44
/** A clip's width plus the gap after it: the pitch a drag steps by. */
const FRAME_PITCH = 52 + 8
const STRIP_PAD_LEFT = 12
/** iOS's sheet / spring-like ease-out: fast start, long soft settle. */
const GLIDE = 'cubic-bezier(0.32, 0.72, 0, 1)'
const GLIDE_MS = 380
const PLAY_STEP_MS = 850

/** Pictures already fetched, by branch, step and pixel size: a picture seen
 *  once shows instantly after. Bounded; an evicted picture's object URL is
 *  released. */
const MAX_PICTURES = 300
const pictures = new Map<string, string>()

function keepPicture(key: string, url: string) {
  pictures.set(key, url)
  while (pictures.size > MAX_PICTURES) {
    const oldest = pictures.keys().next().value
    if (oldest === undefined) break
    const old = pictures.get(oldest)
    pictures.delete(oldest)
    if (old) URL.revokeObjectURL(old)
  }
}

function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true
}

/**
 * One step's picture at `px` CSS pixels. Loaded through `fetch` -- not a bare
 * `<img src>` -- so it carries the sign-in the app's fetch interceptor adds:
 * with auth on, an `<img>` request has no Authorization header and the
 * picture route refuses it. Rendered at 2x on a high-density screen; fades in
 * when it arrives.
 */
export function FramePicture({
  branch,
  seq,
  alt,
  px = FRAME_PX,
}: {
  branch: string
  seq: number
  alt: string
  px?: number
}) {
  const scale = typeof window !== 'undefined' && window.devicePixelRatio > 1 ? 2 : 1
  const key = `${branch}:${seq}:${px * scale}`
  const [url, setUrl] = useState<string | null>(() => pictures.get(key) ?? null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    if (pictures.has(key)) return
    let live = true
    fetch(framePictureUrl(branch, seq, px * scale))
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((blob) => {
        const objectUrl = URL.createObjectURL(blob)
        keepPicture(key, objectUrl)
        if (live) setUrl(objectUrl)
      })
      .catch(() => {
        if (live) setFailed(true)
      })
    return () => {
      live = false
    }
  }, [branch, seq, px, scale, key])
  if (failed) {
    return <span className="flex h-full w-full items-center justify-center text-[11px] text-muted-foreground">no picture</span>
  }
  if (!url) return null
  return (
    <img
      src={url}
      width={px}
      height={px}
      alt={alt}
      draggable={false}
      className="block h-full w-full object-contain animate-in fade-in duration-300 motion-reduce:animate-none"
    />
  )
}

function TransportButton({
  label,
  onClick,
  children,
}: {
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="grid h-7 w-7 place-items-center rounded-md text-foreground/80 transition-[background-color,transform] duration-150 hover:bg-accent/60 active:scale-90 motion-reduce:transition-none"
    >
      {children}
    </button>
  )
}

export function FilmStrip({
  chapters,
  branch = 'main',
  onFrameContextMenu,
}: {
  chapters: Chapter[]
  branch?: string
  onFrameContextMenu?: (e: React.MouseEvent, anchor: number) => void
}) {
  const viewing = useHistoryView((s) => s.step)
  const selectedChapter = useHistoryView((s) => s.selectedChapter)
  const view = useHistoryView((s) => s.view)
  const scroller = useRef<HTMLDivElement>(null)
  const dragRef = useRef(false)
  const [dragging, setDragging] = useState(false)
  const [playing, setPlaying] = useState(false)
  const anchors = chapters.map((c) => c.anchor)
  const count = chapters.length
  const last = count - 1

  // The clip the playhead is on: a past one, or the live end.
  const at = viewing === null ? last : Math.max(0, anchors.indexOf(viewing))

  const show = (i: number) => {
    const c = chapters[i]
    if (!c) return
    if (i === last) view(null)
    else if (viewing !== c.anchor) view(c.anchor, chapterTitle(c), branch)
  }

  // The clip under the pointer: the element actually there; past either end
  // of the track, the end clip.
  const indexAtPointer = (clientX: number, clientY: number) => {
    const el = scroller.current
    if (!el) return 0
    const hit = document.elementFromPoint(clientX, clientY)?.closest('li[data-frame]')
    if (hit && el.contains(hit)) return Number(hit.getAttribute('data-frame'))
    const left = el.getBoundingClientRect().left + STRIP_PAD_LEFT - el.scrollLeft
    return frameIndexAt(clientX, left, FRAME_PITCH, count)
  }

  // The history changed under a past view: if its step is gone, go live.
  useEffect(() => {
    if (viewing !== null && !anchors.includes(viewing)) view(null)
  })

  // Fetch the clips either side of the playhead, so stepping is instant.
  useEffect(() => {
    for (const i of [at - 1, at + 1]) {
      const c = chapters[i]
      if (c && i !== last) prefetchStep(c.anchor, branch)
    }
  }, [at, chapters, last, branch])

  // Play: one clip per beat, gliding, until the live end.
  useEffect(() => {
    if (!playing) return
    const t = window.setTimeout(() => {
      const next = nextPlayIndex(at, count)
      if (next === null) setPlaying(false)
      else show(next)
    }, PLAY_STEP_MS)
    return () => window.clearTimeout(t)
  })

  // Keep the playhead in view as it moves (smoothly, unless dragging).
  useLayoutEffect(() => {
    const el = scroller.current
    const clip = el?.querySelector<HTMLElement>(`li[data-frame="${at}"]`)
    if (!el || !clip) return
    const left = clip.offsetLeft
    const right = left + clip.offsetWidth
    if (left < el.scrollLeft + 24 || right > el.scrollLeft + el.clientWidth - 24) {
      el.scrollTo({
        left: Math.max(0, left - el.clientWidth / 2 + clip.offsetWidth / 2),
        behavior: dragRef.current || prefersReducedMotion() ? 'auto' : 'smooth',
      })
    }
  }, [at])

  const reduced = prefersReducedMotion()
  const playheadX = STRIP_PAD_LEFT + at * FRAME_PITCH + (FRAME_PITCH - 8) / 2
  const stop = () => setPlaying(false)

  return (
    <div className="flex min-w-0 flex-1 items-center gap-1 pl-2">
      <div className="flex shrink-0 items-center gap-0.5" role="group" aria-label="Transport">
        <TransportButton
          label="To the start"
          onClick={() => {
            stop()
            show(0)
          }}
        >
          <ChevronRight size={14} className="rotate-180" />
        </TransportButton>
        <TransportButton
          label="Back one feature"
          onClick={() => {
            stop()
            show(Math.max(0, at - 1))
          }}
        >
          <ChevronRight size={12} className="rotate-180" />
        </TransportButton>
        <TransportButton
          label={playing ? 'Pause' : 'Play the history'}
          onClick={() => {
            if (!playing && at === last) show(0)
            setPlaying((p) => !p)
          }}
        >
          {playing ? <Pause size={16} /> : <Play size={16} />}
        </TransportButton>
        <TransportButton
          label="Forward one feature"
          onClick={() => {
            stop()
            show(Math.min(last, at + 1))
          }}
        >
          <ChevronRight size={12} />
        </TransportButton>
        <TransportButton
          label="To the end (the live part)"
          onClick={() => {
            stop()
            show(last)
          }}
        >
          <ChevronRight size={14} />
        </TransportButton>
      </div>

      <div
        ref={scroller}
        role="slider"
        tabIndex={0}
        aria-label="History: drag the playhead or click a feature to see the part after it. Arrow keys step, Space plays, Esc returns to the live part."
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={at}
        aria-valuetext={chapters[at] ? chapterTitle(chapters[at]) : undefined}
        onPointerDown={(e) => {
          if (e.button !== 0) return
          stop()
          dragRef.current = true
          setDragging(true)
          try {
            e.currentTarget.setPointerCapture(e.pointerId)
          } catch {
            // A synthetic pointer cannot be captured; the drag still works
            // while the pointer stays on the track.
          }
          show(indexAtPointer(e.clientX, e.clientY))
        }}
        onPointerMove={(e) => {
          if (dragRef.current) show(indexAtPointer(e.clientX, e.clientY))
        }}
        onPointerUp={(e) => {
          if (!dragRef.current) return
          dragRef.current = false
          setDragging(false)
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
        }}
        onKeyDown={(e) => {
          let next: number
          if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
            const step = stepTo(anchors, viewing, e.key === 'ArrowLeft' ? -1 : 1)
            next = step === null ? last : anchors.indexOf(step)
          } else if (e.key === 'Home') next = 0
          else if (e.key === 'End') next = last
          else if (e.key === ' ') {
            e.preventDefault()
            setPlaying((p) => !p)
            return
          } else return
          e.preventDefault()
          stop()
          show(next)
        }}
        className="relative min-w-0 flex-1 cursor-grab touch-none select-none overflow-x-auto rounded-md outline-none focus-visible:ring-1 focus-visible:ring-ring active:cursor-grabbing [scrollbar-width:thin]"
      >
        <ol aria-hidden className="relative flex items-end gap-2 px-3 pb-1 pt-2">
          {chapters.map((c, i) => {
            const key = chapterIconKey(c.kind)
            const Icon = key ? ToolIcons[key] : Box
            const current = i === at
            const ahead = i > at
            const title = chapterTitle(c)
            return (
              <li
                key={c.anchor}
                data-frame={i}
                onContextMenu={onFrameContextMenu ? (e) => onFrameContextMenu(e, c.anchor) : undefined}
                title={c.sound ? title : `${title} — not sound: ${c.refusals[0]?.reason ?? c.refusals[0]?.status ?? ''}`}
                className="flex w-[52px] shrink-0 flex-col items-center gap-0.5"
                style={{
                  opacity: ahead ? 0.42 : 1,
                  transform: current ? 'translateY(-3px) scale(1.06)' : 'none',
                  transition: reduced ? 'none' : `transform 260ms ${GLIDE}, opacity 220ms ease-out`,
                }}
              >
                <div
                  className={cn(
                    'relative overflow-hidden rounded-md border bg-background',
                    current ? 'border-amber-500 shadow-[0_6px_14px_-6px_rgba(0,0,0,0.45)]' : 'border-border/60',
                    !current && selectedChapter === c.anchor && 'border-sky-500 ring-1 ring-sky-500',
                    !c.sound && 'border-destructive',
                  )}
                  style={{
                    width: FRAME_PX,
                    height: FRAME_PX,
                    transition: reduced ? 'none' : 'box-shadow 260ms ease-out, border-color 200ms ease-out',
                  }}
                >
                  <FramePicture key={`${branch}:${c.anchor}`} branch={branch} seq={c.anchor} alt={`The part after ${title}`} />
                  <span className="absolute left-0 top-0">
                    <Icon size={14} />
                  </span>
                  {!c.sound ? (
                    <span className="absolute right-0 top-0">
                      <X size={12} />
                    </span>
                  ) : null}
                </div>
                <span
                  className={cn(
                    'w-full truncate text-center text-[11px] leading-4',
                    current ? 'font-medium text-foreground' : 'text-muted-foreground',
                  )}
                >
                  {title}
                </span>
              </li>
            )
          })}
          {/* The playhead: a line across the clips with a head on top; glides
              when it jumps, follows the pointer exactly while dragging. */}
          <span
            aria-hidden
            className="pointer-events-none absolute bottom-0 left-0 top-0 w-0"
            style={{
              transform: `translateX(${playheadX}px)`,
              transition: dragging || reduced ? 'none' : `transform ${GLIDE_MS}ms ${GLIDE}`,
            }}
          >
            <span className="absolute -left-[5px] top-0 h-2.5 w-2.5 rounded-full bg-amber-500 shadow-[0_0_0_2px_var(--background)]" />
            <span className="absolute bottom-0 left-[-1px] top-1 w-0.5 rounded-full bg-amber-500/90" />
          </span>
        </ol>
      </div>

      <span className="shrink-0 px-1 text-[11px] tabular-nums text-muted-foreground">
        {at + 1}/{count}
      </span>
    </div>
  )
}
