/**
 * Context-menu primitive — THE one right-click menu implementation.
 *
 * The app grew three hand-rolled context menus (viewport, model tree,
 * document tabs) that disagreed about positioning, dismissal, and
 * keyboard behaviour. This module is the single shared implementation;
 * new right-click surfaces must reuse it rather than hand-rolling a
 * fourth.
 *
 * Behaviour, in one place:
 *   - Portals to `document.body` so no ancestor `overflow`/`backdrop-
 *     filter` can clip it (the model-tree menu was invisible for
 *     exactly that reason before it portalled).
 *   - Edge-aware placement: flips inward when the click lands close
 *     enough to a viewport edge that the natural layout would clip.
 *   - Dismissal: outside pointer-down, Escape, or any item click.
 *   - Keyboard: focuses the first enabled item on open; ArrowUp/Down
 *     rove focus (wrapping), Home/End jump, Enter/Space activate.
 *   - Destructive items render `danger` (red) so they are
 *     distinguishable BEFORE the click, and disabled items explain
 *     themselves via `title` rather than sitting mute.
 */

import { useCallback, useEffect, useLayoutEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/utils'

export function ContextMenu({
  x,
  y,
  onClose,
  children,
  'aria-label': ariaLabel,
}: {
  x: number
  y: number
  onClose: () => void
  children: React.ReactNode
  'aria-label'?: string
}) {
  const ref = useRef<HTMLDivElement>(null)

  // Edge-aware positioning — render hidden at the raw coordinates,
  // measure, then flip inward if the menu would clip. Positioning is a
  // direct DOM mutation (before paint) rather than state: the DOM is the
  // external system being synchronised, and there is nothing for React
  // to re-render.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight
    const margin = 8
    let nx = x
    let ny = y
    if (nx + rect.width > vw - margin) nx = Math.max(margin, x - rect.width)
    if (ny + rect.height > vh - margin) ny = Math.max(margin, y - rect.height)
    el.style.left = `${nx}px`
    el.style.top = `${ny}px`
    el.style.visibility = 'visible'
  }, [x, y])

  // Focus the first enabled item once positioned (effects run after the
  // layout effect above), so the menu is keyboard-operable immediately
  // after a right-click.
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const first = el.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')
    first?.focus()
  }, [x, y])

  useEffect(() => {
    const onPointerDown = (e: MouseEvent) => {
      const el = ref.current
      if (el && e.target instanceof Node && el.contains(e.target)) return
      onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('mousedown', onPointerDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onPointerDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return
    const el = ref.current
    if (!el) return
    const items = Array.from(
      el.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'),
    )
    if (items.length === 0) return
    e.preventDefault()
    const current = items.indexOf(document.activeElement as HTMLButtonElement)
    let next: number
    switch (e.key) {
      case 'ArrowDown':
        next = current < 0 ? 0 : (current + 1) % items.length
        break
      case 'ArrowUp':
        next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length
        break
      case 'Home':
        next = 0
        break
      default:
        next = items.length - 1
    }
    items[next].focus()
  }, [])

  return createPortal(
    <div
      ref={ref}
      role="menu"
      aria-label={ariaLabel}
      onKeyDown={handleKeyDown}
      className="cad-panel-floating fixed z-[1000] min-w-[170px] select-none rounded-md py-1 text-[12px]"
      style={{ left: x, top: y, visibility: 'hidden' }}
    >
      {children}
    </div>,
    document.body,
  )
}

export function ContextMenuItem({
  children,
  onClick,
  danger,
  disabled,
  title,
}: {
  children: React.ReactNode
  onClick: () => void
  /** Destructive action — red before the click, not after. */
  danger?: boolean
  disabled?: boolean
  /** Hover/focus explanation; use it to say WHY an item is disabled. */
  title?: string
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={cn(
        'cad-focus flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors',
        disabled
          ? 'cursor-not-allowed text-muted-foreground/40'
          : danger
            ? 'text-destructive hover:bg-destructive/10'
            : 'text-foreground hover:bg-accent/40',
      )}
    >
      {children}
    </button>
  )
}

export function ContextMenuSeparator() {
  return <div role="separator" className="my-1 border-t border-border/50" />
}

/** Non-interactive header naming what the menu acts on. */
export function ContextMenuHeader({ children }: { children: React.ReactNode }) {
  return (
    <div className="truncate px-3 py-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground/70">
      {children}
    </div>
  )
}
