import { Check, X } from '@/components/ui/icons'
import { PART_VERDICT_TEXT, partVerdict } from '@/lib/part-verdict'
import { cn } from '@/lib/utils'

/**
 * The mark beside a part in the tree: its certificate verdict.
 * A tick is sound, a cross is not sound, a dashed amber ring is "not
 * certified yet" -- the same tri-state the certificate rows use, so a part
 * with no certificate can never be mistaken for one that passed.
 */
export function PartVerdictMark({ sound, className }: { sound: boolean | null | undefined; className?: string }) {
  const verdict = partVerdict(sound)
  const text = PART_VERDICT_TEXT[verdict]
  return (
    <span
      role="img"
      aria-label={text}
      title={text}
      className={cn('inline-flex h-4 w-4 shrink-0 items-center justify-center', className)}
    >
      {verdict === 'sound' ? (
        <Check size={14} />
      ) : verdict === 'unsound' ? (
        <X size={14} />
      ) : (
        <span className="h-2.5 w-2.5 rounded-full border border-dashed border-caution" />
      )}
    </span>
  )
}
