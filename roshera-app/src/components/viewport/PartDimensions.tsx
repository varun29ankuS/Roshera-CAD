import { useEffect, useMemo, useState } from 'react'
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
 * Labels arrive unit-formatted from the kernel (e.g. "Ø0.236in") and are
 * displayed VERBATIM (minus the axis-prefix strip below) — this layer
 * performs zero unit conversion; a document-unit change re-fires the
 * fetch via `unitEpoch` (see use-part-dimensions).
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
 * ## Display polish (ui-units spec section 4)
 *   - **Model-locked text scale**: every annotation `<Html>` carries
 *     `distanceFactor={HTML_DISTANCE_FACTOR}` so the text scales WITH
 *     the dimension graphics (which are world-sized lines/cones) — the
 *     badge keeps the same apparent size relative to the model and its
 *     own arrows at any zoom, exactly like text on the 2D sheet. The
 *     value is tuned to reproduce the previous on-screen size at the
 *     default camera distance (see the constant's doc).
 *   - **Stacked standoffs**: linear dims sharing a span axis + lane are
 *     staggered deterministically (see {@link computeStandoffs}).
 *   - **Kind filter**: rows whose chip (`extent` / `diameter` /
 *     `length` / `position`) is toggled off in
 *     `dimensionKindFilter` are skipped — including their standoff
 *     slots and datum markers, so hiding positions also hides the
 *     datums nothing references anymore. Chips UI lives in CADViewport.
 *   - **Hover linking**: hovering a badge tints the row's entity faces
 *     (same per-triangle faceIds fan-out as PartLabels' LabelFaceTints)
 *     and emphasizes the row's datum marker. Hover state is local to
 *     each ObjectDimensions instance — never global store.
 *
 * ## Display labels
 * Bare axis/length prefixes ("X " / "Y " / "Z " / "L ") are stripped
 * from DISPLAYED text only — the graphics now carry the direction.
 * Ø / SØ / ∠ prefixes are kept (they are meaning, not axis shorthand).
 * The wire `label` is never modified — drawings still consume it.
 *
 * ## Perpendicular choice
 * All perpendiculars (leader offset, extension ticks, text offset,
 * standoff direction) come from one deterministic rule: cross the
 * dimension direction with world-Y, falling back to world-X when the
 * direction is near-parallel to Y (|dot| > 0.9). Computed from kernel
 * vectors, never from the camera — stable across orbit.
 *
 * ## Depth strategy
 * All dimension graphics (span, ticks, arrow cones, datum glyph) render
 * with `depthTest={false}` + `depthWrite={false}` — the house overlay
 * style (`SubElementHighlight` edge lines and vertex marks, `Datums`'
 * origin marker). The hover face tint instead follows LabelFaceTints:
 * inflated 0.002 proud of the surface with the depth test ON, so the
 * tint reads as paint on the face, not as x-ray.
 *
 * ## pointerEvents
 * Lines / cones / tints are never raycast targets
 * (`raycast={() => null}`). `<Html>` wrappers stay
 * `pointerEvents="none"`; ONLY badge divs that participate in hover
 * linking re-enable events locally (`pointer-events-auto`), and the
 * pinned ✕ button keeps its existing click affordance.
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
 * Map a kernel row kind to its filter chip, or `null` for kinds no chip
 * governs (angles, interactive-measure kinds) — those always render.
 * `radius` shares the Ø chip: both are hole/boss size callouts.
 */
function chipKindOf(kind: string): string | null {
  if (kind === 'extent' || kind === 'length' || kind === 'position') {
    return kind
  }
  if (kind === 'diameter' || kind === 'radius') return 'diameter'
  return null
}

/**
 * Fetches the dimension table for one solid and renders its rows plus
 * one datum marker per DISTINCT datum (deduped by origin — every
 * position row of a part typically shares the same part-corner datum,
 * which must render as one glyph, not one per row).
 *
 * Also owns the layer-local hover state: the hovered row's entity
 * faces get a tint overlay and its datum marker is emphasized. State
 * lives here (per object) rather than in the store — hover is a
 * transient, render-local concern.
 *
 * Isolated so each toggled object has its own fetch lifecycle and
 * loading state, without coupling the render of one solid to the
 * in-flight request of another.
 */
function ObjectDimensions({ objectId, solidId }: ObjectDimensionsProps) {
  const { dimensions } = usePartDimensions(solidId, true)
  const kindFilter = useSceneStore((s) => s.dimensionKindFilter)
  const [hovered, setHovered] = useState<DimensionRow | null>(null)

  // Kind filter runs BEFORE standoffs and datum dedupe: a hidden kind
  // must not reserve a stagger slot, and datums referenced only by
  // hidden position rows must not render.
  const visible = useMemo(
    () =>
      dimensions.filter((row) => {
        const chip = chipKindOf(row.kind)
        return chip === null || kindFilter.has(chip)
      }),
    [dimensions, kindFilter],
  )

  const standoffs = useMemo(() => computeStandoffs(visible), [visible])

  // Distinct datums referenced by this part's visible rows, keyed by
  // origin. Dedupe is by origin (not name): two descriptors at the same
  // point are one physical reference; the first row's descriptor wins.
  const datums = useMemo(() => {
    const map = new Map<string, DatumDescriptor>()
    for (const row of visible) {
      if (!row.datum) continue
      const key = row.datum.origin.join(',')
      if (!map.has(key)) map.set(key, row.datum)
    }
    return Array.from(map.entries())
  }, [visible])

  // Derive (don't effect): a hovered row that a fresh fetch or a chip
  // toggle removed simply stops linking — no set-state-in-effect needed;
  // the stale reference is dropped on the next hover.
  const hoveredRow = hovered !== null && visible.includes(hovered) ? hovered : null
  const hoveredDatumKey = hoveredRow?.datum
    ? hoveredRow.datum.origin.join(',')
    : null

  if (visible.length === 0) return null

  return (
    <group name={`part-dimensions-${objectId}`}>
      {visible.map((row) => (
        <DimensionAnnotation
          key={row.id}
          row={row}
          standoff={standoffs.get(row.id) ?? null}
          // Hover linking only pays off when there is something to
          // link: entity faces to tint or a datum to emphasize.
          onHoverChange={
            row.entities.length > 0 || row.datum
              ? (hovering) => setHovered(hovering ? row : null)
              : undefined
          }
        />
      ))}
      {datums.map(([key, datum]) => (
        <DatumMarker
          key={key}
          datum={datum}
          emphasized={key === hoveredDatumKey}
        />
      ))}
      {hoveredRow !== null && hoveredRow.entities.length > 0 && (
        <DimensionFaceTints
          objectId={objectId}
          entityIds={hoveredRow.entities}
        />
      )}
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

// ─── Stacked standoffs ───────────────────────────────────────────────

/**
 * Stagger step between successive dims in one stack. ~3.5 mm world at
 * default zoom — a badge-height of clearance, inside the spec's 3–4 mm
 * guidance.
 */
const STANDOFF_STEP_MM = 3.5

/**
 * Lane quantisation for "measured along the same line". Two spans whose
 * anchors sit within one bin of each other perpendicular to the span
 * axis are treated as sharing a lane and get staggered apart. 4 mm ≈
 * the visual thickness of a dimension (line + text) at default zoom.
 */
const LANE_BIN_MM = 4

/**
 * Sign-canonicalise a unit direction so +d and −d land in the same
 * stack group (a span along −X shares the corner edge with one along
 * +X): flip so the first non-zero component is positive.
 */
function canonicalDirection(d: THREE.Vector3): THREE.Vector3 {
  const c = d.clone()
  const eps = 1e-6
  const flip =
    c.x < -eps ||
    (Math.abs(c.x) <= eps &&
      (c.y < -eps || (Math.abs(c.y) <= eps && c.z < -eps)))
  return flip ? c.negate() : c
}

/**
 * Deterministic ISO-129-style stacking (ui-units polish item 2).
 *
 * Groups the LINEAR rows by (sign-canonicalised unit direction, rough
 * perpendicular lane) — the lane is the anchor's component perpendicular
 * to the span axis, binned to {@link LANE_BIN_MM} so near-collinear dims
 * (e.g. two X-position dims sharing the datum corner edge, or the X
 * extent along the same edge) fall in one group. Within a group, rows
 * sort by value ascending (ties broken by row id for total determinism);
 * the smallest stays at its kernel position and each successive row is
 * offset k·{@link STANDOFF_STEP_MM} along the group's canonical
 * perpendicular. Non-linear rows never stack (leader form has no span).
 *
 * Pure function of the row list — same rows in, same offsets out, every
 * render, every orbit.
 */
function computeStandoffs(
  rows: DimensionRow[],
): Map<string, THREE.Vector3> {
  const groups = new Map<string, DimensionRow[]>()

  for (const row of rows) {
    if (!isLinear(row.kind) || row.direction === null) continue
    const dir = canonicalDirection(
      new THREE.Vector3(...row.direction).normalize(),
    )
    const anchor = new THREE.Vector3(...row.anchor)
    // Lane = anchor with its along-span component removed, binned.
    const residual = anchor.clone().addScaledVector(dir, -anchor.dot(dir))
    const key = [
      dir.x.toFixed(3),
      dir.y.toFixed(3),
      dir.z.toFixed(3),
      Math.round(residual.x / LANE_BIN_MM),
      Math.round(residual.y / LANE_BIN_MM),
      Math.round(residual.z / LANE_BIN_MM),
    ].join('|')
    const group = groups.get(key)
    if (group) {
      group.push(row)
    } else {
      groups.set(key, [row])
    }
  }

  const out = new Map<string, THREE.Vector3>()
  for (const group of groups.values()) {
    if (group.length < 2) continue
    group.sort(
      (a, b) => a.value - b.value || a.id.localeCompare(b.id),
    )
    // One shared perpendicular per group (from the canonical direction)
    // so every member staggers to the SAME side in consistent steps.
    const first = group[0]
    const dir0 = canonicalDirection(
      new THREE.Vector3(
        ...(first.direction as [number, number, number]),
      ).normalize(),
    )
    const perp = stablePerpendicular(dir0)
    group.forEach((row, i) => {
      if (i === 0) return // smallest span stays at the kernel position
      out.set(row.id, perp.clone().multiplyScalar(i * STANDOFF_STEP_MM))
    })
  }
  return out
}

// ─── DimBadge ────────────────────────────────────────────────────────

/**
 * Apparent-size factor for every annotation `<Html>` in this layer.
 *
 * With `distanceFactor`, drei scales the HTML like a world-space object
 * (scale = factor / (2·tan(fov/2)·distance) for the perspective camera,
 * factor·zoom for the orthographic one), so the text zooms WITH the
 * dimension graphics instead of staying a fixed pixel size — readable
 * when zoomed out relative to everything else on screen, never
 * billboard-huge over a small part.
 *
 * PartLabels does not use distanceFactor (its chips are pick-targets
 * that must stay finger-sized), so per the spec the value is tuned to
 * reproduce this layer's previous on-screen size at the DEFAULT camera:
 * scale = 1 wants factor = 2·tan(fov/2)·distance with the app's
 * PERSPECTIVE_FOV = 50 (CameraController) and the ~30-unit default
 * camera distance (CAMERA_PRESETS): 2·tan(25°)·30 ≈ 28.
 */
const HTML_DISTANCE_FACTOR = 28

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
 * graph clean.
 *
 * When `onHoverChange` is provided the badge div (and ONLY the div —
 * the `<Html>` wrapper stays `pointerEvents="none"`) re-enables pointer
 * events for hover linking; leaders and span lines stay raycast-null,
 * so the badge is the layer's single hoverable surface.
 */
function DimBadge({
  position,
  text,
  onHoverChange,
}: {
  position: THREE.Vector3
  text: string
  onHoverChange?: (hovering: boolean) => void
}) {
  const hoverable = onHoverChange !== undefined
  return (
    <Html
      position={position}
      center
      pointerEvents="none"
      zIndexRange={[100, 0]}
      distanceFactor={HTML_DISTANCE_FACTOR}
    >
      <div
        onPointerEnter={hoverable ? () => onHoverChange(true) : undefined}
        onPointerLeave={hoverable ? () => onHoverChange(false) : undefined}
        className={`px-1.5 py-0.5 text-[10px] font-mono uppercase tracking-wider bg-background/80 border border-border/60 text-foreground backdrop-blur-sm whitespace-nowrap select-none ${
          hoverable
            ? 'pointer-events-auto cursor-default hover:border-foreground/60'
            : 'pointer-events-none'
        }`}
      >
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
 * `standoff` (from {@link computeStandoffs}) translates the WHOLE
 * graphic — span, ticks, arrows, text — perpendicular to the span so
 * stacked dims in one lane don't overlap.
 *
 * Everything here is DISPLAY geometry derived from kernel-provided
 * `anchor` / `direction` / `value` — no value is computed frontend-side.
 */
function LinearDimensionGraphic({
  row,
  standoff,
  onHoverChange,
}: {
  row: DimensionRow
  standoff: THREE.Vector3 | null
  onHoverChange?: (hovering: boolean) => void
}) {
  const geo = useMemo(() => {
    // Router guarantees direction is non-null for linear rows.
    const dir = new THREE.Vector3(...(row.direction as [number, number, number])).normalize()
    const anchor = new THREE.Vector3(...row.anchor)
    if (standoff) anchor.add(standoff)
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
  }, [row, standoff])

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
      <DimBadge
        position={geo.textPos}
        text={displayLabel(row.label)}
        onHoverChange={onHoverChange}
      />
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
 * `emphasized` (hover linking): while a position dim referencing this
 * datum is hovered, the arms render thicker and fully opaque so the
 * eye jumps from the number to its reference.
 *
 * Same depth strategy as the dimension graphics: `depthTest={false}` so
 * the reference marker is always visible, even inside the part corner.
 */
function DatumMarker({
  datum,
  emphasized,
}: {
  datum: DatumDescriptor
  emphasized: boolean
}) {
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
          lineWidth={emphasized ? 3.5 : 2}
          opacity={emphasized ? 1 : 0.9}
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

// ─── Hover face tint ─────────────────────────────────────────────────

// Amber, not the blue selection accent: hover linking must read as
// "this face belongs to that number" without impersonating (or hiding
// under) the SubElementHighlight selection fill, which is the blue
// accent at higher opacity. Same hue family as the Datums origin
// marker (#f1c40f).
const HOVER_TINT_COLOR = '#f1c40f'
// Matches TINT_OFFSET in PartLabels' LabelFaceTints / the picking
// highlight: inflate just proud of the surface so the tint wins the
// depth test without z-fighting.
const HOVER_TINT_OFFSET = 0.002

/**
 * Paints the triangles of the hovered dimension's entity faces, using
 * the SAME per-triangle `faceIds` fan-out as PartLabels'
 * `LabelFaceTints` (and the picking highlight): every triangle whose
 * `faceIds[t]` is one of the row's `entities` belongs to a referenced
 * B-Rep face. Unlike LabelFaceTints we already KNOW the owning object
 * (dimensions are per-part), so there is no cross-solid search — one
 * merged inflated geometry, one draw call.
 *
 * Depth test stays ON (tint = paint on the face, matching the
 * house tint pattern), unlike the always-on-top dimension lines.
 */
function DimensionFaceTints({
  objectId,
  entityIds,
}: {
  objectId: string
  entityIds: number[]
}) {
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)

  const positions = useMemo(() => {
    const obj = objects.get(objectId)
    const faceIds = obj?.mesh.faceIds
    if (!obj || !faceIds) return null
    const mesh = meshRefs.get(objectId)
    const geom = mesh?.geometry
    if (!mesh || !geom) return null
    const posAttr = geom.getAttribute('position') as
      | THREE.BufferAttribute
      | undefined
    if (!posAttr) return null
    const indexAttr = geom.getIndex()
    mesh.updateWorldMatrix(true, false)
    const world = mesh.matrixWorld
    const wanted = new Set(entityIds)

    const tris: number[] = []
    const a = new THREE.Vector3()
    const b = new THREE.Vector3()
    const c = new THREE.Vector3()
    const ab = new THREE.Vector3()
    const ac = new THREE.Vector3()
    const n = new THREE.Vector3()

    for (let t = 0; t < faceIds.length; t++) {
      if (!wanted.has(faceIds[t])) continue
      const i0 = t * 3
      let vi0: number, vi1: number, vi2: number
      if (indexAttr) {
        if (i0 + 2 >= indexAttr.count) continue
        vi0 = indexAttr.getX(i0)
        vi1 = indexAttr.getX(i0 + 1)
        vi2 = indexAttr.getX(i0 + 2)
      } else {
        vi0 = i0
        vi1 = i0 + 1
        vi2 = i0 + 2
      }
      if (
        vi0 >= posAttr.count ||
        vi1 >= posAttr.count ||
        vi2 >= posAttr.count
      ) {
        continue
      }
      a.fromBufferAttribute(posAttr, vi0).applyMatrix4(world)
      b.fromBufferAttribute(posAttr, vi1).applyMatrix4(world)
      c.fromBufferAttribute(posAttr, vi2).applyMatrix4(world)
      ab.subVectors(b, a)
      ac.subVectors(c, a)
      n.crossVectors(ab, ac).normalize()
      tris.push(
        a.x + n.x * HOVER_TINT_OFFSET, a.y + n.y * HOVER_TINT_OFFSET, a.z + n.z * HOVER_TINT_OFFSET,
        b.x + n.x * HOVER_TINT_OFFSET, b.y + n.y * HOVER_TINT_OFFSET, b.z + n.z * HOVER_TINT_OFFSET,
        c.x + n.x * HOVER_TINT_OFFSET, c.y + n.y * HOVER_TINT_OFFSET, c.z + n.z * HOVER_TINT_OFFSET,
      )
    }
    return tris.length > 0 ? new Float32Array(tris) : null
  }, [objectId, entityIds, meshRefs, objects])

  const geometry = useMemo(() => {
    if (!positions) return null
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    g.computeVertexNormals()
    return g
  }, [positions])

  // Hover tints mount/unmount constantly — dispose the GPU buffer on
  // replacement/unmount (CADMesh's discipline, not LabelFaceTints'
  // leak-on-unmount).
  useEffect(() => {
    return () => {
      geometry?.dispose()
    }
  }, [geometry])

  if (!geometry) return null

  return (
    <mesh geometry={geometry} raycast={() => null}>
      <meshBasicMaterial
        color={HOVER_TINT_COLOR}
        side={THREE.DoubleSide}
        transparent
        opacity={0.3}
        depthWrite={false}
      />
    </mesh>
  )
}

// ─── Single annotation (kind router) ─────────────────────────────────

interface DimensionAnnotationProps {
  row: DimensionRow
  standoff: THREE.Vector3 | null
  onHoverChange?: (hovering: boolean) => void
}

/**
 * Route one row to its render form: linear → full dimension graphics;
 * direction-less → bare badge at the anchor; everything else
 * (diameter / radius / angle) → the leader + badge form. Standoffs
 * apply to linear rows only (leader forms have no span to stack).
 */
function DimensionAnnotation({
  row,
  standoff,
  onHoverChange,
}: DimensionAnnotationProps) {
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
    return (
      <LinearDimensionGraphic
        row={row}
        standoff={standoff}
        onHoverChange={onHoverChange}
      />
    )
  }

  if (endVec === null) {
    // No direction to draw with: badge at the anchor, no graphics.
    return (
      <DimBadge
        position={anchorVec}
        text={displayLabel(row.label)}
        onHoverChange={onHoverChange}
      />
    )
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
      <DimBadge
        position={endVec}
        text={displayLabel(row.label)}
        onHoverChange={onHoverChange}
      />
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
 * button is clickable. Same `distanceFactor` as every other annotation
 * in the layer so pins scale with the model too.
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
      distanceFactor={HTML_DISTANCE_FACTOR}
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
