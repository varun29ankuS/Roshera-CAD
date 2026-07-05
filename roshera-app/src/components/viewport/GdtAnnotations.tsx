import { useMemo, useState } from 'react'
import { Html, Line } from '@react-three/drei'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import {
  useGdt,
  type GdtDatumWire,
  type GdtAnnotationWire,
  type DatumResolutionWire,
} from './use-gdt'

/**
 * GD&T viewport overlay — datum flags and FCF badges for every solid
 * whose GDT state the kernel currently holds.
 *
 * ## Datum flags
 * Each live datum renders a boxed letter (e.g. `[A]`) with a datum-feature
 * triangle glyph anchored at the datum face's origin (from
 * `resolution.origin`). Dangling datums (feature consumed by a later
 * operation) render in a muted/struck amber style — still listed, never
 * silently dropped. Datums with `"dangling"` resolution have no world
 * anchor; they are stacked at a fixed fallback point.
 *
 * ## FCF badges
 * Each annotation renders a bordered frame text badge (e.g. `⊥ 0.05 | A`)
 * near the datum face origin that it references. Conformance colour:
 *   - green (`in_spec`): the measured value is within the tolerance zone.
 *   - red (`out_of_spec`): the measured value exceeds the tolerance zone.
 *   - grey (`not_evaluable`): the verdict cannot be computed (dangling datum,
 *     missing basic dims, etc.).
 * Hover shows the measured vs tolerance labels and fit residual via a
 * tooltip panel — all values are displayed VERBATIM from the kernel's
 * unit-formatted `*_label` fields; zero client-side math.
 *
 * ## Fetch discipline
 * One `useGdt` hook per toggled solid, gated by `gdtVisible`. Refetch on
 * geometry change (objectOrder + vertex-count fingerprint) AND on
 * `unitEpoch` bump so label strings arrive in the new unit. Cleanup on
 * part removal/clear is handled by the hook's abort flag + the store's
 * `removeObject` action dropping the solid from scope.
 *
 * ## Session-persistence honesty
 * If the GET returns empty arrays (server restart cleared GD&T state),
 * the layer renders nothing — no placeholder chrome.
 *
 * ## No frontend math
 * Every verdict value (conformance, tolerance, measured, residual) is
 * displayed verbatim from kernel-returned fields. The layer never
 * computes a verdict or formats a measurement.
 *
 * ## Depth strategy
 * All glyph graphics (leader, triangle, datum box) render with
 * `depthTest={false}` + `depthWrite={false}` — always-on-top overlay.
 * The `<Html>` badges use `zIndexRange={[100, 0]}` matching PartDimensions.
 * No beacon dots, no ray-cast targets on geometry lines.
 */
export function GdtAnnotations() {
  const gdtVisible = useSceneStore((s) => s.gdtVisible)
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)

  if (!gdtVisible) return null

  // Collect objects that have a solidId so we can fetch GDT per-solid.
  const active: Array<{ objectId: string; solidId: number }> = []
  for (const id of objectOrder) {
    const solidId = objects.get(id)?.analyticalGeometry?.solidId
    if (solidId === undefined) continue
    active.push({ objectId: id, solidId })
  }

  if (active.length === 0) return null

  return (
    <group name="gdt-annotations">
      {active.map(({ objectId, solidId }) => (
        <ObjectGdtAnnotations
          key={objectId}
          objectId={objectId}
          solidId={solidId}
        />
      ))}
    </group>
  )
}

// ─── Per-object GDT fetcher + renderer ───────────────────────────────

interface ObjectGdtAnnotationsProps {
  objectId: string
  solidId: number
}

/**
 * Fetches the GD&T table for one solid and renders datum flags + FCF
 * badges. Isolated so each solid has its own fetch lifecycle and loading
 * state, without coupling the render of one solid to the in-flight
 * request of another.
 */
function ObjectGdtAnnotations({ solidId }: ObjectGdtAnnotationsProps) {
  const gdtVisible = useSceneStore((s) => s.gdtVisible)
  const { datums, annotations } = useGdt(solidId, gdtVisible)

  if (datums.length === 0 && annotations.length === 0) return null

  return (
    <group name={`gdt-solid-${solidId}`}>
      {datums.map((datum) => (
        <DatumFlag key={datum.persistent_id} datum={datum} />
      ))}
      {annotations.map((ann, i) => (
        <FcfBadge
          key={`${ann.feature_pid}-${i}`}
          annotation={ann}
          datums={datums}
        />
      ))}
    </group>
  )
}

// ─── Geometry constants ───────────────────────────────────────────────

/**
 * Apparent-size factor for all GDT `<Html>` badges. Mirrors
 * `HTML_DISTANCE_FACTOR` in PartDimensions (28): tuned to reproduce
 * the previous on-screen size at the default camera distance so GDT
 * badges scale with the model like dimension annotations.
 */
const HTML_DISTANCE_FACTOR = 28

// Blueprint grey for datum glyph geometry — same as PartDimensions'
// LEADER_COLOR so the overlay visual language is consistent.
const GLYPH_COLOR = '#4a5568'

// Datum-feature triangle glyph dimensions (ISO 1101 / ASME Y14.5 style).
// The triangle sits below the datum box, apex pointing at the surface.
const TRIANGLE_HALF = 2.0 // half-base width in mm
const TRIANGLE_HEIGHT = 3.0 // height in mm

// Leader standoff: the datum box is offset this many mm along the datum
// direction (or world-Y when the direction is parallel to world-Y).
const DATUM_STANDOFF_MM = 8

/**
 * Deterministic perpendicular to `dir` — same logic as PartDimensions'
 * `stablePerpendicular`. Cross with world-Y; fall back to world-X when
 * `dir` is near-parallel to Y (|dot| > 0.9).
 */
function stablePerpendicular(dir: THREE.Vector3): THREE.Vector3 {
  const worldY = new THREE.Vector3(0, 1, 0)
  const worldX = new THREE.Vector3(1, 0, 0)
  const ref = Math.abs(dir.dot(worldY)) > 0.9 ? worldX : worldY
  return new THREE.Vector3().crossVectors(dir, ref).normalize()
}

// ─── GD&T characteristic symbol map ──────────────────────────────────

/**
 * Map a kernel `characteristic` string to its ISO 1101 symbol.
 * Falls back to the characteristic name when no symbol is mapped
 * (covering future characteristics gracefully).
 */
const CHARACTERISTIC_SYMBOLS: Record<string, string> = {
  flatness: '⏥',
  perpendicularity: '⊥',
  parallelism: '∥',
  position: '⊕',
  straightness: '⏤',
  circularity: '○',
  cylindricity: '⌭',
  profile_line: '⌒',
  profile_surface: '⌓',
  angularity: '∠',
  concentricity: '◎',
  symmetry: '⌯',
  circular_runout: '↗',
  total_runout: '↗↗',
}

function characteristicSymbol(c: string): string {
  return CHARACTERISTIC_SYMBOLS[c.toLowerCase()] ?? c
}

// ─── FCF badge text formatter ─────────────────────────────────────────

/**
 * Build the compact FCF badge text from a verdict.
 * Form: `⊥ 0.05 | A B` — symbol, tolerance label, then datum refs
 * joined with spaces (separated from the tolerance by ` | `).
 *
 * All values are sourced from the kernel's `tolerance_label` and the
 * verdict's `datum_statuses` labels — zero frontend formatting.
 */
function fcfBadgeText(ann: GdtAnnotationWire): string {
  const symbol = characteristicSymbol(ann.verdict.characteristic)
  const tol = ann.verdict.tolerance_label
  const refs = ann.verdict.datum_statuses.map((d) => d.label)
  const datumPart = refs.length > 0 ? ` | ${refs.join(' ')}` : ''
  return `${symbol} ${tol}${datumPart}`
}

// ─── Conformance colour class ─────────────────────────────────────────

function conformanceClass(conforms: string): string {
  if (conforms === 'in_spec') return 'border-emerald-500/70 text-emerald-400'
  if (conforms === 'out_of_spec') return 'border-rose-500/70 text-rose-400'
  return 'border-border/50 text-muted-foreground'
}

// ─── Datum position helpers ───────────────────────────────────────────

/**
 * Resolve the world-space anchor for a live datum. Returns `null` for
 * dangling datums (no world position exists).
 */
function resolutionToAnchor(
  resolution: DatumResolutionWire,
): THREE.Vector3 | null {
  if (resolution.status !== 'live') return null
  return new THREE.Vector3(...resolution.origin)
}

/**
 * Resolve the badge position for an FCF annotation. Prefers the first
 * datum reference's live origin (the toleranced feature is typically
 * near its datum), falling back to world origin when no live datum is
 * available. This is PURELY an anchor choice for the badge position;
 * no verdict math is performed.
 */
function annotationAnchor(
  ann: GdtAnnotationWire,
  datums: GdtDatumWire[],
): THREE.Vector3 {
  // Find a datum referenced by this annotation that is live.
  for (const ds of ann.verdict.datum_statuses) {
    const datum = datums.find((d) => d.label === ds.label)
    if (!datum) continue
    const anchor = resolutionToAnchor(datum.resolution)
    if (anchor) {
      // Offset slightly so the FCF badge doesn't overlap the datum flag.
      return anchor.clone().add(new THREE.Vector3(0, DATUM_STANDOFF_MM * 2, 0))
    }
  }
  // No referenced datum is live — render at world origin (dangling context).
  return new THREE.Vector3(0, 0, 0)
}

// ─── DatumFlag ───────────────────────────────────────────────────────

/**
 * One datum flag: a boxed letter `[A]` badge above a datum-feature
 * triangle, anchored at the datum face's live resolution origin.
 *
 * Dangling datums (resolution.status === 'dangling') render in an amber
 * struck-through style — still visible and listed, never silently dropped.
 * They have no world anchor so they are rendered at the world origin with
 * a visual indicator that the feature is gone.
 *
 * Geometry (triangle glyph + leader) renders with `depthTest={false}` so
 * it is always visible even when inside the part.
 */
function DatumFlag({ datum }: { datum: GdtDatumWire }) {
  const isLive = datum.resolution.status === 'live'
  const anchor = useMemo(() => resolutionToAnchor(datum.resolution), [datum])

  const glyph = useMemo(() => {
    if (!anchor) return null

    // Datum direction: use the resolved direction for live plane/axis datums.
    const dir =
      datum.resolution.status === 'live'
        ? new THREE.Vector3(...datum.resolution.direction).normalize()
        : new THREE.Vector3(0, 1, 0)

    const perp = stablePerpendicular(dir)

    // Triangle apex sits at the anchor (on the feature surface).
    // The triangle base is DATUM_STANDOFF_MM above the anchor along the
    // datum direction (or perp when direction is ambiguous).
    // The box label sits above the base.
    const apex = anchor.clone()
    const base = anchor.clone().addScaledVector(dir, DATUM_STANDOFF_MM)
    const baseLeft = base.clone().addScaledVector(perp, -TRIANGLE_HALF)
    const baseRight = base.clone().addScaledVector(perp, TRIANGLE_HALF)
    const labelPos = base
      .clone()
      .addScaledVector(dir, TRIANGLE_HEIGHT + 1)

    return { apex, baseLeft, baseRight, labelPos }
  }, [anchor, datum.resolution])

  if (!glyph) {
    // Dangling: render badge only at world origin, no triangle.
    return (
      <Html
        position={[0, 0, 0]}
        center
        pointerEvents="none"
        zIndexRange={[100, 0]}
        distanceFactor={HTML_DISTANCE_FACTOR}
      >
        <div className="px-1.5 py-0.5 text-[10px] font-mono tracking-wider bg-background/80 border border-amber-500/60 text-amber-400/60 backdrop-blur-sm whitespace-nowrap select-none line-through decoration-amber-500/40">
          [{datum.label}] dangling
        </div>
      </Html>
    )
  }

  return (
    <group name={`datum-flag-${datum.label}`}>
      {/* Leader line from apex to base */}
      <Line
        points={[glyph.apex, glyph.baseLeft.clone().lerp(glyph.baseRight, 0.5)]}
        color={GLYPH_COLOR}
        lineWidth={1.5}
        opacity={0.85}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      {/* Datum-feature triangle (base + two sides) */}
      <Line
        points={[glyph.baseLeft, glyph.baseRight]}
        color={GLYPH_COLOR}
        lineWidth={1.5}
        opacity={0.85}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      <Line
        points={[glyph.baseLeft, glyph.apex]}
        color={GLYPH_COLOR}
        lineWidth={1.5}
        opacity={0.85}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      <Line
        points={[glyph.baseRight, glyph.apex]}
        color={GLYPH_COLOR}
        lineWidth={1.5}
        opacity={0.85}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      {/* Boxed letter badge */}
      <Html
        position={glyph.labelPos}
        center
        pointerEvents="none"
        zIndexRange={[100, 0]}
        distanceFactor={HTML_DISTANCE_FACTOR}
      >
        <div
          className={`px-1.5 py-0.5 text-[10px] font-mono font-semibold tracking-wider bg-background/85 border backdrop-blur-sm whitespace-nowrap select-none ${
            isLive
              ? 'border-border/70 text-foreground'
              : 'border-amber-500/60 text-amber-400/60 line-through decoration-amber-500/40'
          }`}
        >
          [{datum.label}]
        </div>
      </Html>
    </group>
  )
}

// ─── FcfBadge ────────────────────────────────────────────────────────

/**
 * One FCF badge: a bordered frame text annotation near the toleranced
 * feature, conformance-coloured (green / red / grey). Hover reveals a
 * tooltip panel showing measured vs tolerance + fit residual, all
 * verbatim from the kernel's unit-formatted `*_label` fields.
 *
 * The badge is the only hover target in this group — the leader line
 * never intercepts pointer events.
 */
function FcfBadge({
  annotation,
  datums,
}: {
  annotation: GdtAnnotationWire
  datums: GdtDatumWire[]
}) {
  const [hovered, setHovered] = useState(false)

  const badgePos = useMemo(
    () => annotationAnchor(annotation, datums),
    [annotation, datums],
  )

  const badgeText = useMemo(() => fcfBadgeText(annotation), [annotation])
  const colorClass = conformanceClass(annotation.verdict.conforms)

  return (
    <group name={`fcf-badge-${annotation.feature_pid}`}>
      <Html
        position={badgePos}
        center
        pointerEvents="none"
        zIndexRange={[100, 0]}
        distanceFactor={HTML_DISTANCE_FACTOR}
      >
        <div className="relative select-none">
          {/* FCF bordered frame badge */}
          <div
            onPointerEnter={() => setHovered(true)}
            onPointerLeave={() => setHovered(false)}
            className={`pointer-events-auto cursor-default px-1.5 py-0.5 text-[10px] font-mono tracking-wider bg-background/85 border-2 backdrop-blur-sm whitespace-nowrap ${colorClass}`}
          >
            {badgeText}
          </div>
          {/* Hover tooltip: measured vs tolerance + residual */}
          {hovered && (
            <FcfTooltip annotation={annotation} />
          )}
        </div>
      </Html>
    </group>
  )
}

// ─── FcfTooltip ──────────────────────────────────────────────────────

/**
 * Hover tooltip for an FCF badge — shows the full verdict detail:
 * measured value, tolerance, fit residual, and any not-evaluable reason.
 * All values are displayed verbatim from kernel `*_label` fields.
 * Zero frontend math.
 */
function FcfTooltip({ annotation }: { annotation: GdtAnnotationWire }) {
  const v = annotation.verdict

  const conformsLabel =
    v.conforms === 'in_spec'
      ? 'IN SPEC'
      : v.conforms === 'out_of_spec'
        ? 'OUT OF SPEC'
        : 'NOT EVALUABLE'

  const conformsColor =
    v.conforms === 'in_spec'
      ? 'text-emerald-400'
      : v.conforms === 'out_of_spec'
        ? 'text-rose-400'
        : 'text-muted-foreground'

  return (
    <div className="absolute left-full top-0 ml-1.5 z-10 min-w-[160px] cad-panel px-2.5 py-1.5 text-[10px] uppercase tracking-wider pointer-events-none whitespace-nowrap">
      <div className={`font-semibold mb-1 ${conformsColor}`}>
        {conformsLabel}
      </div>
      <div className="flex items-center justify-between gap-3">
        <span className="text-muted-foreground">Tolerance</span>
        <span className="text-foreground font-mono">{v.tolerance_label}</span>
      </div>
      {v.measured_label !== null && (
        <div className="flex items-center justify-between gap-3">
          <span className="text-muted-foreground">Measured</span>
          <span className="text-foreground font-mono">{v.measured_label}</span>
        </div>
      )}
      {v.fit_residual_mm !== null && (
        <div className="flex items-center justify-between gap-3">
          <span className="text-muted-foreground">Residual</span>
          <span className="text-foreground font-mono">
            {v.fit_residual_mm.toExponential(2)}mm
          </span>
        </div>
      )}
      {v.reason !== null && (
        <div className="mt-1 pt-1 border-t border-border/40 text-muted-foreground/80 normal-case leading-relaxed max-w-[200px] whitespace-normal">
          {v.reason}
        </div>
      )}
    </div>
  )
}
