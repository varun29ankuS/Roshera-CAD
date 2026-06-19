import { useMemo } from 'react'
import { Html } from '@react-three/drei'
import * as THREE from 'three'
import { Tag, AlertTriangle, Check, X } from 'lucide-react'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { usePartLabels } from './use-part-labels'
import { labelText, type Label } from '@/lib/labels-api'

/**
 * LABELLER overlay — renders each of the active part's labels as a
 * billboard callout anchored at its world point, so the USER can read
 * the part's named features (and frame questions around them) the same
 * way the agent can.
 *
 * Lives inside the R3F `<Canvas>` and uses drei's `<Html>` (the overlay
 * primitive already used by `SketchOverlay`), so each callout tracks its
 * world anchor through orbit / pan / zoom without us re-projecting by
 * hand.
 *
 * ## Colour-coded, dimensioned, conformance-aware
 * Each label carries a deterministic `color`, an optional verified
 * `measurement` (units-bearing `display` string), and an optional spec
 * `conformance` verdict. The chip is painted in the label's colour
 * (border + accent + leader), reads `name — measurement.display`, and
 * appends a ✓ (green, in-spec) / ✗ (red, out-of-spec) badge. The SAME
 * colour tints the labelled entity's faces in the scene (see
 * {@link LabelFaceTints}) so the user can match each callout to its
 * feature. Older backends that omit colour/measurement fall back to a
 * neutral chip with just the name.
 *
 * ## Verified vs stale
 * A label whose named entity still resolves carries a world `anchor` and
 * renders normally. A stale label (entity deleted/regenerated away —
 * `stale === true` or `anchor === null`) renders amber, struck-through,
 * and pinned at the world origin with a warning glyph, never silently
 * dropped — but still shows its colour swatch so the user can place it.
 *
 * Mounts only when the labels overlay is toggled on (`labelsVisible`).
 */
export function PartLabels() {
  const labelsVisible = useSceneStore((s) => s.labelsVisible)
  const { labels } = usePartLabels(labelsVisible)

  if (!labelsVisible) return null

  // Anchored (verified) labels billboard at their world point; stale
  // ones have no world point, so they are collected into a single
  // origin-pinned stack rather than overlapping at [0,0,0].
  const anchored = labels.filter(
    (l): l is Label & { anchor: [number, number, number] } =>
      l.anchor !== null,
  )
  const stale = labels.filter((l) => l.anchor === null)

  return (
    <group name="part-labels">
      <LabelFaceTints labels={labels} />
      {anchored.map((label) => (
        <Html
          key={label.name}
          position={label.anchor}
          center
          pointerEvents="none"
          zIndexRange={[90, 0]}
        >
          <LabelChip label={label} />
        </Html>
      ))}
      {stale.length > 0 && (
        <Html
          position={[0, 0, 0]}
          center
          pointerEvents="none"
          zIndexRange={[90, 0]}
        >
          <div className="flex flex-col items-center gap-0.5 select-none">
            {stale.map((label) => (
              <LabelChip key={label.name} label={label} />
            ))}
          </div>
        </Html>
      )}
    </group>
  )
}

/**
 * Conformance badge: in-spec → green ✓, out-of-spec → red ✗,
 * not-verified / absent → no badge. Inline-coloured so it reads against
 * any chip background.
 */
function ConformanceBadge({ label }: { label: Label }) {
  if (label.conformance === 'in_spec') {
    return (
      <Check
        className="w-3 h-3 shrink-0 text-emerald-400"
        aria-label="in spec"
      />
    )
  }
  if (label.conformance === 'out_of_spec') {
    return (
      <X className="w-3 h-3 shrink-0 text-red-400" aria-label="out of spec" />
    )
  }
  return null
}

/**
 * One callout chip. When the kernel ships a `color`, the chip border,
 * glyph, and a leading swatch are painted in it (inline style — the
 * colour is data, not a Tailwind token) so the user can match the chip
 * to the same-coloured face tint in the scene. Stale labels switch to
 * the amber warning treatment with a struck-through name, but still show
 * their colour swatch.
 */
function LabelChip({ label }: { label: Label }) {
  const Icon = label.stale ? AlertTriangle : Tag
  const text = labelText(label)

  // Stale chips keep the amber semantic so "this no longer resolves"
  // reads at a glance; the colour swatch still anchors which feature it
  // was. Live chips take the label colour on border + glyph when present,
  // else the neutral blueprint tone (older backend).
  const staleTone = 'border-amber-400/60 text-amber-300 bg-amber-950/50'
  const neutralTone = 'border-border/70 text-foreground bg-background/85'
  const colored = !label.stale && label.color !== null
  const tone = label.stale ? staleTone : colored ? '' : neutralTone

  const style = colored
    ? {
        borderColor: label.color as string,
        color: label.color as string,
      }
    : undefined

  const title = label.stale
    ? `${label.name} — stale: the ${label.kind} this name pinned no longer exists`
    : [
        label.measurement ? `${label.name}: ${label.measurement.display}` : label.name,
        label.conformance === 'in_spec'
          ? 'in spec'
          : label.conformance === 'out_of_spec'
            ? 'out of spec'
            : null,
        label.description,
      ]
        .filter(Boolean)
        .join(' — ')

  return (
    <div
      title={title}
      style={style}
      className={`inline-flex items-center gap-1 px-1.5 h-5 whitespace-nowrap text-[10px] font-medium tracking-tight border ${tone} bg-background/85 backdrop-blur-sm shadow-sm`}
    >
      {label.color !== null && (
        // Colour swatch: a small square in the label colour. Present even
        // on stale chips (and when the glyph/border take amber) so the
        // user can always match a chip to its same-coloured face tint.
        <span
          aria-hidden
          className="w-2 h-2 shrink-0 rounded-[1px] border border-black/20"
          style={{ backgroundColor: label.color }}
        />
      )}
      <Icon className="w-3 h-3 shrink-0" aria-hidden />
      <span className={label.stale ? 'line-through decoration-amber-400/70' : ''}>
        {text}
      </span>
      <ConformanceBadge label={label} />
    </div>
  )
}

/**
 * In-scene face tint: paints the triangles of each label's target FACE in
 * the label's colour, so the user can see which mark is which feature.
 *
 * Reuses the EXACT face-id → triangle fan-out the picking highlight uses
 * (`SubElementHighlight`): every triangle whose per-triangle `faceIds[t]`
 * equals the label's kernel `entityId` belongs to that B-Rep face. The
 * matched triangles are merged into one inflated `BufferGeometry` (one
 * draw call per label) drawn just proud of the surface so it wins the
 * depth test without z-fighting.
 *
 * Only `kind === 'face'` labels with a resolved `entityId`, a `color`,
 * and a live (non-stale) entity tint. Edge / vertex / section labels —
 * and any label the backend ships without a colour or entity id — fall
 * back to the colour-matched chip + swatch alone (the documented
 * fallback): no tint is drawn, nothing breaks.
 */
function LabelFaceTints({ labels }: { labels: Label[] }) {
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)

  // The viewport runs one active document, so the part's labelled faces
  // live across the visible solids. We don't get an object id on the
  // label, so we search every solid's faceIds for the entity id — the
  // kernel FaceId is unique per part, so at most one solid matches.
  const tints = useMemo(() => {
    const faceLabels = labels.filter(
      (l) => l.kind === 'face' && l.entityId !== null && l.color !== null && !l.stale,
    )
    if (faceLabels.length === 0) return []

    const out: Array<{ key: string; color: string; positions: Float32Array }> = []

    for (const label of faceLabels) {
      const entityId = label.entityId as number
      // Find the solid that owns this face id.
      let owner: { obj: CADObject; faceIds: Uint32Array } | null = null
      for (const id of objectOrder) {
        const obj = objects.get(id)
        const faceIds = obj?.mesh.faceIds
        if (obj && faceIds && faceIds.includes(entityId)) {
          owner = { obj, faceIds }
          break
        }
      }
      if (!owner) continue

      const mesh = meshRefs.get(owner.obj.id)
      const geom = mesh?.geometry
      if (!mesh || !geom) continue
      const positions = geom.getAttribute('position') as
        | THREE.BufferAttribute
        | undefined
      if (!positions) continue
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

      const faceIds = owner.faceIds
      for (let t = 0; t < faceIds.length; t++) {
        if (faceIds[t] !== entityId) continue
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
          vi0 >= positions.count ||
          vi1 >= positions.count ||
          vi2 >= positions.count
        ) {
          continue
        }
        a.fromBufferAttribute(positions, vi0).applyMatrix4(world)
        b.fromBufferAttribute(positions, vi1).applyMatrix4(world)
        c.fromBufferAttribute(positions, vi2).applyMatrix4(world)
        ab.subVectors(b, a)
        ac.subVectors(c, a)
        n.crossVectors(ab, ac).normalize()
        // Inflate each vertex along the triangle normal so the tint sits
        // just proud of the surface (matches SubElementHighlight's 0.002).
        tris.push(
          a.x + n.x * TINT_OFFSET, a.y + n.y * TINT_OFFSET, a.z + n.z * TINT_OFFSET,
          b.x + n.x * TINT_OFFSET, b.y + n.y * TINT_OFFSET, b.z + n.z * TINT_OFFSET,
          c.x + n.x * TINT_OFFSET, c.y + n.y * TINT_OFFSET, c.z + n.z * TINT_OFFSET,
        )
      }
      if (tris.length === 0) continue
      out.push({
        key: `${label.name}:${owner.obj.id}:${entityId}`,
        color: label.color as string,
        positions: new Float32Array(tris),
      })
    }
    return out
    // `objects`/`meshRefs` map identities change on every WS frame; the
    // geometryKey inside usePartLabels already gates refetch, and React
    // memoises by these references — recomputing the tint on a frame that
    // changed the mesh is correct (the world positions moved).
  }, [labels, meshRefs, objects, objectOrder])

  if (tints.length === 0) return null

  return (
    <group name="part-label-face-tints">
      {tints.map((t) => (
        <FaceTintMesh key={t.key} color={t.color} positions={t.positions} />
      ))}
    </group>
  )
}

const TINT_OFFSET = 0.002

function FaceTintMesh({
  color,
  positions,
}: {
  color: string
  positions: Float32Array
}) {
  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry()
    geom.setAttribute('position', new THREE.BufferAttribute(positions, 3))
    geom.computeVertexNormals()
    return geom
  }, [positions])

  return (
    <mesh geometry={geometry}>
      <meshBasicMaterial
        color={color}
        side={THREE.DoubleSide}
        transparent
        opacity={0.35}
        depthWrite={false}
      />
    </mesh>
  )
}
