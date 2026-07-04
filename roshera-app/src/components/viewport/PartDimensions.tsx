import { useMemo } from 'react'
import { Html, Line } from '@react-three/drei'
import * as THREE from 'three'
import { X } from 'lucide-react'
import { useSceneStore, type PinnedMeasurement } from '@/stores/scene-store'
import { usePartDimensions, type DimensionRow } from './use-part-dimensions'
import { usePinnedMeasurementsRevalidation } from './use-pinned-measurements'

/**
 * Live kernel dimension annotations for every object whose id is in
 * `showDimensions`, plus the user's pinned interactive measurements.
 * Renders inside the R3F `<Canvas>` scene graph so all positions track
 * camera orbit / pan / zoom automatically via drei's `<Html>` and
 * `<Line>`.
 *
 * ## Layout
 * Each `DimensionRow` renders as:
 *   - **Non-extent rows** (feature dimensions — diameter, length, angle,
 *     radius): a short `<Line>` leader from `row.anchor` to a label
 *     endpoint offset ~6 mm perpendicularly from `row.direction`, then a
 *     `<Html>` billboard showing `row.label` at the leader tip.
 *   - **Extent rows** (whole-part bounding-box extents): a `<Html>`
 *     billboard at `row.anchor` directly — no leader. Extents span the
 *     whole solid so a leader from one anchor would just clutter the scene.
 *
 * ## Perpendicular offset choice
 * The leader endpoint is `anchor + 6mm * perp(direction)` where `perp`
 * picks the stable perpendicular via the classic "cross with world-Y;
 * fall back to world-X when direction is near-parallel to Y" approach.
 * This produces the same perp as engineering drawings draw witness lines
 * — consistent across orbit because it is computed from the kernel
 * direction vector, not from the screen.
 *
 * ## Visual language
 * Styling mirrors `SketchOverlay.tsx`'s `DimLabel` function (which is
 * file-local and not exported). A local `DimBadge` replicates the same
 * CSS class string exactly so the annotation palette is identical. The
 * `<Line>` color uses the app's `--cad-tick` CSS variable via its hex
 * equivalent, matching the neutral annotation tones used by `PartLabels`.
 *
 * ## pointerEvents
 * Ambient annotations are read-only (`pointerEvents="none"` on `<Html>`,
 * `raycast={() => null}` on `<Line>`) so this layer never intercepts
 * orbit, selection, or sketch clicks. Pinned measurements are the one
 * exception: their ✕ dismiss button re-enables pointer events on JUST
 * that button (CSS `pointer-events: auto` inside a `pointer-events:
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
 * Fetches the dimension table for one solid and renders its rows.
 * Isolated so each toggled object has its own fetch lifecycle and
 * loading state, without coupling the render of one solid to the
 * in-flight request of another.
 */
function ObjectDimensions({ objectId, solidId }: ObjectDimensionsProps) {
  const { dimensions } = usePartDimensions(solidId, true)

  if (dimensions.length === 0) return null

  return (
    <group name={`part-dimensions-${objectId}`}>
      {dimensions.map((row) => (
        <DimensionAnnotation key={row.id} row={row} />
      ))}
    </group>
  )
}

// ─── Leader offset computation ────────────────────────────────────────

/**
 * Derive a stable perpendicular to `dir` for positioning the leader
 * endpoint. Uses the cross-product approach: cross with world-Y and
 * normalise. When `dir` is near-parallel to Y (|dot| > 0.9), fall back
 * to crossing with world-X. This gives a single deterministic result
 * that doesn't depend on camera orientation.
 *
 * 6 mm offset: matches the witness-line gap used in engineering drawing
 * templates and is visible at typical model scales without obscuring
 * the feature.
 */
const LEADER_OFFSET_MM = 6

function leaderEndpoint(
  anchor: THREE.Vector3,
  direction: THREE.Vector3,
): THREE.Vector3 {
  const worldY = new THREE.Vector3(0, 1, 0)
  const worldX = new THREE.Vector3(1, 0, 0)
  const ref =
    Math.abs(direction.dot(worldY)) > 0.9 ? worldX : worldY
  const perp = new THREE.Vector3()
    .crossVectors(direction, ref)
    .normalize()
  return anchor.clone().addScaledVector(perp, LEADER_OFFSET_MM)
}

// ─── Extent kind check ───────────────────────────────────────────────

/**
 * Returns true for whole-part extent rows. Extents span the full solid
 * bounding box, so a short feature leader adds noise rather than
 * clarity. They render at their anchor directly, no leader.
 *
 * Kernel kind string for extents: `"extent"` (readable/dimensions.rs).
 */
function isExtent(kind: string): boolean {
  return kind === 'extent'
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

// ─── Single annotation ───────────────────────────────────────────────

// Annotation tint: a neutral blueprint grey matching the --cad-tick
// CSS variable used by CADMesh edge overlays and the sketch plane
// border. Hardcoded as a constant so the component tree stays free of
// runtime CSS-variable lookups (which would need a ref to the canvas
// DOM element). This is the same approach PartLabels uses for its
// neutral chip tone.
const LEADER_COLOR = '#4a5568'

interface DimensionAnnotationProps {
  row: DimensionRow
}

function DimensionAnnotation({ row }: DimensionAnnotationProps) {
  // Memoised per row: the row object identity only changes when a fresh
  // fetch lands, so the Vector3s (anchor, leader endpoint) are computed
  // once per fetch instead of on every render of every annotation.
  const { anchorVec, endVec } = useMemo(() => {
    const anchor = new THREE.Vector3(...row.anchor)
    if (isExtent(row.kind) || row.direction === null) {
      return { anchorVec: anchor, endVec: null }
    }
    const dir = new THREE.Vector3(...row.direction)
    return { anchorVec: anchor, endVec: leaderEndpoint(anchor, dir) }
  }, [row])

  if (endVec === null) {
    // Extents: billboard directly at the anchor, no leader.
    return <DimBadge position={anchorVec} text={row.label} />
  }

  // Feature dimensions: leader from anchor to offset endpoint. The
  // leader must never be a raycast target — `raycast={() => null}`
  // removes it from Three.js hit-testing entirely so orbit / selection
  // clicks pass straight through.
  return (
    <>
      <Line
        points={[anchorVec, endVec]}
        color={LEADER_COLOR}
        lineWidth={1}
        opacity={0.7}
        transparent
        raycast={() => null}
      />
      <DimBadge position={endVec} text={row.label} />
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
