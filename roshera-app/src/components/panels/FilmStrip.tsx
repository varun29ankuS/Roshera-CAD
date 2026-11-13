/**
 * The film strip: one frame per chapter, each a real picture of the part
 * right after that chapter (rendered by the kernel from its frame cache,
 * GET /api/timeline/frame/{branch}/{seq}/picture), with the chapter's tool
 * icon on its corner and its name underneath. The last frame -- the part as
 * it stands -- is outlined. Right-click a frame for the actions on the step
 * that defines it.
 */
import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, framePictureUrl, type Chapter } from '@/lib/chapters-api'
import { Box, ToolIcons, X } from '@/components/ui/icons'
import { frameIndexAt, stepTo } from '@/lib/history-view'
import { useHistoryView } from '@/stores/history-view-store'

const FRAME_PX = 64
/** A frame's width plus the gap after it: the pitch a drag steps by. */
const FRAME_PITCH = 72 + 8
const STRIP_PAD_LEFT = 12

/**
 * One frame's picture. Loaded through `fetch` -- not a bare `<img src>` --
 * so it carries the sign-in the app's fetch interceptor adds: with auth on,
 * an `<img>` request has no Authorization header and the picture route
 * refuses it. Rendered at 2x on a high-density screen; the object URL is
 * released when the frame changes or goes away.
 */
function FramePicture({ branch, seq, alt }: { branch: string; seq: number; alt: string }) {
  const [url, setUrl] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    const scale = typeof window !== 'undefined' && window.devicePixelRatio > 1 ? 2 : 1
    let objectUrl: string | null = null
    let live = true
    fetch(framePictureUrl(branch, seq, FRAME_PX * scale))
      .then((r) => (r.ok ? r.blob() : Promise.reject(new Error(`HTTP ${r.status}`))))
      .then((blob) => {
        if (!live) return
        objectUrl = URL.createObjectURL(blob)
        setUrl(objectUrl)
      })
      .catch(() => {
        if (live) setFailed(true)
      })
    return () => {
      live = false
      if (objectUrl) URL.revokeObjectURL(objectUrl)
    }
  }, [branch, seq])
  if (failed) {
    return <span className="flex h-full w-full items-center justify-center text-[11px] text-muted-foreground">no picture</span>
  }
  if (!url) return null
  return (
    <img
      src={url}
      width={FRAME_PX}
      height={FRAME_PX}
      alt={alt}
      draggable={false}
      className="block h-full w-full object-contain"
    />
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
  const strip = useRef<HTMLOListElement>(null)
  const dragging = useRef(false)
  const anchors = chapters.map((c) => c.anchor)

  // Show chapter i: the last chapter IS the live part.
  const show = (i: number) => {
    const c = chapters[i]
    if (!c) return
    if (i === chapters.length - 1) view(null)
    else if (viewing !== c.anchor) view(c.anchor, chapterTitle(c), branch)
  }
  // The frame under the pointer: the element actually there (robust to
  // scrolling and layout); past either end of the strip, the end frame.
  const indexAtPointer = (clientX: number, clientY: number) => {
    const el = strip.current
    if (!el) return 0
    const hit = document.elementFromPoint(clientX, clientY)?.closest('li[data-frame]')
    if (hit && el.contains(hit)) return Number(hit.getAttribute('data-frame'))
    const left = el.getBoundingClientRect().left + STRIP_PAD_LEFT - el.scrollLeft
    return frameIndexAt(clientX, left, FRAME_PITCH, chapters.length)
  }

  // The history changed under a past view (a new step, an undo): if the step
  // being viewed is gone, go back to the live part.
  useEffect(() => {
    if (viewing !== null && !anchors.includes(viewing)) view(null)
  })

  return (
    <ol
      ref={strip}
      tabIndex={0}
      aria-label="Film strip: the part after each feature. Click or drag to slide through the history; arrow keys step; Esc returns to the live part."
      onPointerDown={(e) => {
        if (e.button !== 0) return
        dragging.current = true
        try {
          e.currentTarget.setPointerCapture(e.pointerId)
        } catch {
          // No capturable pointer (a synthetic event): the drag still works
          // while the pointer stays over the strip.
        }
        show(indexAtPointer(e.clientX, e.clientY))
      }}
      onPointerMove={(e) => {
        if (dragging.current) show(indexAtPointer(e.clientX, e.clientY))
      }}
      onPointerUp={(e) => {
        dragging.current = false
        if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
      }}
      onKeyDown={(e) => {
        if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
        e.preventDefault()
        const next = stepTo(anchors, viewing, e.key === 'ArrowLeft' ? -1 : 1)
        const c = chapters.find((ch) => ch.anchor === next)
        view(next, c ? chapterTitle(c) : null, branch)
      }}
      className="flex min-w-0 flex-1 cursor-ew-resize select-none items-end gap-2 overflow-x-auto px-3 py-1.5 outline-none focus-visible:ring-1 focus-visible:ring-ring"
    >
      {chapters.map((c, i) => {
        const key = chapterIconKey(c.kind)
        const Icon = key ? ToolIcons[key] : Box
        // The frame the viewport is showing: a past one, or the live part.
        const current = viewing === null ? i === chapters.length - 1 : c.anchor === viewing
        const title = chapterTitle(c)
        return (
          <li
            key={c.anchor}
            data-frame={i}
            onContextMenu={onFrameContextMenu ? (e) => onFrameContextMenu(e, c.anchor) : undefined}
            title={c.sound ? title : `${title} — not sound: ${c.refusals[0]?.reason ?? c.refusals[0]?.status ?? ''}`}
            className="flex w-[72px] shrink-0 flex-col items-center gap-0.5"
          >
            <div
              className={cn(
                'relative overflow-hidden rounded border bg-background',
                current ? 'border-amber-500 ring-1 ring-amber-500' : 'border-border/60',
                // The chapter selected in the list: marked, not travelled to.
                !current && selectedChapter === c.anchor && 'border-sky-500 ring-1 ring-sky-500',
                !c.sound && 'border-destructive',
              )}
              style={{ width: FRAME_PX, height: FRAME_PX }}
            >
              <FramePicture key={`${branch}:${c.anchor}`} branch={branch} seq={c.anchor} alt={`The part after ${title}`} />
              <span className="absolute left-0.5 top-0.5">
                <Icon size={16} />
              </span>
              {!c.sound ? (
                <span className="absolute right-0.5 top-0.5">
                  <X size={14} />
                </span>
              ) : null}
            </div>
            <span className="w-full truncate text-center text-[11px] leading-4 text-muted-foreground">{title}</span>
          </li>
        )
      })}
    </ol>
  )
}
