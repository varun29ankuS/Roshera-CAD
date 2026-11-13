/**
 * The film strip: one frame per chapter, each a real picture of the part
 * right after that chapter (rendered by the kernel from its frame cache,
 * GET /api/timeline/frame/{branch}/{seq}/picture), with the chapter's tool
 * icon on its corner and its name underneath. The last frame -- the part as
 * it stands -- is outlined. Right-click a frame for the actions on the step
 * that defines it.
 */
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, framePictureUrl, type Chapter } from '@/lib/chapters-api'
import { Box, ToolIcons, X } from '@/components/ui/icons'

const FRAME_PX = 64

export function FilmStrip({
  chapters,
  branch = 'main',
  onFrameContextMenu,
}: {
  chapters: Chapter[]
  branch?: string
  onFrameContextMenu?: (e: React.MouseEvent, anchor: number) => void
}) {
  return (
    <ol
      aria-label="Film strip: the part after each feature"
      className="flex min-w-0 flex-1 items-end gap-2 overflow-x-auto px-3 py-1.5"
    >
      {chapters.map((c, i) => {
        const key = chapterIconKey(c.kind)
        const Icon = key ? ToolIcons[key] : Box
        const current = i === chapters.length - 1
        const title = chapterTitle(c)
        return (
          <li
            key={c.anchor}
            onContextMenu={onFrameContextMenu ? (e) => onFrameContextMenu(e, c.anchor) : undefined}
            title={c.sound ? title : `${title} — not sound: ${c.refusals[0]?.reason ?? c.refusals[0]?.status ?? ''}`}
            className="flex w-[72px] shrink-0 flex-col items-center gap-0.5"
          >
            <div
              className={cn(
                'relative overflow-hidden rounded border bg-background',
                current ? 'border-amber-500 ring-1 ring-amber-500' : 'border-border/60',
                !c.sound && 'border-destructive',
              )}
              style={{ width: FRAME_PX, height: FRAME_PX }}
            >
              <img
                src={framePictureUrl(branch, c.anchor, FRAME_PX)}
                srcSet={`${framePictureUrl(branch, c.anchor, FRAME_PX)} 1x, ${framePictureUrl(branch, c.anchor, FRAME_PX * 2)} 2x`}
                width={FRAME_PX}
                height={FRAME_PX}
                alt={`The part after ${title}`}
                loading="lazy"
                decoding="async"
                draggable={false}
                className="block h-full w-full object-contain"
              />
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
