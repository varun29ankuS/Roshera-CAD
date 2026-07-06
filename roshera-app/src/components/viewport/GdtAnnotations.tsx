import { useEffect, useMemo, useState } from 'react'
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
function ObjectGdtAnnotations({ objectId, solidId }: ObjectGdtAnnotationsProps) {
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
          objectId={objectId}
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
 * Map a kernel `characteristic` string to its ISO 1101 Unicode symbol.
 * Falls back to the characteristic name when no symbol is mapped
 * (covering future characteristics gracefully).
 *
 * SOURCE OF TRUTH: geometry-engine/src/gdt/model.rs GeometricCharacteristic::iso_glyph().
 * Every entry here must match the corresponding iso_glyph() arm exactly.
 * When adding a new characteristic, update BOTH this table AND iso_glyph().
 */
const CHARACTERISTIC_SYMBOLS: Record<string, string> = {
  flatness: '⏥',          // ⏥  U+23E5
  perpendicularity: '⊥',  // ⊥  U+22A5
  parallelism: '∥',       // ∥  U+2225
  position: '⌖',          // ⌖  U+2316  (was '⊕' U+2295 — wrong glyph)
  straightness: '⏤',      // ⏤  U+23E4
  circularity: '○',       // ○  U+25CB
  cylindricity: '⌭',      // ⌭  U+232D
  profile_line: '⌒',      // ⌒  U+2312
  profile_surface: '⌓',   // ⌓  U+2313
  angularity: '∠',        // ∠  U+2220
  concentricity: '◎',     // ◎  U+25CE
  symmetry: '≡',          // ≡  U+2261  (was '⌯' U+232F — wrong glyph)
  circular_runout: '⇗',   // ⇗  U+21D7  (was '↗' U+2197 — wrong glyph)
  total_runout: '⟲',      // ⟲  U+27F2  (was '↗↗' — wrong glyph)
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
 * Resolve the badge position for an FCF annotation.
 *
 * Priority order (kernel-computed, no frontend math):
 * 1. `ann.anchor_mm` — a world point ON the toleranced feature, supplied
 *    by the Rust handler from the actual analytic surface. This is the
 *    correct anchor for the badge regardless of datum positions.
 * 2. First live referenced datum's origin (legacy fallback for older servers
 *    that do not supply `anchor_mm`, or for dangling targets whose anchor
 *    could not be resolved).
 * 3. World origin (fully dangling — no live datum and no anchor).
 *
 * No verdict math is performed here; all positions come from kernel fields.
 */
function annotationAnchor(
  ann: GdtAnnotationWire,
  datums: GdtDatumWire[],
): THREE.Vector3 {
  // Prefer the kernel-supplied feature anchor when available.
  if (ann.anchor_mm !== null) {
    return new THREE.Vector3(...ann.anchor_mm)
  }

  // Fallback: first live referenced datum origin, offset so the FCF badge
  // doesn't overlap the datum flag glyph.
  for (const ds of ann.verdict.datum_statuses) {
    const datum = datums.find((d) => d.label === ds.label)
    if (!datum) continue
    const anchor = resolutionToAnchor(datum.resolution)
    if (anchor) {
      return anchor.clone().add(new THREE.Vector3(0, DATUM_STANDOFF_MM * 2, 0))
    }
  }

  // Fully dangling — render at world origin.
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

// ─── GDT face tint constants ──────────────────────────────────────────

// Teal, distinct from the amber used by DimensionFaceTints and the blue
// used by SubElementHighlight. A GDT hover tint must read as "this face
// is the toleranced feature" — a different semantic from "this face
// belongs to a dimension number".
const GDT_TINT_COLOR = '#0891b2'
// Inflate just proud of the surface so the tint wins the depth test
// without z-fighting — same value as DimensionFaceTints / LabelFaceTints.
const GDT_TINT_OFFSET = 0.002

// ─── FcfBadge ────────────────────────────────────────────────────────

/**
 * One FCF badge: a bordered frame text annotation near the toleranced
 * feature, conformance-coloured (green / red / grey). Hover reveals a
 * tooltip panel showing measured vs tolerance + fit residual (all verbatim
 * from the kernel's unit-formatted `*_label` fields) AND tints the
 * toleranced feature's face via the same per-triangle `faceIds[t]` fan-out
 * as `DimensionFaceTints` — using the `target_face_id` returned by the
 * Rust handler.
 *
 * The badge is the only hover target in this group — the tint mesh and
 * any future leader lines are never raycast targets.
 *
 * Dangling annotations (`target_face_id === null`) render the badge but
 * produce no tint on hover — `FcfFaceTint` guards against null face ids.
 */
function FcfBadge({
  annotation,
  datums,
  objectId,
}: {
  annotation: GdtAnnotationWire
  datums: GdtDatumWire[]
  objectId: string
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
      {/* Hover tint: paints the toleranced feature face on hover.
          Mounted/unmounted with `hovered` state — FcfFaceTint disposes
          the GPU buffer on unmount matching DimensionFaceTints' discipline.
          Only rendered when target_face_id is non-null (live feature). */}
      {hovered && annotation.target_face_id !== null && (
        <FcfFaceTint
          objectId={objectId}
          faceId={annotation.target_face_id}
        />
      )}
    </group>
  )
}

// ─── FcfFaceTint ─────────────────────────────────────────────────────

/**
 * Tints the triangles of the hovered FCF's target face using the SAME
 * per-triangle `faceIds[t]` fan-out as `DimensionFaceTints`:
 * - Reads the object's mesh from the scene store (`objects.get(objectId)`).
 * - Reads the mesh ref (GPU geometry) from `meshRefs`.
 * - For every triangle `t` where `faceIds[t] === faceId`, inflates the
 *   vertices 0.002 mm along the face normal and emits them into a tint
 *   mesh.
 *
 * Depth test ON (tint = paint on the face, not x-ray — matching the
 * house tint pattern). Disposes the BufferGeometry on unmount/replacement
 * following CADMesh's discipline (matching DimensionFaceTints).
 */
function FcfFaceTint({
  objectId,
  faceId,
}: {
  objectId: string
  faceId: number
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

    const tris: number[] = []
    const a = new THREE.Vector3()
    const b = new THREE.Vector3()
    const c = new THREE.Vector3()
    const ab = new THREE.Vector3()
    const ac = new THREE.Vector3()
    const n = new THREE.Vector3()

    for (let t = 0; t < faceIds.length; t++) {
      if (faceIds[t] !== faceId) continue
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
        a.x + n.x * GDT_TINT_OFFSET,
        a.y + n.y * GDT_TINT_OFFSET,
        a.z + n.z * GDT_TINT_OFFSET,
        b.x + n.x * GDT_TINT_OFFSET,
        b.y + n.y * GDT_TINT_OFFSET,
        b.z + n.z * GDT_TINT_OFFSET,
        c.x + n.x * GDT_TINT_OFFSET,
        c.y + n.y * GDT_TINT_OFFSET,
        c.z + n.z * GDT_TINT_OFFSET,
      )
    }
    return tris.length > 0 ? new Float32Array(tris) : null
  }, [objectId, faceId, meshRefs, objects])

  const geometry = useMemo(() => {
    if (!positions) return null
    const g = new THREE.BufferGeometry()
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    g.computeVertexNormals()
    return g
  }, [positions])

  // Dispose GPU buffer on replacement/unmount — matching DimensionFaceTints.
  useEffect(() => {
    return () => {
      geometry?.dispose()
    }
  }, [geometry])

  if (!geometry) return null

  return (
    <mesh geometry={geometry} raycast={() => null}>
      <meshBasicMaterial
        color={GDT_TINT_COLOR}
        side={THREE.DoubleSide}
        transparent
        opacity={0.3}
        depthWrite={false}
      />
    </mesh>
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
