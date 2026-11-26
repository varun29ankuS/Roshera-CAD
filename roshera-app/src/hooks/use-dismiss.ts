import { useEffect, useRef } from 'react'

/**
 * Close a popover on an outside press or Escape.
 *
 * The press is heard in the CAPTURE phase: a panel that stops a mouse-down
 * from travelling on (the Sheet panel does, to keep its presses off the
 * canvas) also stopped it reaching a listener on the way back up, so a menu
 * opened from that panel stayed open however often the panel was clicked.
 * Nothing below can keep a press from a capture listener on the window.
 */
export function useDismiss(open: boolean, onClose: () => void) {
  const ref = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey)
    }
  }, [open, onClose])
  return ref
}
