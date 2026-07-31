/**
 * GD&T SYMBOL TABLE — comprehensive, verified, dependency-free.
 * =============================================================
 * The single source of truth for GD&T notation in the frontend. This module
 * deliberately imports NOTHING so it can be lifted verbatim into
 * `roshera-mcp/src/tools/gdt.ts` (whose `glyphFor` today covers only four
 * characteristics) once that package is safe to touch — tool output and UI
 * must agree glyph-for-glyph.
 *
 * # Code-point provenance
 * Every code point below was verified against the Unicode chart nameslists
 * (unicode.org/charts/nameslist, blocks U+2200, U+2300, U+2460, U+2190,
 * U+25A0) on 2026-07-31 — names are quoted verbatim from the charts, not
 * recalled. Where a characteristic has NO dedicated Unicode character
 * (circularity, circular runout, statistical tolerance, spherical diameter)
 * the fallback is deliberate and documented — a lookalike glyph that silently
 * means something else is worse than a spelled-out word.
 *
 * Perpendicularity note: Unicode's semantic character is U+27C2 PERPENDICULAR,
 * but the MCP layer (`gdt.ts::glyphFor`) already emits U+22A5 UP TACK (the
 * chart cross-references it as "→ 27C2 ⟂ perpendicular"). We match the MCP
 * layer so the two surfaces stay byte-identical; both render as ⊥.
 *
 * # Font coverage
 * The app's stack is 'Geist Variable', ui-sans-serif, system-ui — Geist does
 * not cover the Miscellaneous Technical block, so the browser falls back
 * per-glyph. All 25 code points in this file were checked present in
 * Segoe UI Symbol (the Windows fallback) via GlyphTypeface on 2026-07-31.
 * The UI additionally pins symbol spans to `.gdt-glyph` (Segoe UI Symbol
 * first) so a future Geist update cannot shift the notation's appearance.
 *
 * # ★ The certification boundary
 * Only flatness ⏥, perpendicularity ⊥, parallelism ∥ and position ⌖ are
 * KERNEL-CERTIFIED, evaluated at RFS, and the kernel schema
 * (`gdt_fcf` in roshera-mcp, `/api/agent/parts/{id}/fcf`) carries NO
 * material-condition modifier. Every other characteristic — and ANY frame
 * carrying a modifier — is design intent the kernel has not verified.
 * Rendering must keep that distinction visible: notation being comprehensive
 * must never make certification look comprehensive.
 */

export type GdtGroup = 'form' | 'profile' | 'orientation' | 'location' | 'runout'

/** ASME Y14.5-2018 status of a characteristic. Y14.5-2018 REMOVED
 *  concentricity and symmetry (use position, runout, or profile instead);
 *  ISO GPS (ISO 1101) retains coaxiality. Dialects must not be mixed
 *  silently — the governing standard is stated once and adhered to. */
export type AsmeStatus = 'current' | 'removed-2018'

export interface GdtCharacteristicSymbol {
  /** Canonical lowercase id — matches the kernel/MCP `characteristic` strings
   *  where they exist (`flatness`, `perpendicularity`, `parallelism`,
   *  `position`). */
  id: string
  /** Alternate ids accepted on lookup (snake_case wire variants). */
  aliases: readonly string[]
  glyph: string
  /** `U+XXXX`, verified against the Unicode chart nameslist. */
  codePoint: string
  /** Verbatim Unicode character name from the chart. */
  unicodeName: string
  /** Human name as the standards use it. */
  name: string
  group: GdtGroup
  /** TRUE only for the four kernel-certified characteristics (RFS, no
   *  modifier). Everything else is design intent, uncertified. */
  certified: boolean
  asme: AsmeStatus
  /** ISO 1101 name when it differs from the ASME name. */
  isoName?: string
  /** Set when the glyph is a drafting CONVENTION or deliberate fallback
   *  rather than a dedicated GD&T character — explains why this glyph. */
  conventional?: string
}

/**
 * The full characteristic set, grouped as ASME Y14.5 / ISO 1101 group them.
 * Order: form → profile → orientation → location → runout.
 */
export const GDT_CHARACTERISTICS: readonly GdtCharacteristicSymbol[] = [
  // ── Form ────────────────────────────────────────────────────────────
  {
    id: 'straightness',
    aliases: [],
    glyph: '⏤', // ⏤
    codePoint: 'U+23E4',
    unicodeName: 'Straightness',
    name: 'Straightness',
    group: 'form',
    certified: false,
    asme: 'current',
  },
  {
    id: 'flatness',
    aliases: [],
    glyph: '⏥', // ⏥
    codePoint: 'U+23E5',
    unicodeName: 'Flatness',
    name: 'Flatness',
    group: 'form',
    certified: true,
    asme: 'current',
  },
  {
    id: 'circularity',
    aliases: ['roundness'],
    glyph: '○', // ○
    codePoint: 'U+25CB',
    unicodeName: 'White Circle',
    name: 'Circularity',
    group: 'form',
    certified: false,
    asme: 'current',
    conventional:
      'No dedicated Unicode character; WHITE CIRCLE is the accepted drafting rendering.',
  },
  {
    id: 'cylindricity',
    aliases: [],
    glyph: '⌭', // ⌭
    codePoint: 'U+232D',
    unicodeName: 'Cylindricity',
    name: 'Cylindricity',
    group: 'form',
    certified: false,
    asme: 'current',
  },
  // ── Profile ─────────────────────────────────────────────────────────
  {
    id: 'profile_of_a_line',
    aliases: ['line_profile', 'profile_line'],
    glyph: '⌒', // ⌒
    codePoint: 'U+2312',
    unicodeName: 'Arc',
    name: 'Profile of a line',
    group: 'profile',
    certified: false,
    asme: 'current',
  },
  {
    id: 'profile_of_a_surface',
    aliases: ['surface_profile', 'profile_surface'],
    glyph: '⌓', // ⌓
    codePoint: 'U+2313',
    unicodeName: 'Segment',
    name: 'Profile of a surface',
    group: 'profile',
    certified: false,
    asme: 'current',
  },
  // ── Orientation ─────────────────────────────────────────────────────
  {
    id: 'angularity',
    aliases: [],
    glyph: '∠', // ∠
    codePoint: 'U+2220',
    unicodeName: 'Angle',
    name: 'Angularity',
    group: 'orientation',
    certified: false,
    asme: 'current',
  },
  {
    id: 'perpendicularity',
    aliases: ['squareness'],
    glyph: '⊥', // ⊥ — matches gdt.ts; chart: "→ 27C2 ⟂ perpendicular"
    codePoint: 'U+22A5',
    unicodeName: 'Up Tack',
    name: 'Perpendicularity',
    group: 'orientation',
    certified: true,
    asme: 'current',
    conventional:
      'U+22A5 UP TACK matches the MCP layer; the semantic character U+27C2 PERPENDICULAR renders identically.',
  },
  {
    id: 'parallelism',
    aliases: [],
    glyph: '∥', // ∥
    codePoint: 'U+2225',
    unicodeName: 'Parallel To',
    name: 'Parallelism',
    group: 'orientation',
    certified: true,
    asme: 'current',
  },
  // ── Location ────────────────────────────────────────────────────────
  {
    id: 'position',
    aliases: ['true_position'],
    glyph: '⌖', // ⌖
    codePoint: 'U+2316',
    unicodeName: 'Position Indicator',
    name: 'Position',
    group: 'location',
    certified: true,
    asme: 'current',
  },
  {
    id: 'concentricity',
    aliases: ['coaxiality'],
    glyph: '◎', // ◎
    codePoint: 'U+25CE',
    unicodeName: 'Bullseye',
    name: 'Concentricity',
    group: 'location',
    certified: false,
    asme: 'removed-2018',
    isoName: 'Coaxiality',
    conventional:
      'No dedicated Unicode character; BULLSEYE is the accepted drafting rendering.',
  },
  {
    id: 'symmetry',
    aliases: [],
    glyph: '⌯', // ⌯
    codePoint: 'U+232F',
    unicodeName: 'Symmetry',
    name: 'Symmetry',
    group: 'location',
    certified: false,
    asme: 'removed-2018',
  },
  // ── Runout ──────────────────────────────────────────────────────────
  {
    id: 'circular_runout',
    aliases: ['runout'],
    glyph: '↗', // ↗
    codePoint: 'U+2197',
    unicodeName: 'North East Arrow',
    name: 'Circular runout',
    group: 'runout',
    certified: false,
    asme: 'current',
    conventional:
      'Unicode has TOTAL RUNOUT (U+2330) but no circular-runout character; the NE arrow is the accepted fallback.',
  },
  {
    id: 'total_runout',
    aliases: [],
    glyph: '⌰', // ⌰
    codePoint: 'U+2330',
    unicodeName: 'Total Runout',
    name: 'Total runout',
    group: 'runout',
    certified: false,
    asme: 'current',
  },
] as const

/** Frame modifiers and related notation. `glyph: null` means there is NO
 *  reliable single Unicode character — `fallback` is the deliberate textual
 *  rendering (never a lookalike). NONE of these are accepted by the kernel
 *  schema: a frame carrying any modifier is uncertified by definition
 *  (kernel evaluation is RFS-only today). */
export interface GdtModifierSymbol {
  id: string
  glyph: string | null
  codePoint: string | null
  unicodeName: string | null
  name: string
  meaning: string
  /** Deliberate textual fallback when `glyph` is null or composed. */
  fallback?: string
  /** Modifier only meaningful in one dialect. */
  dialect?: 'iso-only' | 'asme-only'
}

export const GDT_MODIFIERS: readonly GdtModifierSymbol[] = [
  {
    id: 'diameter',
    glyph: '⌀', // ⌀
    codePoint: 'U+2300',
    unicodeName: 'Diameter Sign',
    name: 'Diameter',
    meaning: 'Cylindrical tolerance zone / diametral dimension.',
  },
  {
    id: 'spherical_diameter',
    glyph: null,
    codePoint: null,
    unicodeName: null,
    name: 'Spherical diameter',
    meaning: 'Spherical tolerance zone / spherical dimension.',
    fallback: 'S⌀', // S⌀ — composed; no single Unicode character exists
  },
  {
    id: 'mmc',
    glyph: 'Ⓜ', // Ⓜ
    codePoint: 'U+24C2',
    unicodeName: 'Circled Latin Capital Letter M',
    name: 'Maximum material condition',
    meaning: 'Tolerance applies at MMC; bonus tolerance as the feature departs.',
  },
  {
    id: 'lmc',
    glyph: 'Ⓛ', // Ⓛ
    codePoint: 'U+24C1',
    unicodeName: 'Circled Latin Capital Letter L',
    name: 'Least material condition',
    meaning: 'Tolerance applies at LMC.',
  },
  {
    id: 'projected',
    glyph: 'Ⓟ', // Ⓟ
    codePoint: 'U+24C5',
    unicodeName: 'Circled Latin Capital Letter P',
    name: 'Projected tolerance zone',
    meaning: 'Zone projected beyond the feature (e.g. tapped-hole fastener axis).',
  },
  {
    id: 'free_state',
    glyph: 'Ⓕ', // Ⓕ
    codePoint: 'U+24BB',
    unicodeName: 'Circled Latin Capital Letter F',
    name: 'Free state',
    meaning: 'Applies with the part unrestrained (non-rigid parts).',
  },
  {
    id: 'tangent_plane',
    glyph: 'Ⓣ', // Ⓣ
    codePoint: 'U+24C9',
    unicodeName: 'Circled Latin Capital Letter T',
    name: 'Tangent plane',
    meaning: 'Control applies to the tangent plane of the toleranced surface.',
  },
  {
    id: 'unequally_disposed',
    glyph: 'Ⓤ', // Ⓤ
    codePoint: 'U+24CA',
    unicodeName: 'Circled Latin Capital Letter U',
    name: 'Unequally disposed profile',
    meaning: 'Profile zone biased to one side of true profile.',
  },
  {
    id: 'continuous_feature',
    glyph: 'Ⓒ', // Ⓒ
    codePoint: 'U+24B8',
    unicodeName: 'Circled Latin Capital Letter C',
    name: 'Continuous feature',
    meaning: 'Interrupted features treated as one continuous feature.',
  },
  {
    id: 'independency',
    glyph: 'Ⓘ', // Ⓘ
    codePoint: 'U+24BE',
    unicodeName: 'Circled Latin Capital Letter I',
    name: 'Independency',
    meaning: 'Size does not control form (ISO 8015 explicit independency).',
    dialect: 'iso-only',
  },
  {
    id: 'envelope',
    glyph: 'Ⓔ', // Ⓔ
    codePoint: 'U+24BA',
    unicodeName: 'Circled Latin Capital Letter E',
    name: 'Envelope requirement',
    meaning: 'Size controls form (ISO opt-in to the ASME Rule #1 default).',
    dialect: 'iso-only',
  },
  {
    id: 'statistical',
    glyph: null,
    codePoint: null,
    unicodeName: null,
    name: 'Statistical tolerance',
    meaning: 'Tolerance based on statistical process control.',
    // The standard symbol is "ST" in a hexagon; Unicode has no such
    // character. Angle brackets are the deliberate fallback — NOT a
    // lookalike hexagon glyph from another block.
    fallback: '⟨ST⟩', // ⟨ST⟩
  },
] as const

/** Plain-letter notations (no symbol involved — these ARE the notation). */
export const GDT_TEXT_NOTATIONS = [
  { id: 'radius', notation: 'R', name: 'Radius' },
  { id: 'controlled_radius', notation: 'CR', name: 'Controlled radius' },
  { id: 'spherical_radius', notation: 'SR', name: 'Spherical radius' },
] as const

/**
 * Constructs with NO character representation at all — they are RENDERINGS
 * (a box, a boxed letter on a leader) that the UI layer draws:
 *   - basic dimension  → the value in a rectangular box (KaTeX `\boxed{}` /
 *     a bordered span);
 *   - datum feature symbol → the datum letter in a box, attached by a
 *     filled/open triangle;
 *   - datum target → a circle divided by a horizontal line.
 * Documented here so nobody "finds" a lookalike code point for them later.
 */
export const GDT_RENDERED_CONSTRUCTS = ['basic_dimension', 'datum_feature', 'datum_target'] as const

/** ★ The four kernel-certified characteristics — RFS, no modifier. The
 *  kernel schema cannot even express a material-condition modifier, so ANY
 *  modifier on a frame makes it design intent, not a certified verdict. */
export const CERTIFIED_CHARACTERISTIC_IDS: readonly string[] = [
  'flatness',
  'perpendicularity',
  'parallelism',
  'position',
] as const

const characteristicIndex: ReadonlyMap<string, GdtCharacteristicSymbol> = (() => {
  const m = new Map<string, GdtCharacteristicSymbol>()
  for (const c of GDT_CHARACTERISTICS) {
    m.set(c.id, c)
    for (const a of c.aliases) m.set(a, c)
  }
  return m
})()

/** Look up a characteristic by id or alias (case-insensitive). */
export function characteristicSymbol(id: string): GdtCharacteristicSymbol | null {
  return characteristicIndex.get(id.trim().toLowerCase()) ?? null
}

const modifierIndex: ReadonlyMap<string, GdtModifierSymbol> = (() => {
  const m = new Map<string, GdtModifierSymbol>()
  for (const mod of GDT_MODIFIERS) m.set(mod.id, mod)
  return m
})()

/** Look up a modifier by id (case-insensitive). */
export function modifierSymbol(id: string): GdtModifierSymbol | null {
  return modifierIndex.get(id.trim().toLowerCase()) ?? null
}

/** The glyph (or deliberate fallback) for a modifier id; null when unknown. */
export function modifierGlyph(id: string): string | null {
  const m = modifierSymbol(id)
  if (m === null) return null
  return m.glyph ?? m.fallback ?? null
}

/**
 * Whether a frame is CERTIFIABLE by the kernel: one of the four certified
 * characteristics AND no material-condition/zone modifier beyond ⌀ (the
 * diameter sign describes the zone shape the kernel already evaluates for
 * position; Ⓜ/Ⓛ/Ⓟ/… change the *meaning* of the tolerance and are outside
 * the schema). A certifiable frame still needs an actual kernel verdict to
 * be *certified* — certifiable-but-unevaluated is still design intent.
 */
export function isKernelCertifiable(characteristicId: string, modifierId?: string | null): boolean {
  const c = characteristicSymbol(characteristicId)
  if (c === null || !c.certified) return false
  if (modifierId == null) return true
  const id = modifierId.trim().toLowerCase()
  return id === '' || id === 'diameter'
}

/**
 * Characteristic → display glyph. Same contract as `gdt.ts::glyphFor`
 * (unknown ids fall through to the raw string so nothing is ever silently
 * dropped), now covering the full form/profile/orientation/location/runout
 * set instead of only the certified four.
 */
export function glyphFor(characteristic: string): string {
  return characteristicSymbol(characteristic)?.glyph ?? characteristic
}
