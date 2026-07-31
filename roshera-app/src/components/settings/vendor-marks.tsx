import { cn } from '@/lib/utils'
import { useThemeStore } from '@/stores/theme-store'

/**
 * VENDOR MARKS — inline SVG only
 * ================================
 * The CSP and Roshera's offline-first posture forbid remote asset fetches
 * (no CDN, no `<img src=http…>`), so these are the vendors' own published
 * marks inlined as SVG path data, not redrawn by hand:
 *   - Anthropic: the "Anthropic" mark from simple-icons.org, a
 *     community-maintained set kept in sync with each vendor's brand kit.
 *   - OpenAI: OpenAI's "blossom" symbol (in use since Feb 2025), path data
 *     from Wikimedia Commons' File:OpenAI_logo_2025_(symbol).svg, itself
 *     sourced from openai.com/brand.
 *
 * Rendered in each vendor's OWN published colour (Varun 2026-07-31:
 * recognition is the point — brand colour is what the eye keys on), never
 * a hue the brand doesn't use:
 *   - Anthropic: `#D97757` ("clay", the brand's primary accent per
 *     anthropics/skills' own brand-guidelines skill) — the same value on
 *     both light and dark surfaces; it's Anthropic's own cross-theme
 *     accent, not a tint invented here.
 *   - OpenAI: black on light, white on dark — OpenAI's own guidance is to
 *     pick whichever of the two maximizes contrast against the surface,
 *     never a custom colour. Reactive to the live theme (`useThemeStore`),
 *     not a static default, so it never goes illegible on a theme flip.
 * The shape is never stretched, rotated, or redrawn, only coloured per the
 * vendor's own kit — nominative use (identifying the service), never a
 * partnership or endorsement claim. Callers must still pair a mark with
 * the real wired/unwired status from the allowlist; a logo must never be
 * the only signal that a mode is available.
 */

const MARK_CLASS = 'h-3.5 w-3.5 shrink-0'
const ANTHROPIC_CLAY = '#D97757'

export function AnthropicMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={cn(MARK_CLASS, className)}
      fill={ANTHROPIC_CLAY}
      role="img"
      aria-label="Anthropic"
    >
      <path d="M17.3041 3.541h-3.6718l6.696 16.918H24Zm-10.6082 0L0 20.459h3.7442l1.3693-3.5527h7.0052l1.3693 3.5528h3.7442L10.5363 3.5409Zm-.3712 10.2232 2.2914-5.9456 2.2914 5.9456Z" />
    </svg>
  )
}

export function OpenAIMark({ className }: { className?: string }) {
  // Official guidance: pick whichever of black/white maximizes contrast
  // against the surface — never a custom colour. Read live so a theme
  // toggle while the dialog is open never leaves it illegible.
  const isDark = useThemeStore((s) => s.theme === 'dark')
  return (
    <svg
      viewBox="0 0 20 20"
      className={cn(MARK_CLASS, className)}
      fill={isDark ? '#FFFFFF' : '#000000'}
      role="img"
      aria-label="OpenAI"
    >
      <path d="M11.248 18.25q-.825 0-1.568-.314a4.3 4.3 0 0 1-1.32-.874 4 4 0 0 1-1.304.214 4 4 0 0 1-2.046-.544 4.27 4.27 0 0 1-1.518-1.485 4 4 0 0 1-.56-2.095q0-.48.131-1.04A4.4 4.4 0 0 1 2.04 10.71a4.07 4.07 0 0 1 .017-3.4 4.2 4.2 0 0 1 1.056-1.418 3.8 3.8 0 0 1 1.6-.842 3.9 3.9 0 0 1 .76-1.683q.593-.759 1.451-1.188a4.04 4.04 0 0 1 1.832-.429q.825 0 1.567.313.742.314 1.32.875a4 4 0 0 1 1.304-.215q1.106 0 2.046.545a4.14 4.14 0 0 1 1.501 1.485q.578.941.578 2.095 0 .48-.132 1.04.66.61 1.023 1.419.363.792.363 1.666 0 .892-.38 1.717a4.3 4.3 0 0 1-1.072 1.435 3.8 3.8 0 0 1-1.584.825 3.8 3.8 0 0 1-.775 1.683 4.06 4.06 0 0 1-1.436 1.188 4.04 4.04 0 0 1-1.832.429m-4.076-2.062q.825 0 1.435-.347l3.103-1.782a.36.36 0 0 0 .164-.313v-1.42L7.881 14.62a.67.67 0 0 1-.726 0l-3.118-1.798a.5.5 0 0 1-.017.115v.198q0 .841.396 1.551.413.693 1.139 1.089a3.2 3.2 0 0 0 1.617.412m.165-2.69a.4.4 0 0 0 .181.05q.083 0 .165-.05l1.238-.71-3.977-2.31a.7.7 0 0 1-.363-.643v-3.58q-.825.362-1.32 1.122a2.9 2.9 0 0 0-.495 1.65q0 .809.413 1.55.412.743 1.072 1.123zm3.91 3.663q.875 0 1.585-.396a2.96 2.96 0 0 0 1.534-2.64v-3.564a.32.32 0 0 0-.165-.297l-1.254-.726v4.604a.7.7 0 0 1-.363.643l-3.119 1.799a3 3 0 0 0 1.783.577m.627-6.039V8.878L10.01 7.822 8.129 8.878v2.244l1.881 1.056zM7.057 5.859a.7.7 0 0 1 .363-.644l3.119-1.798a3 3 0 0 0-1.782-.578q-.874 0-1.584.396A2.96 2.96 0 0 0 6.05 4.324a3.07 3.07 0 0 0-.396 1.551v3.547q0 .199.165.314l1.237.726zm8.383 7.887q.825-.364 1.303-1.123.495-.758.495-1.65a3.15 3.15 0 0 0-.412-1.55q-.413-.743-1.073-1.123l-3.086-1.782q-.099-.065-.181-.049a.3.3 0 0 0-.165.05l-1.238.692 3.993 2.327a.6.6 0 0 1 .264.264.64.64 0 0 1 .1.363zm-3.317-8.382a.63.63 0 0 1 .726 0l3.135 1.831v-.297q0-.792-.396-1.501a2.86 2.86 0 0 0-1.105-1.155q-.71-.43-1.65-.43-.825 0-1.436.347L8.294 5.941a.36.36 0 0 0-.165.314v1.418z" />
    </svg>
  )
}

/** `"xAI Grok"` → `"XG"`, `"Baseten"` → `"B"` — same fallback logic a
 *  contact-list avatar uses, capped at two characters so it stays legible
 *  at the small sizes this renders at. */
function initials(displayName: string): string {
  const letters = displayName
    .split(/\s+/)
    .filter(Boolean)
    .map((word) => word[0]?.toUpperCase() ?? '')
    .join('')
  return letters.slice(0, 2) || '?'
}

/** Maps an allowlist provider id to its mark — or, for a provider not yet
 *  drawn here, its initials in a plain text badge. Never a placeholder
 *  image implying a vendor relationship Roshera hasn't verified; a text
 *  fallback makes no such claim, it just keeps the vendor identifiable. */
export function VendorMark({
  providerId,
  displayName,
  className,
}: {
  providerId: string
  /** Required for the initials fallback — every call site already has the
   *  allowlist's `display_name` on hand. */
  displayName: string
  className?: string
}) {
  if (providerId === 'anthropic') return <AnthropicMark className={className} />
  if (providerId === 'openai') return <OpenAIMark className={className} />
  return (
    <span
      className={cn(
        'flex items-center justify-center font-semibold text-muted-foreground',
        className,
      )}
      role="img"
      aria-label={displayName}
      title={displayName}
    >
      {initials(displayName)}
    </span>
  )
}
