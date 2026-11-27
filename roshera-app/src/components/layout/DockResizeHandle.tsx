import { useCallback, useRef } from 'react'
import { DOCK_MAX, DOCK_MIN } from '@/lib/dock-width'
import { useDockStore, useDockWidth } from '@/stores/dock-store'

/**
 * The left edge of the docked Blackboard: drag it to set the dock's width
 * (320 to 520 px, remembered). Double-click puts it back to the default.
 * With the keyboard: focus it, then the arrow keys move it 16 px at a time
 * and Home puts it back.
 */
export function DockResizeHandle() {
  const width = useDockWidth()
  const setSaved = useDockStore((s) => s.setSaved)
  const reset = useDockStore((s) => s.reset)
  const drag = useRef<{ startX: number; startWidth: number } | null>(null)

  const onPointerDown = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      if (e.button !== 0) return
      e.preventDefault()
      e.currentTarget.setPointerCapture(e.pointerId)
      drag.current = { startX: e.clientX, startWidth: width }
    },
    [width],
  )
  const onPointerMove = useCallback(
    (e: React.PointerEvent<HTMLDivElement>) => {
      const d = drag.current
      if (!d) return
      // The dock is on the right: dragging left makes it wider.
      setSaved(d.startWidth + (d.startX - e.clientX))
    },
    [setSaved],
  )
  const endDrag = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    drag.current = null
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId)
  }, [])

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Blackboard width"
      aria-valuemin={DOCK_MIN}
      aria-valuemax={DOCK_MAX}
      aria-valuenow={width}
      tabIndex={0}
      title="Drag to resize the Blackboard · double-click for the default width"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onDoubleClick={reset}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft') setSaved(width + 16)
        else if (e.key === 'ArrowRight') setSaved(width - 16)
        else if (e.key === 'Home') reset()
        else return
        e.preventDefault()
      }}
      className="group/resize pointer-events-auto absolute inset-y-2 -left-1.5 z-10 flex w-3 cursor-col-resize touch-none items-center justify-center outline-none"
    >
      <span className="h-10 w-0.5 rounded-full bg-border-strong/0 transition-colors group-hover/resize:bg-border-strong group-focus-visible/resize:bg-ring group-active/resize:bg-ring" />
    </div>
  )
}
