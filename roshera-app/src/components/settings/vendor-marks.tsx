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
 *   - Mistral AI: the "Mistral AI" mark from simple-icons.org
 *     (`icons/mistralai.svg`), sourced there from chat.mistral.ai.
 *   - DeepSeek: the "DeepSeek" mark from simple-icons.org
 *     (`icons/deepseek.svg`, fetched via jsdelivr), path data verbatim.
 *     `deepseek` is an allowlisted wired provider (api_key, via goose's
 *     bundled `custom_deepseek` declarative definition) and had been
 *     falling through to the initials fallback for want of this entry.
 *   2026-08-01 (Varun): "you didn't use logo for the providers" — the
 *   allowlisted vendors were showing initials fallbacks. Went looking for
 *   real marks (Sarvam AI, Moonshot Kimi and Zhipu GLM have since been
 *   removed from Roshera's providers, Varun 2026-11-13):
 *   - xAI (`XaiMark`): none of simple-icons/jsdelivr, and x.ai itself
 *     sits behind a Cloudflare challenge page (no site-root favicon/logo
 *     SVG reachable). Wikimedia Commons has three candidate uploads;
 *     `File:XAI-Logo.svg` is the one sourced directly from x.ai, CC0
 *     ("simple geometric shapes... do not meet the threshold for
 *     copyright protection") — the other two are unlabelled fan uploads
 *     and were not used. Its four polygons (an angular "X") are inlined
 *     verbatim below.
 *   - Baseten: still no genuine published SVG mark found anywhere —
 *     re-checked simple-icons/jsdelivr (no `baseten.svg`), Wikimedia
 *     Commons (no hits), and `baseten.co/favicon.svg` (404; the site
 *     ships only a raster `/favicon.ico`). Stays on the initials fallback
 *     below, on purpose.
 *   A wrong or unverified logo next to a real company name is a factual
 *   error shipped in the UI; two honest initials is not. See `VendorMark`
 *   below for the fallback itself.
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
 *   - Mistral AI: `#FA520F`, simple-icons' recorded hex — a single
 *     cross-theme accent like Anthropic's, not a black/white mark.
 *   - DeepSeek: `#5786FE`, simple-icons' recorded hex — a colour accent
 *     like Anthropic's and Mistral's, so no light/dark swap.
 *   - xAI: the source file specifies no fill at all (a bare shape, unlike
 *     Anthropic's/Mistral's explicit hex) — treated as a monochrome mark,
 *     same black-on-light/white-on-dark treatment as OpenAI,
 *     reactive to `useThemeStore`.
 * The shape is never stretched, rotated, or redrawn, only coloured per the
 * vendor's own kit — nominative use (identifying the service), never a
 * partnership or endorsement claim. Callers must still pair a mark with
 * the real wired/unwired status from the allowlist; a logo must never be
 * the only signal that a mode is available.
 */

const MARK_CLASS = 'h-3.5 w-3.5 shrink-0'
const ANTHROPIC_CLAY = '#D97757'
const MISTRAL_ORANGE = '#FA520F'
const DEEPSEEK_BLUE = '#5786FE'

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

export function DeepSeekMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={cn(MARK_CLASS, className)}
      fill={DEEPSEEK_BLUE}
      role="img"
      aria-label="DeepSeek"
    >
      <path d="M23.748 4.651c-.254-.124-.364.113-.512.233-.051.04-.094.09-.137.137-.372.397-.806.657-1.373.626-.829-.046-1.537.214-2.163.848-.133-.782-.575-1.248-1.247-1.548-.352-.155-.708-.311-.955-.65-.172-.24-.219-.509-.305-.774-.055-.16-.11-.323-.293-.35-.2-.031-.278.136-.356.276-.313.572-.434 1.202-.422 1.84.027 1.436.633 2.58 1.838 3.393.137.094.172.187.129.323-.082.28-.18.553-.266.833-.055.179-.137.218-.328.14a5.5 5.5 0 0 1-1.737-1.179c-.857-.828-1.631-1.743-2.597-2.46a12 12 0 0 0-.689-.47c-.985-.957.13-1.743.387-1.836.27-.098.094-.433-.778-.428-.872.003-1.67.295-2.687.685a3 3 0 0 1-.465.136 9.6 9.6 0 0 0-2.883-.101c-1.885.21-3.39 1.1-4.497 2.622C.082 8.776-.231 10.854.152 13.02c.403 2.284 1.568 4.175 3.36 5.653 1.857 1.533 3.997 2.284 6.438 2.14 1.482-.085 3.132-.284 4.994-1.86.47.234.962.328 1.78.398.629.058 1.235-.031 1.705-.129.735-.155.684-.836.418-.961-2.155-1.004-1.682-.595-2.112-.926 1.095-1.295 2.768-3.598 3.284-6.733.05-.346.115-.834.108-1.114-.004-.171.035-.238.23-.257a4.2 4.2 0 0 0 1.545-.475c1.397-.763 1.96-2.016 2.093-3.517.02-.23-.004-.467-.247-.588M11.58 18.168c-2.088-1.642-3.101-2.183-3.52-2.16-.39.024-.32.472-.234.763.09.288.207.487.371.74.114.167.192.416-.113.603-.673.416-1.842-.14-1.897-.168-1.361-.801-2.5-1.86-3.301-3.306-.775-1.393-1.225-2.888-1.299-4.482-.02-.385.094-.522.477-.592a4.7 4.7 0 0 1 1.53-.038c2.131.311 3.946 1.264 5.467 2.774.868.86 1.525 1.887 2.202 2.89.72 1.066 1.494 2.082 2.48 2.915.348.291.626.513.892.677-.802.09-2.14.109-3.055-.615zm1.001-6.44a.306.306 0 0 1 .415-.287.3.3 0 0 1 .113.074.3.3 0 0 1 .086.214c0 .17-.136.307-.308.307a.303.303 0 0 1-.306-.307m3.11 1.596c-.2.081-.4.151-.591.16a1.25 1.25 0 0 1-.798-.254c-.274-.23-.47-.358-.551-.758a1.7 1.7 0 0 1 .015-.588c.07-.327-.007-.537-.238-.727-.188-.156-.426-.199-.689-.199a.6.6 0 0 1-.254-.078.253.253 0 0 1-.114-.358 1 1 0 0 1 .192-.21c.356-.202.767-.136 1.146.016.352.144.618.408 1.001.782.392.451.462.576.685.915.176.264.336.536.446.848.066.194-.02.353-.25.45" />
    </svg>
  )
}

export function MistralMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      className={cn(MARK_CLASS, className)}
      fill={MISTRAL_ORANGE}
      role="img"
      aria-label="Mistral AI"
    >
      <path d="M17.143 3.429v3.428h-3.429v3.429h-3.428V6.857H6.857V3.43H3.43v13.714H0v3.428h10.286v-3.428H6.857v-3.429h3.429v3.429h3.429v-3.429h3.428v3.429h-3.428v3.428H24v-3.428h-3.43V3.429z" />
    </svg>
  )
}

export function XaiMark({ className }: { className?: string }) {
  // xAI publishes no brand-colour spec (unlike Anthropic/Mistral) — the
  // mark itself is drawn with no fill override, i.e. it's a monochrome
  // shape like OpenAI's, so it gets the identical contrast-driven
  // black/white swap rather than an invented accent colour.
  const isDark = useThemeStore((s) => s.theme === 'dark')
  return (
    <svg
      viewBox="0 0 466.04 516.93"
      className={cn(MARK_CLASS, className)}
      fill={isDark ? '#FFFFFF' : '#000000'}
      role="img"
      aria-label="xAI"
    >
      <polygon points="0.12 182.71 234.14 516.92 338.15 516.92 104.13 182.71 0.12 182.71" />
      <polygon points="0 516.92 104.08 516.92 156.08 442.67 104.04 368.34 0 516.92" />
      <polygon points="466.04 0 361.96 0 182.1 256.86 234.15 331.18 466.04 0" />
      <polygon points="380.78 516.92 466.04 516.92 466.04 37.16 380.78 158.92 380.78 516.92" />
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
  if (providerId === 'mistral') return <MistralMark className={className} />
  if (providerId === 'xai') return <XaiMark className={className} />
  if (providerId === 'deepseek') return <DeepSeekMark className={className} />
  // baseten: no genuine published SVG mark found anywhere (see the
  // file-header comment) — initials fallback below, on purpose.
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
