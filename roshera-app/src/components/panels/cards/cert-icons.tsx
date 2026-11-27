import type { ReactNode } from 'react'

/**
 * Line icons for certificate rows -- and only for them.
 *
 * The app's icon set is rendered colour artwork (components/ui/icons.tsx),
 * right for tools and wrong for a column of checks: at 16 px beside a
 * verdict it reads as a row of coloured dots that compete with the tick. A
 * certificate row's icon only marks the row, so these are quiet: one colour
 * (`currentColor`, muted by the row), a 1.5 px stroke on a 16 px grid, no
 * fill. Each is a plain picture of the claim, and the claim's full name is
 * always printed beside it.
 */

interface CertIconProps {
  size?: number | string
  className?: string
}

function icon(name: string, paths: ReactNode) {
  function CertIcon({ size = 16, className }: CertIconProps) {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.5}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className={className}
      >
        {paths}
      </svg>
    )
  }
  CertIcon.displayName = `CertIcon(${name})`
  return CertIcon
}

/** B-Rep valid: a solid's boundary -- a cube in outline. */
export const BrepIcon = icon(
  'brep',
  <>
    <path d="M8 1.75 13.5 4.75v6.5L8 14.25 2.5 11.25v-6.5z" />
    <path d="M2.5 4.75 8 7.75l5.5-3M8 7.75v6.5" />
  </>,
)

/** Watertight: a drop. */
export const WatertightIcon = icon('watertight', <path d="M8 1.75c2.4 3 4 5.2 4 7.5a4 4 0 0 1-8 0c0-2.3 1.6-4.5 4-7.5z" />)

/** Manifold: two faces meeting along one shared edge. */
export const ManifoldIcon = icon(
  'manifold',
  <>
    <path d="M8 3 2.5 5v8L8 11z" />
    <path d="M8 3l5.5 2v8L8 11" />
  </>,
)

/** Shells face outward: a body with normals pointing out of it. */
export const OutwardIcon = icon(
  'outward',
  <>
    <rect x="5.25" y="5.25" width="5.5" height="5.5" />
    <path d="M8 5.25V1.75M8 10.75v3.5M5.25 8H1.75M10.75 8h3.5" />
  </>,
)

/** No self-intersection: two surfaces that pass without crossing. */
export const NoIntersectionIcon = icon(
  'no-intersection',
  <>
    <path d="M2 4.5c3.5 0 4.5 2.2 6 2.2s2.5-2.2 6-2.2" />
    <path d="M2 11.5c3.5 0 4.5-2.2 6-2.2s2.5 2.2 6 2.2" />
  </>,
)

/** Construction consistent: the recipe -- steps joined in order. */
export const ConstructionIcon = icon(
  'construction',
  <>
    <circle cx="3.5" cy="3.5" r="1.75" />
    <circle cx="12.5" cy="8" r="1.75" />
    <circle cx="3.5" cy="12.5" r="1.75" />
    <path d="M5.25 3.5H9a1.75 1.75 0 0 1 1.75 1.75v.5M10.75 9.75v.5A1.75 1.75 0 0 1 9 12.5H5.25" />
  </>,
)

/** Labels consistent: a tag. */
export const LabelsIcon = icon(
  'labels',
  <>
    <path d="M2 2h5.5l6.5 6.5-5.5 5.5L2 7.5z" />
    <circle cx="5" cy="5" r="0.6" />
  </>,
)

/** Tessellation clean: a triangle mesh. */
export const TessellationIcon = icon(
  'tessellation',
  <>
    <path d="M2 13.5 8 2.5l6 11z" />
    <path d="M5 8h6M5 8l3 5.5L11 8" />
  </>,
)

/** Mesh quality: a gauge. */
export const MeshQualityIcon = icon(
  'mesh-quality',
  <>
    <path d="M2 11.5a6 6 0 0 1 12 0" />
    <path d="M8 11.5 10.75 7M2 11.5h1.5M12.5 11.5H14M8 5.5V7" />
  </>,
)

/** Dual-view consistent: two viewpoints on one thing. */
export const DualViewIcon = icon(
  'dual-view',
  <>
    <path d="M1.5 8c1.6-2.6 3.8-4 6.5-4s4.9 1.4 6.5 4c-1.6 2.6-3.8 4-6.5 4s-4.9-1.4-6.5-4z" />
    <circle cx="6.25" cy="8" r="1.25" />
    <circle cx="9.75" cy="8" r="1.25" />
  </>,
)

/** Sheet sound: what is printed is true -- a checked shield. */
export const SheetSoundIcon = icon(
  'sheet-sound',
  <>
    <path d="M8 1.75 13 3.5v4.25c0 3-2 5.25-5 6.5-3-1.25-5-3.5-5-6.5V3.5z" />
    <path d="m5.75 7.75 1.75 1.75 3-3.25" />
  </>,
)

/** Sheet complete: every dimension accounted for -- a ticked list. */
export const SheetCompleteIcon = icon(
  'sheet-complete',
  <>
    <path d="M7.5 4h6M7.5 8h6M7.5 12h6" />
    <path d="m2 4 1 1 1.75-2M2 8l1 1 1.75-2M2 12l1 1 1.75-2" />
  </>,
)

/** Sheet layout: views set out inside the frame. */
export const SheetLayoutIcon = icon(
  'sheet-layout',
  <>
    <rect x="1.75" y="2.75" width="12.5" height="10.5" />
    <path d="M4.25 5.25h3v2.5h-3zM9.5 5.25h2.25v2.5H9.5zM4.25 9.75h3v1.25h-3z" />
  </>,
)

/** Sheet paper and scale: a sheet with its corner turned. */
export const SheetPaperIcon = icon(
  'sheet-paper',
  <>
    <path d="M3.5 1.75h6l3 3v9.5h-9z" />
    <path d="M9.5 1.75v3h3" />
  </>,
)
