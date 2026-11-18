import { cn } from '@/lib/utils'
import { Box, Eye, EyeOff, MousePointer2, X } from '@/components/ui/icons'
import { useUserContextStore } from '@/stores/user-context-store'
import { MORE_KEY, chipIds, chipText, type SelectionAttachment, type SelectionItem } from '@/lib/selection-attachment'
import type { SelectionChips } from '@/hooks/use-selection-chips'

/**
 * The person's selection as chips (lib/selection-attachment.ts): on the
 * composer, what the next message will carry, each removable for this ask;
 * on a sent line, what that message carried, frozen. A chip is the entity
 * ("Flange · face 56") and the kernel's measure of it ("plane · 1256.64 mm²"),
 * its ids in the tooltip. Transparent washes, no blur, 11 px.
 */

const CHIP = 'inline-flex max-w-full min-w-0 items-center gap-1 rounded border px-1.5 py-[3px] text-[11px] leading-none'

function ChipBody({ item }: { item: SelectionItem }) {
  const { title, measure } = chipText(item)
  return (
    <>
      {item.kind === 'part' ? (
        <Box size={10} className="shrink-0 text-primary" aria-hidden />
      ) : (
        <MousePointer2 size={10} className="shrink-0 text-primary" aria-hidden />
      )}
      <span className="truncate text-foreground">{title}</span>
      {measure && <span className="shrink-0 font-mono text-muted-foreground">{measure}</span>}
    </>
  )
}

/** On the composer: the chips the next message carries, then the sharing
 *  toggle the status bar's "Agent sees" also flips. Paused with something
 *  selected, the row says the selection will not be attached. */
export function ComposerSelectionChips({ chips }: { chips: SelectionChips }) {
  const setSharing = useUserContextStore((s) => s.setSharing)
  const shown = chips.chips.filter((c) => !chips.removed.has(c.key))
  const moreShown = chips.more > 0 && !chips.removed.has(MORE_KEY)

  if (chips.pausedWithSelection) {
    return (
      <div className="flex flex-wrap items-center gap-1.5 px-2 pt-1.5 text-[11px] text-muted-foreground">
        <EyeOff size={11} className="shrink-0" aria-hidden />
        <span>Selection not attached · sharing paused</span>
        <button
          type="button"
          onClick={() => setSharing(true)}
          className="cad-focus rounded px-1 text-foreground underline-offset-2 hover:underline"
        >
          Share again
        </button>
      </div>
    )
  }
  if (shown.length === 0 && !moreShown) return null

  return (
    <div
      role="list"
      aria-label="Selection attached to this message"
      className="flex flex-wrap items-center gap-1 px-2 pt-1.5"
    >
      {shown.map(({ key, item }) => {
        const { title } = chipText(item)
        return (
          <span
            key={key}
            role="listitem"
            title={`Attached: ${title} · ${chipIds(item)}`}
            className={cn(CHIP, 'border-primary/30 bg-primary/[0.06]')}
          >
            <ChipBody item={item} />
            <button
              type="button"
              onClick={() => chips.remove(key)}
              aria-label={`Do not attach ${title}`}
              title="Do not attach to this message (the viewport selection stays)"
              className="cad-focus -mr-0.5 shrink-0 rounded text-muted-foreground hover:text-foreground"
            >
              <X size={10} />
            </button>
          </span>
        )
      })}
      {moreShown && (
        <span
          role="listitem"
          title="Selected but not listed: the agent is told how many"
          className={cn(CHIP, 'border-dashed border-primary/30 text-muted-foreground')}
        >
          +{chips.more} more
          <button
            type="button"
            onClick={() => chips.remove(MORE_KEY)}
            aria-label={`Do not mention the other ${chips.more} selected`}
            className="cad-focus -mr-0.5 shrink-0 rounded hover:text-foreground"
          >
            <X size={10} />
          </button>
        </span>
      )}
      <button
        type="button"
        onClick={() => setSharing(false)}
        title="Pause sharing what you point at and select: nothing is attached"
        aria-label="Pause sharing your selection with the agent"
        className="cad-focus ml-auto inline-flex shrink-0 items-center gap-1 rounded px-1 text-[11px] text-muted-foreground hover:text-foreground"
      >
        <Eye size={11} aria-hidden />
        Agent sees
      </button>
    </div>
  )
}

/** On a sent line: what the message carried, read back from its text. */
export function LineSelectionChips({ attachment }: { attachment: SelectionAttachment }) {
  return (
    <span role="list" aria-label="Selection sent with this message" className="mt-1 flex flex-wrap gap-1 font-normal">
      {attachment.items.map((item, i) => (
        <span
          key={`${item.object_id}|${item.kind}|${item.id ?? ''}|${i}`}
          role="listitem"
          title={chipIds(item)}
          className={cn(CHIP, 'border-primary/25 bg-transparent')}
        >
          <ChipBody item={item} />
        </span>
      ))}
      {attachment.more > 0 && (
        <span role="listitem" className={cn(CHIP, 'border-dashed border-primary/25 text-muted-foreground')}>
          +{attachment.more} more
        </span>
      )}
    </span>
  )
}
