import { useEffect, useMemo, useRef, useState } from 'react'
import * as THREE from 'three'
import { Html } from '@react-three/drei'
import { useThree, type ThreeEvent } from '@react-three/fiber'
import { useSceneStore, type CADObject } from '@/stores/scene-store'

/**
 * Direct-modeling face-pull gizmo. Visible whenever exactly one face is
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
 *
 * Increment behaviour:
 *   Drag distance snaps to `EXTRUDE_STEP` (0.1 model units). This is the
 *   on-screen UX increment; the REST payload carries the snapped value
 *   verbatim. Sub-step jitter is removed at the drag layer so the readout,
 *   the arrow shaft length, and the eventual committed extrusion all agree.
 *
 * Live ghost preview:
 *   During drag, pointermoves schedule a debounced
 *   `POST /api/geometry/face/extrude/preview` call (50 ms trailing
 *   debounce). The backend runs the kernel face-pull against a
 *   ModelSnapshot, tessellates with realtime params, restores, and
 *   returns the mesh — leaving the model untouched. The response is
 *   rendered as a translucent ghost overlay so the user sees the
 *   final shape before committing. Stale responses are discarded via
 *   a per-drag sequence counter + AbortController; the ghost is torn
 *   down when the drag ends regardless of how it terminates (commit,
 *   sub-tolerance no-op, selection change, etc).
 */
const EXTRUDE_STEP = 0.1
const PREVIEW_DEBOUNCE_MS = 50

type PreviewMesh = {
  vertices: number[]
  indices: number[]
  normals: number[]
}

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

  // Live ghost-mesh state. `previewMesh` drives the React render; the
  // refs sequence in-flight requests so a slow response from a stale
  // distance never overwrites a fresh one. Sequence guard: every
  // dispatched preview call captures the seq value at issue time; on
  // resolve, the response is dropped unless seq still matches.
  const [previewMesh, setPreviewMesh] = useState<PreviewMesh | null>(null)
  const previewTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const previewAbortRef = useRef<AbortController | null>(null)
  const previewSeqRef = useRef(0)
  const previewLastDistanceRef = useRef<number | null>(null)

  // Tear down any pending preview work. Safe to call from anywhere —
  // clears the debounce timer, aborts the in-flight fetch (which
  // surfaces as an AbortError handled below), invalidates the
  // sequence counter, and drops the ghost mesh from the scene.
  const cancelPreview = () => {
    if (previewTimerRef.current !== null) {
      clearTimeout(previewTimerRef.current)
      previewTimerRef.current = null
    }
    if (previewAbortRef.current !== null) {
      previewAbortRef.current.abort()
      previewAbortRef.current = null
    }
    previewSeqRef.current += 1
    previewLastDistanceRef.current = null
    setPreviewMesh(null)
  }

  // Reset live distance whenever the selection target changes — the
  // gizmo always starts at zero relative offset for the currently-
  // selected face. CRITICAL: also release `gizmoDragging` here. If the
  // selection changes mid-drag (e.g. a backend rebroadcast clears
  // `subElementSelections`), the window-level pointerup handler that
  // normally clears the flag is removed when `active` flips false and
  // the drag-cleanup effect unmounts its listeners — leaving
  // `gizmoDragging` stuck at `true` forever, which permanently
  // disables LMB camera orbit. See CameraController.tsx.
  useEffect(() => {
    setDistance(0)
    setPickedDirection(null)
    dragRef.current = null
    setActive(false)
    setGizmoDragging(false)
    cancelPreview()
    // cancelPreview is stable across renders (refs only), and we
    // deliberately want this to run on every selection change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel?.objectId, sel?.index, setGizmoDragging])

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
      const raw = dot - drag.refDot
      // Snap to the UX step so readout, arrow length, and the eventual
      // committed extrusion all agree. Ties round-to-even (Math.round
      // default) which is acceptable at this granularity.
      const snapped = Math.round(raw / EXTRUDE_STEP) * EXTRUDE_STEP
      drag.currentDistance = snapped
      setDistance(snapped)
      schedulePreview(drag.objectId, drag.faceId, snapped, drag.axis)
    }

    // Trailing-edge debounced preview dispatcher. Rapid moves coalesce
    // into a single backend call per `PREVIEW_DEBOUNCE_MS` window;
    // the most recent distance always wins. Sub-tolerance distances
    // collapse to "no preview" so a brief drift back to ~0 clears
    // the ghost rather than showing a degenerate zero-thickness body.
    const schedulePreview = (
      objectId: string,
      faceId: number,
      snapped: number,
      axis: THREE.Vector3,
    ) => {
      if (!Number.isFinite(snapped) || Math.abs(snapped) < 1e-6) {
        // Tear down any ghost from a previous step and abort any
        // in-flight request — we want the ghost to disappear cleanly
        // when the user drags back through zero.
        if (previewTimerRef.current !== null) {
          clearTimeout(previewTimerRef.current)
          previewTimerRef.current = null
        }
        if (previewAbortRef.current !== null) {
          previewAbortRef.current.abort()
          previewAbortRef.current = null
        }
        previewSeqRef.current += 1
        previewLastDistanceRef.current = null
        setPreviewMesh(null)
        return
      }
      // Coalesce identical-distance moves — the snap step means the
      // user can move the pointer several pixels without crossing a
      // step boundary; re-issuing the same call wastes a roundtrip.
      if (previewLastDistanceRef.current === snapped) return
      if (previewTimerRef.current !== null) {
        clearTimeout(previewTimerRef.current)
      }
      previewTimerRef.current = setTimeout(() => {
        previewTimerRef.current = null
        void dispatchPreview(objectId, faceId, snapped, axis)
      }, PREVIEW_DEBOUNCE_MS)
    }

    const dispatchPreview = async (
      objectId: string,
      faceId: number,
      snapped: number,
      axis: THREE.Vector3,
    ) => {
      // Abort the prior in-flight preview, if any. The kernel still
      // ran on the server (we can't recall it), but the response
      // body is never parsed and never overwrites our ghost.
      if (previewAbortRef.current !== null) {
        previewAbortRef.current.abort()
      }
      const controller = new AbortController()
      previewAbortRef.current = controller
      const seq = ++previewSeqRef.current
      previewLastDistanceRef.current = snapped
      try {
        const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
        const resp = await fetch(
          `${API_BASE}/geometry/face/extrude/preview`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              object_uuid: objectId,
              face_id: faceId,
              distance: snapped,
              direction: [axis.x, axis.y, axis.z],
            }),
            signal: controller.signal,
          },
        )
        // Sequence guard: a slower response from an earlier dispatch
        // must not clobber a newer ghost. Abort already pushes us
        // into the catch branch in modern browsers, but the guard
        // also covers the case where the request resolved with a
        // body before abort took effect.
        if (seq !== previewSeqRef.current) return
        if (!resp.ok) {
          // Non-2xx responses are silently swallowed for preview —
          // we don't want a noisy console for every drag tick that
          // produced a degenerate intermediate state. The commit
          // path still surfaces real errors.
          return
        }
        const data = (await resp.json()) as {
          mesh?: {
            vertices?: number[]
            indices?: number[]
            normals?: number[]
          }
        }
        if (seq !== previewSeqRef.current) return
        const mesh = data.mesh
        if (
          !mesh ||
          !Array.isArray(mesh.vertices) ||
          !Array.isArray(mesh.indices) ||
          !Array.isArray(mesh.normals) ||
          mesh.indices.length === 0
        ) {
          return
        }
        setPreviewMesh({
          vertices: mesh.vertices,
          indices: mesh.indices,
          normals: mesh.normals,
        })
      } catch (err) {
        // AbortError is the expected path when a newer dispatch
        // supersedes us; ignore. Anything else is a network error
        // we also ignore for preview — the ghost simply doesn't
        // update, and the commit path will surface real failures.
        if ((err as { name?: string })?.name !== 'AbortError') {
          // Swallow; preview is best-effort.
        }
      } finally {
        if (previewAbortRef.current === controller) {
          previewAbortRef.current = null
        }
      }
    }

    const onUp = () => {
      const drag = dragRef.current
      dragRef.current = null
      setActive(false)
      setPickedDirection(null)
      // Release orbit-rotate suppression — this gesture is over.
      setGizmoDragging(false)
      // Drop the ghost the moment the user releases. Either the
      // commit succeeds and the real mesh arrives via WS broadcast
      // (no overlap window — kernel meshing is fast at realtime
      // params), or the commit is a sub-tolerance no-op and the
      // original mesh is correct. Either way, the ghost has done
      // its job.
      cancelPreview()
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

      void submitExtrude(drag.objectId, drag.faceId, finalDistance, drag.axis)
    }

    const submitExtrude = async (
      objectId: string,
      faceId: number,
      finalDistance: number,
      axis: THREE.Vector3,
    ) => {
      setSubmitting(true)
      try {
        const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
        // Pin the direction to the same vector the gizmo renders. Without
        // this, the backend falls back to `face.normal_at(0.5, 0.5)` which
        // for a closed curved face (cone lateral, cylinder side, sphere)
        // gives a radial / perpendicular-to-slant normal that disagrees
        // with the axial arrow the user sees, producing tilted side faces
        // and a non-manifold shell.
        const resp = await fetch(`${API_BASE}/geometry/face/extrude`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            object_uuid: objectId,
            face_id: faceId,
            distance: finalDistance,
            direction: [axis.x, axis.y, axis.z],
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
      // The drag was torn down externally (selection change,
      // unmount). Cancel any pending preview so a late response
      // doesn't paint a ghost over a now-irrelevant face.
      cancelPreview()
    }
    // cancelPreview is stable (refs only). The other deps are the
    // values the inner closures actually read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

  // Build a BufferGeometry from the latest preview response. Rebuilt
  // only when the mesh data identity changes — React state replaces
  // `previewMesh` wholesale on every successful response, so this
  // hashes to "once per preview tick". The geometry's previous
  // `.dispose()` is called inside the same memo to free the WebGL
  // buffers before the next one allocates.
  const previewGeometry = useMemo(() => {
    if (!previewMesh) return null
    const geometry = new THREE.BufferGeometry()
    geometry.setAttribute(
      'position',
      new THREE.Float32BufferAttribute(previewMesh.vertices, 3),
    )
    geometry.setAttribute(
      'normal',
      new THREE.Float32BufferAttribute(previewMesh.normals, 3),
    )
    geometry.setIndex(previewMesh.indices)
    return geometry
  }, [previewMesh])

  // Dispose the previous geometry when this hook re-runs or the
  // gizmo unmounts. `previewGeometry` holds onto GPU buffers; React
  // garbage-collecting the JS object doesn't free them.
  useEffect(() => {
    return () => {
      if (previewGeometry) previewGeometry.dispose()
    }
  }, [previewGeometry])

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
    <>
      {/* Translucent ghost mesh of what the commit would produce.
          Kernel returns world-space vertices (matches the commit
          response's `position: [0,0,0]`), so the mesh mounts at
          the root — outside the gizmo's positioned/oriented group.
          `depthWrite={false}` keeps the original solid visible
          through the ghost; `side: DoubleSide` makes the ghost
          read sensibly when the preview face normals temporarily
          flip during an extrude-through-self regime. */}
      {previewGeometry && (
        <mesh geometry={previewGeometry} renderOrder={1}>
          <meshStandardMaterial
            color="#ffaa00"
            transparent
            opacity={0.35}
            depthWrite={false}
            side={THREE.DoubleSide}
            metalness={0.0}
            roughness={0.8}
          />
        </mesh>
      )}
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
              {distance.toFixed(1)}
            </span>
          </div>
        </Html>
      )}
      </group>
    </>
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
