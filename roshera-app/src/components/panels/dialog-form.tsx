/**
 * Shared form vocabulary for the app's dialogs.
 *
 * The dialogs were built at different times and disagreed about field
 * labels, error display, and numeric entry. This module is the single
 * source for those three things so a form field or an error line looks
 * identical in every dialog (2026-07-31 UI-pass spec §0: one type ramp,
 * one spacing scale, colour only for state).
 *
 * Error display reuses the tick/cross vocabulary from
 * `cards/card-chrome.tsx` — a red X glyph before the message — rather
 * than a tinted banner or bare red prose, so "something failed" reads
 * by shape before the sentence is parsed.
 */

import type { ReactNode } from 'react'
import { X } from 'lucide-react'
import { Input } from '@/components/ui/input'

/**
 * One failed-action line inside a dialog. Renders nothing when
 * `children` is null/empty so callers can pass their error state
 * directly. `role="alert"` so the failure is announced, not just shown.
 */
export function DialogError({ children }: { children: ReactNode }) {
  if (children === null || children === undefined || children === '') return null
  return (
    <div
      role="alert"
      className="flex items-baseline gap-1.5 text-[11px] leading-snug text-foreground/90"
    >
      <span className="inline-flex shrink-0 translate-y-px text-red-600 dark:text-red-400">
        <X size={11} />
      </span>
      <span className="select-text font-mono text-red-700 dark:text-red-300">{children}</span>
    </div>
  )
}

/** Column field: small caps mono label above the control. */
export function FormField({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground/70">
        {label}
      </span>
      {children}
    </label>
  )
}

/** Free-form decimal entry — mono, tabular, no spinner chrome. */
export function NumericInput({
  value,
  onChange,
  placeholder,
  ariaLabel,
}: {
  value: string
  onChange: (next: string) => void
  placeholder?: string
  ariaLabel?: string
}) {
  return (
    <Input
      type="text"
      inputMode="decimal"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      aria-label={ariaLabel}
      className="cad-readout text-[13px]"
    />
  )
}

/**
 * X/Y/Z triple. Numbers-as-numbers variant (the assembly dialogs hold
 * numeric state); axis letters are the labels so the row scans as a
 * coordinate, not three anonymous boxes.
 */
export function Vec3Input({
  label,
  value,
  onChange,
}: {
  label: string
  value: [number, number, number]
  onChange: (v: [number, number, number]) => void
}) {
  return (
    <FormField label={label}>
      <div className="grid grid-cols-3 gap-2">
        {(['x', 'y', 'z'] as const).map((axis, i) => (
          <label key={axis} className="flex items-center gap-1">
            <span className="w-3 font-mono text-[10px] uppercase text-muted-foreground">
              {axis}
            </span>
            <Input
              type="number"
              step="0.1"
              value={value[i]}
              onChange={(e) => {
                const n = Number(e.target.value)
                const next: [number, number, number] = [...value]
                next[i] = n
                onChange(next)
              }}
              aria-label={`${label} ${axis.toUpperCase()}`}
              className="cad-readout min-w-0 flex-1 text-[12px]"
            />
          </label>
        ))}
      </div>
    </FormField>
  )
}
