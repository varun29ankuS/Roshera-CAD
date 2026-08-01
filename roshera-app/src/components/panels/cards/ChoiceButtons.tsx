import { Check } from 'lucide-react'
import { cn } from '@/lib/utils'

/**
 * SHARED CHOICE BUTTON LIST
 * =========================
 * The button-list visual — used by `ChoicesCard` (an authored
 * `roshera:choices` fence) and `DetectedChoicesCard` (an "Option A: …"
 * enumeration the agent wrote as prose but did not fence) — so both paths
 * look identical to the reader; only the provenance chip in each card's
 * header tells them apart.
 */
export interface ChoiceButtonOption {
  /** Sent verbatim as the next turn when this option is clicked. Also the
   *  React key — callers must keep option values distinct. */
  value: string
  /** Button text — may already be truncated by the caller. */
  label: string
  /** Secondary text beneath the label. */
  detail?: string
  /** Full untruncated text for the hover tooltip, when `label` was
   *  shortened for display. */
  title?: string
}

export function ChoiceButtons({
  options,
  selected,
  clickable,
  onSelect,
}: {
  options: ChoiceButtonOption[]
  selected?: string
  clickable: boolean
  onSelect: (value: string) => void
}) {
  return (
    <div className="mt-1.5 flex flex-col gap-1">
      {options.map((opt) => {
        const chosen = selected === opt.value
        return (
          <button
            key={opt.value}
            type="button"
            title={opt.title}
            disabled={!clickable}
            onClick={(e) => {
              // The committed line renders inside a "click to edit" button
              // (BlackboardLine.tsx) — without this, clicking an option
              // would bubble up and drop the line into raw-text edit mode
              // instead of sending the choice.
              e.stopPropagation()
              onSelect(opt.value)
            }}
            className={cn(
              'flex flex-col items-start gap-0.5 rounded-md border px-2.5 py-1.5 text-left text-[11px] leading-snug transition-colors',
              'disabled:cursor-not-allowed disabled:hover:border-border/40 disabled:hover:bg-transparent',
              chosen && 'border-emerald-500/60 bg-emerald-500/10 text-foreground',
              !chosen && !clickable && 'border-border/40 text-muted-foreground/50 opacity-60',
              !chosen &&
                clickable &&
                'border-border/70 text-foreground/90 hover:border-primary/50 hover:bg-accent/40 cursor-pointer',
            )}
          >
            <span className="flex items-center gap-1.5 font-medium">
              {chosen && <Check size={11} className="shrink-0 text-emerald-600 dark:text-emerald-400" />}
              <span className="min-w-0 truncate">{opt.label}</span>
            </span>
            {opt.detail && (
              <span className="text-[10px] text-muted-foreground/80">{opt.detail}</span>
            )}
          </button>
        )
      })}
    </div>
  )
}
