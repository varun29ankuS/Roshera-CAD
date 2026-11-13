/**
 * Dimension kinds the kernel's dimension table emits
 * (`geometry-engine/src/readable/dimensions.rs`, `DimensionRecord::kind`)
 * and the viewport's filter chips over them.
 *
 * Shared by the scene store default, the `DimensionKindChips` UI, and the
 * `PartDimensions` row filter so all three stay in lock-step. Dependency
 * free so it can be unit-tested directly.
 */

/** Every kind the kernel's dimension table produces. */
export type DimensionKind =
  | 'extent'
  | 'length'
  | 'position'
  | 'diameter'
  | 'radius'
  | 'angle'
  /** A blend's radius: value = the radius, label like `R3.00`. */
  | 'fillet'
  /** An equal-leg chamfer: value = the leg, label like `0.50 × 45°`. */
  | 'chamfer'

/**
 * The chip-filterable kinds, in display order. `diameter` also covers kernel
 * `radius` rows (one Ø chip); kinds not listed here (angles,
 * interactive-measure kinds) are never filtered.
 */
export const ALL_DIMENSION_KINDS = [
  'extent',
  'diameter',
  'length',
  'position',
  'fillet',
  'chamfer',
] as const

export type DimensionChip = (typeof ALL_DIMENSION_KINDS)[number]

/** Chip labels, keyed by chip. */
export const DIMENSION_CHIP_LABELS: Record<DimensionChip, string> = {
  extent: 'Extents',
  diameter: 'Ø',
  length: 'Lengths',
  position: 'Positions',
  fillet: 'Fillets',
  chamfer: 'Chamfers',
}

/**
 * Map a row kind to its filter chip, or `null` for kinds no chip governs
 * (angles, interactive-measure kinds) — those always render. `radius` shares
 * the Ø chip: both are hole/boss size callouts.
 */
export function chipKindOf(kind: string): DimensionChip | null {
  if (
    kind === 'extent' ||
    kind === 'length' ||
    kind === 'position' ||
    kind === 'fillet' ||
    kind === 'chamfer'
  ) {
    return kind
  }
  if (kind === 'diameter' || kind === 'radius') return 'diameter'
  return null
}
