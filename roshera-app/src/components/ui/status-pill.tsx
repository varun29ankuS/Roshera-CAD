import { cva } from 'class-variance-authority';
import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import { cn } from '@/lib/utils';

/*
 * Species A — StatusPill: asserts a fact about something.
 */

const statusToneStyles = {
  neutral: 'bg-card border-border text-muted-foreground',
  positive: 'bg-positive-wash border-positive-border text-positive',
  caution: 'bg-caution-wash border-caution-border text-caution',
  negative: 'bg-negative-wash border-negative-border text-destructive',
} as const;

export type StatusTone = keyof typeof statusToneStyles;

// Severity glyphs live here rather than at call sites: a tone that forgets its
// ▲ or ✕ stops communicating danger, so the component guarantees the signal.
const defaultGlyphByTone: Record<StatusTone, ReactNode> = {
  neutral: undefined,
  positive: '✓',
  caution: '▲',
  negative: '✕',
};

const statusPillVariants = cva(
  // Inline-level so a pill can sit inside a sentence or table cell without
  // forcing a line break around itself.
  'inline-flex h-5 max-w-full items-center gap-1 rounded-full border px-2 align-middle',
  { variants: { tone: statusToneStyles }, defaultVariants: { tone: 'neutral' } },
);

type StatusPillFields = {
  tone?: StatusTone;
  glyph?: ReactNode;
  meta?: string;
  dot?: boolean;
  title?: string;
  role?: ComponentPropsWithoutRef<'span'>['role'];
  className?: string;
};

// Keyed on the literal '': a dot-only pill has no visible text, so its accessible
// name cannot be left optional — the type system enforces it, not reviewer memory.
export type StatusPillProps<Label extends string = string> = StatusPillFields &
  (Label extends '' ? { label: Label; ariaLabel: string } : { label: Label; ariaLabel?: string });

export function StatusPill<Label extends string>({
  tone,
  glyph,
  meta,
  dot,
  title,
  role,
  className,
  label,
  ariaLabel,
}: StatusPillProps<Label>) {
  const effectiveTone = tone ?? 'neutral';
  // "Healthy" reads fastest as wash + dot together, so positive opts in by
  // default while every other tone has to ask for a dot explicitly.
  const showDot = dot ?? effectiveTone === 'positive';
  const resolvedGlyph = glyph ?? defaultGlyphByTone[effectiveTone];

  return (
    <span
      role={role}
      aria-label={ariaLabel}
      title={title}
      className={cn(statusPillVariants({ tone }), className)}
    >
      {resolvedGlyph != null && (
        <span aria-hidden="true" className="shrink-0 leading-none">
          {resolvedGlyph}
        </span>
      )}
      {showDot && (
        <span aria-hidden="true" className="size-1.5 shrink-0 rounded-full bg-positive-dot" />
      )}
      {/* Space is negotiated on the label: the mono meta carries hashes/ids that
          become lies when clipped, so it never wraps or shrinks and the prose
          label absorbs any shortfall as an ellipsis instead. */}
      <span className="min-w-0 truncate text-[11px] font-medium leading-none">{label}</span>
      {meta != null && (
        <span className="shrink-0 whitespace-nowrap font-mono text-[11px] leading-none text-muted-foreground">
          {meta}
        </span>
      )}
    </span>
  );
}

/*
 * Species B — ModeChip / TabChip: choose among equals.
 */

const modeChipVariants = cva(
  'inline-flex h-[22px] items-center justify-center gap-1 rounded px-2 text-xs font-medium align-middle',
  {
    variants: {
      selected: {
        // Filled primary appears nowhere else in this file: reserving it for
        // "the chosen one" is what keeps selection legible at a glance.
        true: 'bg-primary text-primary-foreground',
        false: 'border border-border bg-transparent text-muted-foreground hover:bg-accent',
      },
    },
    defaultVariants: { selected: false },
  },
);

export type ModeChipProps = ComponentPropsWithoutRef<'button'> & {
  selected?: boolean;
};

export function ModeChip({ selected = false, className, children, ...rest }: ModeChipProps) {
  return (
    // data-selected gives tests and parent selectors a stable hook without
    // reaching into generated class internals.
    // A control that CHOOSES is a button, not a span: it must take focus, fire
    // on Enter/Space, and announce its pressed state. Rendering these as spans
    // would have made the whole segmented control unreachable by keyboard.
    <button
      type="button"
      aria-pressed={selected}
      data-selected={selected || undefined}
      className={cn(modeChipVariants({ selected }), className)}
      {...rest}
    >
      {children}
    </button>
  );
}

export type ModeChipGroupProps = ComponentPropsWithoutRef<'div'>;

export function ModeChipGroup({ className, children, ...rest }: ModeChipGroupProps) {
  return (
    <div
      role="group"
      className={cn(
        'inline-flex items-stretch divide-x divide-border overflow-hidden rounded border border-border align-middle',
        // Children surrender their own edges: the container owns the outer
        // radius and the shared dividers, so neighbours read as one control
        // instead of adjacent tiles with doubled borders.
        '[&>*]:rounded-none [&>*]:border-0',
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}

const tabChipVariants = cva(
  'relative inline-flex h-[22px] items-center justify-center gap-1 rounded border border-border px-3 text-xs font-medium align-middle',
  {
    variants: {
      selected: {
        // A tab must not borrow ModeChip's navy fill: it earns selection with a
        // card surface plus a primary underline, keeping the two species
        // visually distinct even side by side.
        true: 'bg-card text-foreground after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:bg-primary after:content-[""]',
        false: 'bg-transparent text-muted-foreground hover:bg-accent',
      },
    },
    defaultVariants: { selected: false },
  },
);

export type TabChipProps = ComponentPropsWithoutRef<'button'> & {
  selected?: boolean;
};

export function TabChip({ selected = false, className, children, ...rest }: TabChipProps) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      data-selected={selected || undefined}
      className={cn(tabChipVariants({ selected }), className)}
      {...rest}
    >
      {children}
    </button>
  );
}