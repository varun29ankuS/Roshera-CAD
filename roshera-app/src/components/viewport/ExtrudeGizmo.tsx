import { useEffect, useMemo, useRef, useState } from 'react'
import * as THREE from 'three'
import { Html } from '@react-three/drei'
import { useThree, type ThreeEvent } from '@react-three/fiber'
import { useSceneStore, type CADObject } from '@/stores/scene-store'

/**
 * Fusion-style face-pull gizmo. Visible whenever exactly one face is
 * selected (sub-element selection mode `face`). Renders an arrow at the
 * face centroid pointing along the average face normal; dragging the
 * arrow translates that face along its normal and, on release, fires
 * `POST /api/geometry/face/extrude`. The backend remains the single
 * source of truth — the kernel mutates topology, then broadcasts
 * `ObjectDeleted` + `ObjectCreated` which drive the local scene store.
 *
 * Display-only inputs:
 *   - `meshRefs.get(objectId)` — the live Three.js mesh (for matrixWorld
 *     and current vertex positions)
 *   - `objects.get(objectId).mesh.faceIds` — the per-triangle FaceId map
 *     shipped on every broadcast frame; selecting a face fans out to
 *     every triangle whose id matches.
 *
 * Drag math:
 *   For each pointermove, raycast the mouse into a plane through the
 *   gizmo origin whose normal is `axis × camDir × axis` (the in-plane
 *   direction perpendicular to the axis but most parallel to the
 *   camera's view direction). Project intersection − origin onto the
 *   axis to read a signed scalar; subtract the pointerdown reference
 *   to get a delta. This gives stable axis-aligned drag regardless of
 *   camera roll, and degrades gracefully (handler returns) when the
 *   axis is nearly parallel to the view direction.
 */
export function ExtrudeGizmo() {
  const subElementSelections = useSceneStore((s) => s.subElementSelections)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const meshRefs = useSceneStore((s) => s.meshRefs)
  const objects = useSceneStore((s) => s.objects)
  const setGizmoDragging = useSceneStore((s) => s.setGizmoDragging)
  const { camera, gl } = useThree()

  // Visible only in face mode with exactly one face selection. Other
  // sub-element types (edge / vertex) and multi-face selections fall
  // through to nothing — the kernel's face-pull operation is a single-
  // face primitive.
  const sel = useMemo(() => {
    if (selectionMode !== 'face') return null
    const faces = subElementSelections.filter((s) => s.type === 'face')
    return faces.length === 1 ? faces[0] : null
  }, [selectionMode, subElementSelections])

  // Centroid + average-normal of every triangle whose `faceIds[t]`
  // matches the selected face. Recomputed when the selection or the
  // underlying mesh changes (a broadcast may rebuild the geometry; the
  // surrounding `objects` map reference flips and triggers refresh).
  const transform = useMemo(() => {
    if (!sel) return null
    const mesh = meshRefs.get(sel.objectId)
    if (!mesh) return null
    const obj: CADObject | undefined = objects.get(sel.objectId)
    const faceIds = obj?.mesh.faceIds
    if (!faceIds) return null

    const positions = mesh.geometry.getAttribute('position') as
      | THREE.BufferAttribute
      | undefined
    if (!positions) return null
    const indexAttr = mesh.geometry.getIndex()

    const centroid = new THREE.Vector3()
    const accumNormal = new THREE.Vector3()
    let triCount = 0

    for (let t = 0; t < faceIds.length; t++) {
      if (faceIds[t] !== sel.index) continue
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
      const a = new THREE.Vector3()
        .fromBufferAttribute(positions, vi0)
        .applyMatrix4(mesh.matrixWorld)
      const b = new THREE.Vector3()
        .fromBufferAttribute(positions, vi1)
        .applyMatrix4(mesh.matrixWorld)
      const c = new THREE.Vector3()
        .fromBufferAttribute(positions, vi2)
        .applyMatrix4(mesh.matrixWorld)
      centroid.add(a).add(b).add(c)
      const ab = new THREE.Vector3().subVectors(b, a)
      const ac = new THREE.Vector3().subVectors(c, a)
      // Sum unweighted face normals (cross product magnitude doubles
      // as triangle area, so this is area-weighted averaging — large
      // triangles dominate, robust to skinny near-zero triangles at
      // the edges of the face).
      accumNormal.add(new THREE.Vector3().crossVectors(ab, ac))
      triCount++
    }
    if (triCount === 0) return null
    centroid.multiplyScalar(1 / (triCount * 3))
    accumNormal.normalize()
    if (!Number.isFinite(accumNormal.x) || accumNormal.lengthSq() < 1e-6) {
      return null
    }
    return { origin: centroid, axis: accumNormal }
  }, [sel, meshRefs, objects])

  const [active, setActive] = useState(false)
  // Two independent hover states so each arrow's tint reflects only
  // its own pointer state. Avoids a single-arrow look when the user
  // hovers one direction while the other stays cool.
  const [hoveredPos, setHoveredPos] = useState(false)
  const [hoveredNeg, setHoveredNeg] = useState(false)
  const [distance, setDistance] = useState(0)
  // Direction the user grabbed at pointerdown. 1 = +axis, -1 = -axis.
  // Drives which arrow is rendered while a drag is in flight, and
  // signs the distance submitted to the backend (the REST endpoint
  // takes a signed scalar along the face's original normal).
  const [pickedDirection, setPickedDirection] = useState<1 | -1 | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const dragRef = useRef<{
    refDot: number
    objectId: string
    faceId: number
    origin: THREE.Vector3
    axis: THREE.Vector3
    direction: 1 | -1
    currentDistance: number
  } | null>(null)

  // Reset live distance whenever the selection target changes — the
  // gizmo always starts at zero relative offset for the currently-
  // selected face.
  useEffect(() => {
    setDistance(0)
    setPickedDirection(null)
    dragRef.current = null
    setActive(false)
  }, [sel?.objectId, sel?.index])

  // Window-level pointer listeners while dragging. Three-fiber's event
  // system would lose the pointer once it leaves the arrow geometry;
  // window scope guarantees we keep getting moves until pointerup.
  useEffect(() => {
    if (!active) return

    const ndc = new THREE.Vector2()
    const raycaster = new THREE.Raycaster()
    const plane = new THREE.Plane()
    const planeNormal = new THREE.Vector3()
    const camDir = new THREE.Vector3()
    const intersection = new THREE.Vector3()
    const fromOrigin = new THREE.Vector3()

    const onMove = (e: PointerEvent) => {
      const drag = dragRef.current
      if (!drag) return
      const rect = gl.domElement.getBoundingClientRect()
      ndc.x = ((e.clientX - rect.left) / rect.width) * 2 - 1
      ndc.y = -((e.clientY - rect.top) / rect.height) * 2 + 1
      raycaster.setFromCamera(ndc, camera)

      camera.getWorldDirection(camDir)
      // Plane normal = the in-plane direction perpendicular to the
      // axis whose other in-plane vector is closest to the camera
      // view direction. `(axis × camDir) × axis` lies in the axis
      // plane and is perpendicular to the axis. If axis ∥ camDir the
      // first cross collapses to zero — bail rather than divide by
      // a degenerate length.
      planeNormal.crossVectors(drag.axis, camDir)
      if (planeNormal.lengthSq() < 1e-6) return
      planeNormal.cross(drag.axis).normalize()
      if (!Number.isFinite(planeNormal.x) || planeNormal.lengthSq() < 1e-6) {
        return
      }
      plane.setFromNormalAndCoplanarPoint(planeNormal, drag.origin)

      if (!raycaster.ray.intersectPlane(plane, intersection)) return
      fromOrigin.subVectors(intersection, drag.origin)
      const dot = fromOrigin.dot(drag.axis)
      const next = dot - drag.refDot
      drag.currentDistance = next
      setDistance(next)
    }

    const onUp = () => {
      const drag = dragRef.current
      dragRef.current = null
      setActive(false)
      setPickedDirection(null)
      // Release orbit-rotate suppression — this gesture is over.
      setGizmoDragging(false)
      if (!drag) {
        setDistance(0)
        return
      }
      // Distance is already signed along the face's +axis (positive
      // toward `transform.axis`, negative away). The REST API takes
      // the same signed scalar — `pickedDirection` only affects which
      // arrow rendered during the drag, not the wire payload.
      const finalDistance = drag.currentDistance
      // Sub-tolerance moves are treated as a click-with-no-pull and
      // don't fire the REST call — avoids "I tapped the arrow and got
      // a no-op solid".
      if (!Number.isFinite(finalDistance) || Math.abs(finalDistance) < 1e-6) {
        setDistance(0)
        return
      }

      void submitExtrude(drag.objectId, drag.faceId, finalDistance)
    }

    const submitExtrude = async (
      objectId: string,
      faceId: number,
      finalDistance: number,
    ) => {
      setSubmitting(true)
      try {
        const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
        const resp = await fetch(`${API_BASE}/geometry/face/extrude`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            object_uuid: objectId,
            face_id: faceId,
            distance: finalDistance,
          }),
        })
        if (!resp.ok) {
          const err = await resp.json().catch(() => ({}))
          // eslint-disable-next-line no-console
          console.error(
            '[ExtrudeGizmo] face/extrude failed:',
            resp.status,
            err?.message || err?.error || resp.statusText,
          )
        }
        // Backend broadcasts ObjectDeleted + ObjectCreated; the local
        // scene store reconciles via ws-bridge. The selection is
        // cleared because the host UUID is retired — and a fresh face
        // selection has to be made on the new solid.
      } catch (err) {
        // eslint-disable-next-line no-console
        console.error('[ExtrudeGizmo] face/extrude network error:', err)
      } finally {
        setSubmitting(false)
        setDistance(0)
      }
    }

    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    return () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
    }
  }, [active, camera, gl, setGizmoDragging])

  // Quaternion that aligns the cone/cylinder default +Y axis to the
  // face normal. Computed unconditionally so the hook order stays
  // stable even when `transform` is null.
  const orientation = useMemo(() => {
    const q = new THREE.Quaternion()
    if (transform) {
      q.setFromUnitVectors(new THREE.Vector3(0, 1, 0), transform.axis)
    }
    return q
  }, [transform])

  if (!sel || !transform) return null

  const handlePointerDown =
    (direction: 1 | -1) => (e: ThreeEvent<PointerEvent>) => {
      if (!sel || !transform) return
      e.stopPropagation()
      const rect = gl.domElement.getBoundingClientRect()
      const ndc = new THREE.Vector2(
        ((e.clientX - rect.left) / rect.width) * 2 - 1,
        -((e.clientY - rect.top) / rect.height) * 2 + 1,
      )
      const raycaster = new THREE.Raycaster()
      raycaster.setFromCamera(ndc, camera)

      const camDir = new THREE.Vector3()
      camera.getWorldDirection(camDir)
      const planeNormal = new THREE.Vector3()
        .crossVectors(transform.axis, camDir)
      if (planeNormal.lengthSq() < 1e-6) return
      planeNormal.cross(transform.axis).normalize()
      if (!Number.isFinite(planeNormal.x) || planeNormal.lengthSq() < 1e-6) {
        return
      }
      const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(
        planeNormal,
        transform.origin,
      )
      const intersection = new THREE.Vector3()
      if (!raycaster.ray.intersectPlane(plane, intersection)) return
      const refDot = new THREE.Vector3()
        .subVectors(intersection, transform.origin)
        .dot(transform.axis)

      dragRef.current = {
        refDot,
        objectId: sel.objectId,
        faceId: sel.index,
        origin: transform.origin.clone(),
        axis: transform.axis.clone(),
        direction,
        currentDistance: 0,
      }
      setDistance(0)
      setPickedDirection(direction)
      setActive(true)
      // Suppress orbit-rotate for the duration of the drag — the
      // canvas would otherwise also rotate the camera on this very
      // pointerdown gesture (OrbitControls runs on raw DOM events).
      setGizmoDragging(true)
    }

  // Visible arrow geometry. Shaft length grows with the live drag
  // distance so the user sees how far they've pulled — capped to a
  // sensible minimum so the arrow remains grabbable at zero distance.
  const SHAFT_BASE = 1.2
  const SHAFT_RADIUS = 0.04
  const HEAD_LENGTH = 0.35
  const HEAD_RADIUS = 0.12
  const HIT_RADIUS = 0.25

  // `distance` is signed along `transform.axis`. Each arrow grows in
  // its own direction — the +arrow grows when distance > 0, the
  // -arrow grows when distance < 0 — so the visual unambiguously
  // reflects which way the face is moving regardless of which arrow
  // the user grabbed.
  const posShaft = Math.max(SHAFT_BASE, SHAFT_BASE + Math.max(0, distance))
  const negShaft = Math.max(SHAFT_BASE, SHAFT_BASE + Math.max(0, -distance))

  // While idle, both arrows render so the user can pick a direction.
  // While dragging, only the picked arrow renders so the opposite
  // direction doesn't visually contradict the live drag.
  const showPos = !active || pickedDirection === 1
  const showNeg = !active || pickedDirection === -1

  const tintColor = (hovered: boolean, isPicked: boolean) =>
    submitting
      ? '#888888'
      : (active && isPicked) || hovered
        ? '#ffaa00'
        : '#ff8800'

  const showReadout = active || Math.abs(distance) > 1e-4

  // Tip offset for the readout — anchor on whichever arrow currently
  // dominates so the readout follows the active drag.
  const readoutOffset =
    pickedDirection === -1
      ? -(negShaft + HEAD_LENGTH + 0.15)
      : posShaft + HEAD_LENGTH + 0.15

  return (
    <group position={transform.origin} quaternion={orientation}>
      {/* +axis arrow */}
      {showPos && (
        <Arrow
          color={tintColor(hoveredPos, pickedDirection === 1)}
          shaftLength={posShaft}
          shaftRadius={SHAFT_RADIUS}
          headLength={HEAD_LENGTH}
          headRadius={HEAD_RADIUS}
          hitRadius={HIT_RADIUS}
          direction={1}
          onPointerDown={handlePointerDown(1)}
          onHoverChange={(h) => {
            setHoveredPos(h)
            if (h) gl.domElement.style.cursor = 'grab'
            else if (!active) gl.domElement.style.cursor = ''
          }}
        />
      )}

      {/* -axis arrow */}
      {showNeg && (
        <Arrow
          color={tintColor(hoveredNeg, pickedDirection === -1)}
          shaftLength={negShaft}
          shaftRadius={SHAFT_RADIUS}
          headLength={HEAD_LENGTH}
          headRadius={HEAD_RADIUS}
          hitRadius={HIT_RADIUS}
          direction={-1}
          onPointerDown={handlePointerDown(-1)}
          onHoverChange={(h) => {
            setHoveredNeg(h)
            if (h) gl.domElement.style.cursor = 'grab'
            else if (!active) gl.domElement.style.cursor = ''
          }}
        />
      )}

      {/* Live distance readout — appears at the active arrow tip
          during drag (or whenever the user has dragged any non-zero
          amount). HTML overlay rather than three Text so it picks
          up the existing panel CSS. */}
      {showReadout && (
        <Html
          position={[0, readoutOffset, 0]}
          center
          style={{ pointerEvents: 'none' }}
        >
          <div className="cad-panel cad-readout px-2 py-1 text-[10px] uppercase tracking-wider whitespace-nowrap">
            <span className="text-muted-foreground mr-2">PULL</span>
            <span className="text-foreground tabular-nums">
              {distance >= 0 ? '+' : ''}
              {distance.toFixed(3)}
            </span>
          </div>
        </Html>
      )}
    </group>
  )
}

/**
 * One arm of the bidirectional face-pull gizmo. Local +Y is the
 * outward direction; the `direction` prop flips the entire arm to
 * the -axis half-space without re-deriving the outer group's
 * orientation. Visible cylinder + cone use `depthTest={false}` so
 * the gizmo always shows; the fat invisible hit volume keeps
 * default depth-testing so it doesn't intercept picks that should
 * land on the underlying solid behind it.
 */
function Arrow(props: {
  color: string
  shaftLength: number
  shaftRadius: number
  headLength: number
  headRadius: number
  hitRadius: number
  direction: 1 | -1
  onPointerDown: (e: ThreeEvent<PointerEvent>) => void
  onHoverChange: (hovered: boolean) => void
}) {
  const {
    color,
    shaftLength,
    shaftRadius,
    headLength,
    headRadius,
    hitRadius,
    direction,
    onPointerDown,
    onHoverChange,
  } = props
  const sign = direction
  const shaftCenterOffset = (shaftLength / 2) * sign
  const headOffset = (shaftLength + headLength / 2) * sign
  const tipOffset = (shaftLength + headLength) * sign
  // Head must point outward — the cone's local +Y axis has to flip
  // for the negative arm. Rotating π around X swaps +Y → -Y.
  const headRotation: [number, number, number] =
    sign === 1 ? [0, 0, 0] : [Math.PI, 0, 0]
  return (
    <group>
      <mesh position={[0, shaftCenterOffset, 0]}>
        <cylinderGeometry
          args={[shaftRadius, shaftRadius, shaftLength, 12]}
        />
        <meshBasicMaterial
          color={color}
          depthTest={false}
          transparent
          opacity={0.95}
        />
      </mesh>
      <mesh position={[0, headOffset, 0]} rotation={headRotation}>
        <coneGeometry args={[headRadius, headLength, 16]} />
        <meshBasicMaterial
          color={color}
          depthTest={false}
          transparent
          opacity={0.95}
        />
      </mesh>
      <mesh
        position={[0, tipOffset / 2, 0]}
        onPointerDown={onPointerDown}
        onPointerOver={(e) => {
          e.stopPropagation()
          onHoverChange(true)
        }}
        onPointerOut={() => onHoverChange(false)}
      >
        <cylinderGeometry
          args={[hitRadius, hitRadius, Math.abs(tipOffset), 12]}
        />
        <meshBasicMaterial visible={false} />
      </mesh>
    </group>
  )
}
