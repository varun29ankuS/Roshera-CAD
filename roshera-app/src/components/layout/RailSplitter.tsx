import { cn } from '@/lib/utils'
import type { RailSplitState } from './useRailSplit'

/**
 * The visible divider. A hairline at rest with a generous invisible grab
 * area — a 1px target is honest about where the boundary is and dishonest
 * about how hard it is to hit.
 */
export function RailSplitter({
  dragging,
  ...handlers
}: RailSplitState['handleProps'] & { dragging: boolean }) {
  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize the agent view — drag, or arrow keys; double-click to fit its content"
      title="Drag to resize · double-click to fit"
      tabIndex={0}
      className={cn(
        'group relative h-1 shrink-0 cursor-ns-resize touch-none',
        'focus-visible:outline-none',
      )}
      {...handlers}
    >
      {/* The grab area extends past the visible line without taking layout
          space from either neighbour. */}
      <span aria-hidden className="absolute inset-x-0 -top-1.5 -bottom-1.5" />
      <span
        aria-hidden
        className={cn(
          'absolute inset-x-0 top-0 h-px transition-colors',
          dragging
            ? 'bg-primary'
            : 'bg-border group-hover:bg-primary/60 group-focus-visible:bg-primary',
        )}
      />
    </div>
  )
}
