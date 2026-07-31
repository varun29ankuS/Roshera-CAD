import { cn } from '@/lib/utils'

/**
 * SRI YANTRA — the mark for Roshera's agent surface
 * ==================================================
 * Interlocking upward and downward triangles around a bindu. Chosen because
 * it is a *geometric construction* rather than a pictogram — the same thing
 * the kernel behind it does.
 *
 * Deliberately reduced: two triangles each way instead of the full nine, so
 * it stays legible at the 22px the tool rail draws at. Line weights match
 * the rail's lucide icons (strokeWidth 1.5, currentColor) so it reads as one
 * of them, not as a logo dropped in.
 *
 * The yantra itself does not rotate — it is axial by construction, and
 * spinning it would be wrong. Only the bindu breathes, which is enough to
 * say "live" without pulling the eye off the model. Motion is off entirely
 * under `prefers-reduced-motion`.
 */

/** Upward triangle: apex at top. */
function up(apexY: number, baseY: number, halfWidth: number): string {
  return `M12 ${apexY} L${12 + halfWidth} ${baseY} L${12 - halfWidth} ${baseY} Z`
}

/** Downward triangle: apex at bottom. */
function down(baseY: number, apexY: number, halfWidth: number): string {
  return `M${12 - halfWidth} ${baseY} L${12 + halfWidth} ${baseY} L12 ${apexY} Z`
}

const TRIANGLES = [
  down(6.2, 19.4, 8.2),
  up(4.6, 17.8, 8.2),
  down(9.0, 17.6, 5.4),
  up(7.4, 16.0, 5.4),
]

interface ProviderMandalaProps {
  /** A provider is configured and the backend reports it serving. */
  active?: boolean
  size?: number
  className?: string
}

export function ProviderMandala({ active = false, size = 22, className }: ProviderMandalaProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      role="presentation"
      className={cn('roshera-yantra', className)}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinejoin="round"
    >
      <style>{`
        .roshera-yantra .yantra-bindu { transform-origin: 12px 12px; animation: roshera-bindu 3.6s ease-in-out infinite; }
        @keyframes roshera-bindu { 0%, 100% { opacity: 0.45; } 50% { opacity: 1; } }
        @media (prefers-reduced-motion: reduce) { .roshera-yantra .yantra-bindu { animation: none; } }
      `}</style>

      <circle cx="12" cy="12" r="10.5" opacity="0.55" />
      {TRIANGLES.map((d, i) => (
        <path key={i} d={d} />
      ))}
      <circle
        className="yantra-bindu"
        cx="12"
        cy="12"
        r={active ? 1.5 : 1.1}
        fill="currentColor"
        stroke="none"
      />
    </svg>
  )
}
