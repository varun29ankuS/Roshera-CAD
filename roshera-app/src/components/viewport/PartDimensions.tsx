import { useMemo } from 'react'
import { Html, Line } from '@react-three/drei'
import * as THREE from 'three'
import { X } from 'lucide-react'
import { useSceneStore, type PinnedMeasurement } from '@/stores/scene-store'
import type { DatumDescriptor } from '@/lib/measure-api'
import { usePartDimensions, type DimensionRow } from './use-part-dimensions'
import { usePinnedMeasurementsRevalidation } from './use-pinned-measurements'

/**
 * Live kernel dimension annotations for every object whose id is in
 * `showDimensions`, plus the user's pinned interactive measurements.
 * Renders inside the R3F `<Canvas>` scene graph so all positions track
 * camera orbit / pan / zoom automatically via drei's `<Html>` and
 * `<Line>`.
 *
 * ## Render forms (Amendment A2)
 * Three per-row forms, routed by kind:
 *   - **Linear rows** (`extent`, `length`, `position`): real ISO-129
 *     style dimension graphics — endpoints at `anchor ± direction ·
 *     value/2` (the same math the 2D sheet uses; the kernel constructs
 *     anchor as the span midpoint and direction as the span axis for
 *     all three kinds — see readable/dimensions.rs), a span `<Line>`
 *     with an arrowhead cone at each end pointing outward, two short
 *     extension ticks perpendicular at the endpoints, and the text
 *     billboard centred on the span, offset a couple mm perpendicular
 *     so it doesn't sit on the line.
 *   - **Leader rows** (`diameter`, `radius`, `angle`, any future kind
 *     with a direction): unchanged — a short leader from `anchor` to a
 *     6 mm perpendicular offset, badge at the tip (Ø / ∠ prefixes are
 *     part of the kernel label).
 *   - **Direction-less rows** (`direction === null`, e.g. face-info
 *     measure results): badge at the anchor, no graphics.
 * Rows carrying a `datum` additionally contribute a datum marker glyph
 * (deduped by origin) so position numbers visibly reference it.
 *
 * ## Display labels
 * Bare axis/length prefixes ("X " / "Y " / "Z " / "L ") are stripped
 * from DISPLAYED text only — the graphics now carry the direction, so
 * "X 60.00" reads as a clean "60.00" on an X-spanning dimension line.
 * Ø / SØ / ∠ prefixes are kept (they are meaning, not axis shorthand).
 * The wire `label` is never modified — drawings still consume it.
 *
 * ## Perpendicular choice
 * All perpendiculars (leader offset, extension ticks, text offset) come
 * from one deterministic rule: cross the dimension direction with
 * world-Y, falling back to world-X when the direction is near-parallel
 * to Y (|dot| > 0.9). Computed from kernel vectors, never from the
 * camera — stable across orbit.
 *
 * ## Depth strategy
 * All dimension graphics (span, ticks, arrow cones, datum glyph) render
 * with `depthTest={false}` + `depthWrite={false}` — the house overlay
 * style (`SubElementHighlight` edge lines and vertex marks, `Datums`'
 * origin marker). Annotations draw on top of the part, so they can
 * never z-fight it and a driller can always read them; this beats a
 * surface-offset approach here because dimension lines legitimately lie
 * ON part edges (extents run along the AABB edge).
 *
 * ## pointerEvents
 * Ambient annotations are read-only (`pointerEvents="none"` on `<Html>`,
 * `raycast={() => null}` on every `<Line>`/`<mesh>`) so this layer never
 * intercepts orbit, selection, or sketch clicks. Pinned measurements are
 * the one exception: their ✕ dismiss button re-enables pointer events on
 * JUST that button (CSS `pointer-events: auto` inside a `pointer-events:
 * none` wrapper) — the rest of the pin chip stays click-through.
 *
 * ## Pinned measurements
 * Rendered in the app's primary accent tone (visually distinct from the
 * neutral ambient dimensions) at `row.anchor`, no leader. Re-validated
 * on every geometry change by `usePinnedMeasurementsRevalidation`,
 * mounted here because this component is the single place pins can be
 * visible (see that hook's doc for the full policy).
 */
export function PartDimensions() {
  const showDimensions = useSceneStore((s) => s.showDimensions)
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const pinnedMeasurements = useSceneStore((s) => s.pinnedMeasurements)

  // Keep every pinned measurement honest against the current geometry.
  // Must be called unconditionally (before any early return) — pins can
  // exist and need re-validation even when no ambient layer is toggled.
  usePinnedMeasurementsRevalidation()

  // Collect objects that have dimensions toggled on and carry a solidId,
  // pairing each id with its (narrowed) kernel solid id up front so the
  // render below needs no non-null assertions.
  const active: Array<{ objectId: string; solidId: number }> = []
  for (const id of objectOrder) {
    if (!showDimensions.has(id)) continue
    const solidId = objects.get(id)?.analyticalGeometry?.solidId
    if (solidId === undefined) continue
    active.push({ objectId: id, solidId })
  }

  if (active.length === 0 && pinnedMeasurements.length === 0) return null

  return (
    <group name="part-dimensions">
      {active.map(({ objectId, solidId }) => (
        <ObjectDimensions
          key={objectId}
          objectId={objectId}
          solidId={solidId}
        />
      ))}
      {pinnedMeasurements.map((pin) => (
        <PinnedMeasurementAnnotation key={pin.id} pin={pin} />
      ))}
    </group>
  )
}

// ─── Per-object dimension fetcher + renderer ─────────────────────────

interface ObjectDimensionsProps {
  objectId: string
  solidId: number
}

/**
 * Fetches the dimension table for one solid and renders its rows plus
 * one datum marker per DISTINCT datum (deduped by origin — every
 * position row of a part typically shares the same part-corner datum,
 * which must render as one glyph, not one per row).
 *
 * Isolated so each toggled object has its own fetch lifecycle and
 * loading state, without coupling the render of one solid to the
 * in-flight request of another.
 */
function ObjectDimensions({ objectId, solidId }: ObjectDimensionsProps) {
  const { dimensions } = usePartDimensions(solidId, true)

  // Distinct datums referenced by this part's rows, keyed by origin.
  // Dedupe is by origin (not name): two descriptors at the same point
  // are one physical reference; the first row's descriptor wins.
  const datums = useMemo(() => {
    const map = new Map<string, DatumDescriptor>()
    for (const row of dimensions) {
      if (!row.datum) continue
      const key = row.datum.origin.join(',')
      if (!map.has(key)) map.set(key, row.datum)
    }
    return Array.from(map.entries())
  }, [dimensions])

  if (dimensions.length === 0) return null

  return (
    <group name={`part-dimensions-${objectId}`}>
      {dimensions.map((row) => (
        <DimensionAnnotation key={row.id} row={row} />
      ))}
      {datums.map(([key, datum]) => (
        <DatumMarker key={key} datum={datum} />
      ))}
    </group>
  )
}

// ─── Geometry helpers ─────────────────────────────────────────────────

/**
 * Deterministic perpendicular to `dir`: cross with world-Y, falling
 * back to world-X when `dir` is near-parallel to Y (|dot| > 0.9).
 * Computed from the kernel direction vector, never the camera — the
 * same annotation geometry every frame, every orbit.
 */
function stablePerpendicular(dir: THREE.Vector3): THREE.Vector3 {
  const worldY = new THREE.Vector3(0, 1, 0)
  const worldX = new THREE.Vector3(1, 0, 0)
  const ref = Math.abs(dir.dot(worldY)) > 0.9 ? worldX : worldY
  return new THREE.Vector3().crossVectors(dir, ref).normalize()
}

/**
 * Leader tip for leader-form rows (diameter / radius / angle): 6 mm
 * from the anchor along the stable perpendicular — the witness-line
 * gap used in engineering drawing templates.
 */
const LEADER_OFFSET_MM = 6

function leaderEndpoint(
  anchor: THREE.Vector3,
  direction: THREE.Vector3,
): THREE.Vector3 {
  return anchor
    .clone()
    .addScaledVector(stablePerpendicular(direction), LEADER_OFFSET_MM)
}

// ─── Kind routing ────────────────────────────────────────────────────

/**
 * Linear kinds render as real dimension graphics (span + arrows +
 * extension ticks). For all three the kernel constructs `anchor` as the
 * span MIDPOINT and `direction` as the span axis, so the endpoints are
 * exactly `anchor ± direction · value/2` (verified against
 * readable/dimensions.rs):
 *   - `extent`: anchor = AABB axis midpoint on the min-min edge,
 *     direction = world axis → spans bb.min→bb.max.
 *   - `length`: anchor = cylinder lateral at mid-height, direction =
 *     cylinder axis → spans the face's axial extent.
 *   - `position`: anchor = midpoint between datum corner and bore axis,
 *     direction = signed world axis toward the axis → spans
 *     corner→axis (Amendment A2).
 */
function isLinear(kind: string): boolean {
  return kind === 'extent' || kind === 'length' || kind === 'position'
}

/**
 * Display-only label cleanup (Amendment A2 item 3): strip a bare
 * leading axis / length shorthand ("X " / "Y " / "Z " / "L ") — the
 * dimension graphics now show the direction, so the prefix is noise for
 * the machinist. Ø / SØ / ∠ prefixes carry meaning and are untouched.
 * The wire label is NEVER modified; this runs at render time only.
 */
// Known-safe non-stripped prefixes: "A " (face_info area), "Ø"/"SØ"
// (diameters), "∠" (angles). If a future kind introduces a label
// starting with a bare X/Y/Z/L that must SURVIVE display, extend
// this regex guard — silent stripping is the failure mode.
function displayLabel(label: string): string {
  return label.replace(/^[XYZL]\s+/, '')
}

// ─── DimBadge ────────────────────────────────────────────────────────

/**
 * Read-only annotation badge rendered via drei `<Html>`. Replicates
 * the exact CSS class string of `SketchOverlay.tsx`'s `DimLabel`
 * read-only path so the two annotation layers share a consistent visual
 * language.
 *
 * `DimLabel` in `SketchOverlay.tsx` is a file-local function — it is
 * not exported and carries `onCommit` / `value` / `variant` props for
 * interactive editing which this read-only layer does not need. A
 * `DimBadge` here avoids entangling the dimension layer with the sketch
 * store state that `SketchOverlay` depends on, and keeps the import
 * graph clean. The visual output is identical to the read-only
 * `DimLabel` variant (no `onCommit`).
 */
function DimBadge({ position, text }: { position: THREE.Vector3; text: string }) {
  return (
    <Html
      position={position}
      center
      pointerEvents="none"
      zIndexRange={[100, 0]}
    >
      <div className="px-1.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-background/80 border border-border/60 text-foreground backdrop-blur-sm whitespace-nowrap select-none pointer-events-none">
        {text}
      </div>
    </Html>
  )
}

// ─── Dimension graphics constants ────────────────────────────────────

// Annotation tint: a neutral blueprint grey matching the --cad-tick
// CSS variable used by CADMesh edge overlays and the sketch plane
// border. Hardcoded as a constant so the component tree stays free of
// runtime CSS-variable lookups (which would need a ref to the canvas
// DOM element). This is the same approach PartLabels uses for its
// neutral chip tone.
const LEADER_COLOR = '#4a5568'

/** Arrowhead cone: length along the span, base radius. ISO-ish 3:1. */
const ARROW_LEN_MM = 1.6
const ARROW_RADIUS_MM = 0.5
/** Extension tick half-length each side of the endpoint. */
const EXT_TICK_MM = 2
/** Text billboard offset from the span line, along the perpendicular. */
const TEXT_OFFSET_MM = 2.5

// One shared cone geometry for every arrowhead in the layer (module
// scope — created once, never disposed; ~100 B of GPU memory). Cone
// apex is at +Y·(len/2) in local space; each arrow orients it via a
// quaternion so the apex lands exactly on the span endpoint.
const ARROW_GEOMETRY = new THREE.ConeGeometry(
  ARROW_RADIUS_MM,
  ARROW_LEN_MM,
  8,
)

const UNIT_Y = new THREE.Vector3(0, 1, 0)

// ─── Linear dimension graphics ───────────────────────────────────────

interface ArrowTransform {
  position: THREE.Vector3
  quaternion: THREE.Quaternion
}

/**
 * Real dimension graphics for one linear row: span line between the
 * kernel-defined endpoints, an arrowhead cone at each end pointing
 * outward (apex ON the endpoint, body toward the span centre — the
 * standard inside-arrows dimension form), a short perpendicular
 * extension tick at each endpoint, and the value billboard centred on
 * the span offset off the line.
 *
 * Everything here is DISPLAY geometry derived from kernel-provided
 * `anchor` / `direction` / `value` — no value is computed frontend-side.
 */
function LinearDimensionGraphic({ row }: { row: DimensionRow }) {
  const geo = useMemo(() => {
    // Router guarantees direction is non-null for linear rows.
    const dir = new THREE.Vector3(...(row.direction as [number, number, number])).normalize()
    const anchor = new THREE.Vector3(...row.anchor)
    const half = row.value / 2
    const p1 = anchor.clone().addScaledVector(dir, -half)
    const p2 = anchor.clone().addScaledVector(dir, half)
    const perp = stablePerpendicular(dir)

    const ext1: [THREE.Vector3, THREE.Vector3] = [
      p1.clone().addScaledVector(perp, -EXT_TICK_MM),
      p1.clone().addScaledVector(perp, EXT_TICK_MM),
    ]
    const ext2: [THREE.Vector3, THREE.Vector3] = [
      p2.clone().addScaledVector(perp, -EXT_TICK_MM),
      p2.clone().addScaledVector(perp, EXT_TICK_MM),
    ]

    // Arrow at p1: apex exactly at p1 pointing outward (-dir), body
    // toward the centre. ConeGeometry's apex is +Y·(len/2) from its
    // centre, so the mesh centre sits half a length inside the span.
    const arrow1: ArrowTransform = {
      position: p1.clone().addScaledVector(dir, ARROW_LEN_MM / 2),
      quaternion: new THREE.Quaternion().setFromUnitVectors(
        UNIT_Y,
        dir.clone().negate(),
      ),
    }
    const arrow2: ArrowTransform = {
      position: p2.clone().addScaledVector(dir, -ARROW_LEN_MM / 2),
      quaternion: new THREE.Quaternion().setFromUnitVectors(UNIT_Y, dir),
    }

    const textPos = anchor.clone().addScaledVector(perp, TEXT_OFFSET_MM)

    return { p1, p2, ext1, ext2, arrow1, arrow2, textPos }
  }, [row])

  return (
    <>
      {/* Span line between the kernel endpoints. */}
      <Line
        points={[geo.p1, geo.p2]}
        color={LEADER_COLOR}
        lineWidth={1}
        opacity={0.85}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      {/* Extension ticks, perpendicular at each endpoint. */}
      <Line
        points={geo.ext1}
        color={LEADER_COLOR}
        lineWidth={1}
        opacity={0.6}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      <Line
        points={geo.ext2}
        color={LEADER_COLOR}
        lineWidth={1}
        opacity={0.6}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      {/* Arrowhead cones, apex on the endpoints, pointing outward. */}
      {[geo.arrow1, geo.arrow2].map((a, i) => (
        <mesh
          key={i}
          geometry={ARROW_GEOMETRY}
          position={a.position}
          quaternion={a.quaternion}
          raycast={() => null}
        >
          <meshBasicMaterial
            color={LEADER_COLOR}
            transparent
            opacity={0.9}
            depthTest={false}
            depthWrite={false}
          />
        </mesh>
      ))}
      <DimBadge position={geo.textPos} text={displayLabel(row.label)} />
    </>
  )
}

// ─── Datum marker ────────────────────────────────────────────────────

// Axis triad colors — same palette as `Datums.tsx` (X red / Y green /
// Z blue) so the glyph reads with the app's existing axis vocabulary.
const DATUM_AXIS_COLORS = ['#e74c3c', '#2ecc71', '#3498db'] as const
const DATUM_ARM_MM = 4
/** Label offset along the body diagonal so it clears the triad glyph. */
const DATUM_LABEL_OFFSET = new THREE.Vector3(1, 1, 1)
  .normalize()
  .multiplyScalar(5)

/**
 * Anchor glyph for one distinct datum: a small axis-colored triad
 * (three 4 mm arms along +X/+Y/+Z from the datum origin — the corner
 * bracket a machinist clamps against) plus a tiny name billboard. The
 * driller must SEE what the position numbers reference.
 *
 * Same depth strategy as the dimension graphics: `depthTest={false}` so
 * the reference marker is always visible, even inside the part corner.
 */
function DatumMarker({ datum }: { datum: DatumDescriptor }) {
  const geo = useMemo(() => {
    const origin = new THREE.Vector3(...datum.origin)
    const arms: Array<[THREE.Vector3, THREE.Vector3]> = [
      [origin, origin.clone().add(new THREE.Vector3(DATUM_ARM_MM, 0, 0))],
      [origin, origin.clone().add(new THREE.Vector3(0, DATUM_ARM_MM, 0))],
      [origin, origin.clone().add(new THREE.Vector3(0, 0, DATUM_ARM_MM))],
    ]
    const labelPos = origin.clone().add(DATUM_LABEL_OFFSET)
    return { arms, labelPos }
  }, [datum])

  return (
    <group name={`datum-${datum.name}`}>
      {geo.arms.map((points, i) => (
        <Line
          key={i}
          points={points}
          color={DATUM_AXIS_COLORS[i]}
          lineWidth={2}
          opacity={0.9}
          transparent
          depthTest={false}
          depthWrite={false}
          raycast={() => null}
        />
      ))}
      <DimBadge position={geo.labelPos} text={datum.name} />
    </group>
  )
}

// ─── Single annotation (kind router) ─────────────────────────────────

interface DimensionAnnotationProps {
  row: DimensionRow
}

/**
 * Route one row to its render form: linear → full dimension graphics;
 * direction-less → bare badge at the anchor; everything else
 * (diameter / radius / angle) → the leader + badge form.
 */
function DimensionAnnotation({ row }: DimensionAnnotationProps) {
  // Memoised per row: the row object identity only changes when a fresh
  // fetch lands, so the Vector3s (anchor, leader endpoint) are computed
  // once per fetch instead of on every render of every annotation.
  const { anchorVec, endVec } = useMemo(() => {
    const anchor = new THREE.Vector3(...row.anchor)
    if (isLinear(row.kind) || row.direction === null) {
      // Linear rows build their own geometry in LinearDimensionGraphic;
      // direction-less rows need no leader. Neither uses endVec.
      return { anchorVec: anchor, endVec: null }
    }
    const dir = new THREE.Vector3(...row.direction)
    return { anchorVec: anchor, endVec: leaderEndpoint(anchor, dir) }
  }, [row])

  if (isLinear(row.kind) && row.direction !== null) {
    return <LinearDimensionGraphic row={row} />
  }

  if (endVec === null) {
    // No direction to draw with: badge at the anchor, no graphics.
    return <DimBadge position={anchorVec} text={displayLabel(row.label)} />
  }

  // Leader form (diameter / radius / angle): leader from anchor to the
  // perpendicular offset tip. The leader must never be a raycast target
  // — `raycast={() => null}` removes it from Three.js hit-testing
  // entirely so orbit / selection clicks pass straight through.
  return (
    <>
      <Line
        points={[anchorVec, endVec]}
        color={LEADER_COLOR}
        lineWidth={1}
        opacity={0.7}
        transparent
        depthTest={false}
        depthWrite={false}
        raycast={() => null}
      />
      <DimBadge position={endVec} text={displayLabel(row.label)} />
    </>
  )
}

// ─── Pinned measurement annotation ───────────────────────────────────

/**
 * One user-pinned interactive measurement. Anchored at `row.anchor`,
 * no leader (the measured relation spans two faces — a single-anchor
 * leader would point at only one of them).
 *
 * Visually distinct from ambient dimensions: the app's primary accent
 * tone (`border-primary/70 text-primary`) instead of the neutral
 * `border-border/60 text-foreground` — the same distinction the spec
 * calls for ("visually distinct accent color").
 *
 * The outer `<Html>` keeps `pointerEvents="none"` so the chip never
 * blocks orbit; `pointer-events` is an inherited CSS property, so the
 * ✕ button re-enables it locally with `pointer-events-auto`. Only that
 * button is clickable.
 */
function PinnedMeasurementAnnotation({ pin }: { pin: PinnedMeasurement }) {
  const dismissMeasurement = useSceneStore((s) => s.dismissMeasurement)

  const anchorVec = useMemo(
    () => new THREE.Vector3(...pin.row.anchor),
    [pin.row],
  )

  return (
    <Html
      position={anchorVec}
      center
      pointerEvents="none"
      zIndexRange={[110, 0]}
    >
      <div className="flex items-center gap-1 px-1.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-background/85 border border-primary/70 text-primary backdrop-blur-sm whitespace-nowrap select-none">
        <span>{pin.row.label}</span>
        <button
          type="button"
          onClick={() => dismissMeasurement(pin.id)}
          className="pointer-events-auto cursor-pointer shrink-0 text-primary/70 hover:text-primary transition-colors"
          title="Dismiss measurement"
          aria-label="Dismiss measurement"
        >
          <X className="w-3 h-3" aria-hidden />
        </button>
      </div>
    </Html>
  )
}
