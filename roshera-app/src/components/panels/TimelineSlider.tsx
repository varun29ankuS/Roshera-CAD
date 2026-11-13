/**
 * The history slider (Varun 2026-11-13: "literally like a slider in a video
 * file ... slid forward and backwards"). One track, a mark per chapter, the
 * right end the live part. It is the only control that travels in time.
 *
 * Heavy-CAD behaviour, as a video player scrubs: while the pointer hovers or
 * drags, a preview card shows the step's PICTURE (small, cached) above the
 * track -- instant at any part size; the viewport loads that step's 3D only
 * when the thumb is let go. The steps either side of the one shown are
 * fetched ahead. Play walks the chapters to the live end.
 *
 * A general time axis: its marks are chapters today; simulation time steps
 * (FEA load steps, CFD / heat-transfer time, mode shapes) will ride the same
 * control.
 */
import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterTitle, type Chapter } from '@/lib/chapters-api'
import { markFraction, nearestMark, nextPlayIndex } from '@/lib/slider'
import { prefetchStep, useHistoryView } from '@/stores/history-view-store'
import { FramePicture } from './FilmStrip'
import { Pause, Play } from '@/components/ui/icons'

const PLAY_STEP_MS = 900
const PREVIEW_PX = 144

export function TimelineSlider({ chapters, branch = 'main' }: { chapters: Chapter[]; branch?: string }) {
  const viewing = useHistoryView((s) => s.step)
  const view = useHistoryView((s) => s.view)
  const track = useRef<HTMLDivElement>(null)
  const [hover, setHover] = useState<number | null>(null)
  const [dragging, setDragging] = useState(false)
  // The drag as handlers see it: a press and its release can arrive before
  // a re-render, and a release reading the `dragging` STATE of the render
  // before the press ignored itself -- every other click moved the thumb but
  // never the view (seen live). The ref is always current.
  const dragRef = useRef(false)
  const [playing, setPlaying] = useState(false)
  const count = chapters.length
  const last = count - 1

  // The thumb sits on the step being viewed, or the live end.
  const shownIndex = viewing === null ? last : Math.max(0, chapters.findIndex((c) => c.anchor === viewing))
  const thumbIndex = dragging && hover !== null ? hover : shownIndex

  const goTo = (i: number) => {
    const c = chapters[i]
    if (!c) return
    if (i === last) view(null)
    else view(c.anchor, chapterTitle(c), branch)
  }

  // Fetch the neighbours of the step shown, so stepping is instant.
  useEffect(() => {
    for (const i of [shownIndex - 1, shownIndex + 1]) {
      const c = chapters[i]
      if (c && i !== last) prefetchStep(c.anchor, branch)
    }
  }, [shownIndex, chapters, last, branch])

  // Play: one chapter per beat until the live end.
  useEffect(() => {
    if (!playing) return
    const t = window.setTimeout(() => {
      const next = nextPlayIndex(shownIndex, count)
      if (next === null) setPlaying(false)
      else goTo(next)
    }, PLAY_STEP_MS)
    return () => window.clearTimeout(t)
  })

  const indexAt = (clientX: number) => {
    const el = track.current
    if (!el) return 0
    const r = el.getBoundingClientRect()
    return nearestMark(clientX, r.left, r.width, count)
  }

  if (count === 0) return null
  const preview = hover !== null ? chapters[hover] : null

  return (
    <div className="flex items-center gap-2 px-3 pt-1.5">
      <button
        type="button"
        aria-label={playing ? 'Pause' : 'Play the history'}
        title={playing ? 'Pause' : 'Play the history from here'}
        onClick={() => {
          if (!playing && shownIndex === last) goTo(0)
          setPlaying((p) => !p)
        }}
        className="shrink-0 rounded p-1 hover:bg-accent/50"
      >
        {playing ? <Pause size={16} /> : <Play size={16} />}
      </button>
      <div
        ref={track}
        role="slider"
        tabIndex={0}
        aria-label="History: slide to see the part after each feature"
        aria-valuemin={0}
        aria-valuemax={last}
        aria-valuenow={thumbIndex}
        aria-valuetext={chapters[thumbIndex] ? chapterTitle(chapters[thumbIndex]) : undefined}
        onPointerDown={(e) => {
          if (e.button !== 0) return
          setPlaying(false)
          dragRef.current = true
          setDragging(true)
          try {
            e.currentTarget.setPointerCapture(e.pointerId)
          } catch {
            // A synthetic pointer cannot be captured; dragging still works
            // while the pointer stays on the track.
          }
          setHover(indexAt(e.clientX))
        }}
        onPointerMove={(e) => setHover(indexAt(e.clientX))}
        onPointerUp={(e) => {
          if (!dragRef.current) return
          dragRef.current = false
          setDragging(false)
          if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
          // Let go: now load the 3D of that step.
          goTo(indexAt(e.clientX))
        }}
        onPointerLeave={() => {
          if (!dragRef.current) setHover(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowLeft') goTo(Math.max(0, shownIndex - 1))
          else if (e.key === 'ArrowRight') goTo(Math.min(last, shownIndex + 1))
          else if (e.key === 'Home') goTo(0)
          else if (e.key === 'End') goTo(last)
          else return
          e.preventDefault()
          setPlaying(false)
        }}
        className="relative h-6 flex-1 cursor-pointer touch-none select-none outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
      >
        {/* The track, filled up to the thumb. */}
        <div className="absolute left-0 right-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-border" />
        <div
          className="absolute left-0 top-1/2 h-1 -translate-y-1/2 rounded-full bg-amber-500"
          style={{ width: `${markFraction(thumbIndex, count) * 100}%` }}
        />
        {/* A mark per chapter; a refused chapter's mark is red. */}
        {chapters.map((c, i) => (
          <span
            key={c.anchor}
            className={cn(
              'absolute top-1/2 h-2.5 w-0.5 -translate-x-1/2 -translate-y-1/2 rounded-full',
              c.sound ? 'bg-foreground/40' : 'bg-destructive',
            )}
            style={{ left: `${markFraction(i, count) * 100}%` }}
          />
        ))}
        {/* The thumb. */}
        <span
          className="absolute top-1/2 h-3.5 w-3.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-background bg-amber-500 shadow"
          style={{ left: `${markFraction(thumbIndex, count) * 100}%` }}
        />
        {/* The preview: the step's picture and name, above the track. */}
        {preview && hover !== null ? (
          <div
            className="pointer-events-none absolute bottom-7 -translate-x-1/2 cad-panel p-1"
            style={{ left: `${markFraction(hover, count) * 100}%`, width: PREVIEW_PX + 8 }}
          >
            <div style={{ width: PREVIEW_PX, height: PREVIEW_PX }}>
              <FramePicture
                key={`${branch}:${preview.anchor}:${PREVIEW_PX}`}
                branch={branch}
                seq={preview.anchor}
                px={PREVIEW_PX}
                alt={`The part after ${chapterTitle(preview)}`}
              />
            </div>
            <div className="truncate px-1 pt-0.5 text-center text-[11px] text-foreground">
              {hover + 1}/{count} · {chapterTitle(preview)}
              {hover === last ? ' · live' : ''}
            </div>
          </div>
        ) : null}
      </div>
      <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
        {thumbIndex + 1}/{count}
      </span>
    </div>
  )
}
