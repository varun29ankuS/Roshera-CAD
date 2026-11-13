import { useCursorStore } from '@/stores/cursor-store'
import { isNearestHit } from '@/lib/status-bar'
import { memo, useRef, useMemo, useCallback, useEffect, type ComponentRef } from 'react'
import { Edges } from '@react-three/drei'
import { useSceneStore, type CADObject } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useAssemblyStore } from '@/stores/assembly-store'
import { resolveCssVar } from '@/lib/css-color'
import { wsClient } from '@/lib/ws-client'
import { isRaycastSuppressed } from './orbit-raycast-gate'
import * as THREE from 'three'
import { useThree, type ThreeEvent } from '@react-three/fiber'

const ASM_OBJ_PREFIX = 'asm-comp:'

// Shared-perception bridge (HOVER-α.3): report the user's pointer
// interactions to the server so agents in conversation know what the
// user is pointing at ("this hole — bigger"). Fire-and-forget; errors
// are deliberately swallowed — a dropped pointer report must never
// affect the UI.
const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`
function reportPointer(
  objectId: string,
  faceId: number | undefined,
  position: [number, number, number],
) {
  void fetch(`${API_BASE}/agent/pointer`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      kind: 'click',
      object_id: objectId,
      face_id: faceId ?? null,
      position,
      at_ms: Date.now(),
    }),
  }).catch(() => {})
}

interface CADMeshProps {
  object: CADObject
  isSelected: boolean
  isHovered: boolean
}

// Memoized: `SceneObjects` re-renders on every hover / selection change, and
// without this EVERY mesh re-ran its hooks and its `<Edges>` with it. Measured
// on a 327-part imported assembly (2026-11-05): a pointer sweep across the
// model re-rendered all 327 meshes per hovered part — 51% of React's work and
// 0.4–0.65 s main-thread freezes. Props are the object (a stable reference
// until that part changes) and two booleans, so a hover now re-renders only
// the two meshes whose `isHovered` flipped.
export const CADMesh = memo(function CADMesh({ object, isSelected, isHovered }: CADMeshProps) {
  const meshRef = useRef<THREE.Mesh>(null!)
  const selectObject = useSceneStore((s) => s.selectObject)
  const setHovered = useSceneStore((s) => s.setHovered)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const registerMeshRef = useSceneStore((s) => s.registerMeshRef)
  const unregisterMeshRef = useSceneStore((s) => s.unregisterMeshRef)
  const edgeSettings = useSceneStore((s) => s.edgeSettings)
  const theme = useThemeStore((s) => s.theme)
  // While the user is editing an existing sketch, dim every solid so
  // the focus is on the 2D profile + dimension labels, not the
  // surrounding geometry. Distinct from "creating a new sketch" — that
  // path leaves `serverId` null because the panel issues `POST /api/sketch`
  // but doesn't reuse an existing session, so dimming would be confusing.
  const editingSketch = useSceneStore(
    (s) => s.sketch.active && s.sketch.serverId !== null,
  )
  // Any active sketch session — including a fresh-from-scratch sketch
  // (serverId === null) — must let pointer events fall through to the
  // SketchOverlay capture plane behind the solids. Otherwise the
  // raycaster hits CAD geometry first, this mesh's handlers fire with
  // e.stopPropagation(), and the user can only click in empty space.
  const sketchActive = useSceneStore((s) => s.sketch.active)
  const sectionView = useSceneStore((s) => s.sectionView)

  const { defaultEdgeHex, accentEdgeHex, selectedEdgeHex } = useMemo(() => {
    const tick = resolveCssVar('--cad-tick')
    const accent = resolveCssVar('--cad-selected')
    const selected = resolveCssVar('--cad-edge-selected')
    return {
      defaultEdgeHex: `#${tick.color.getHexString()}`,
      accentEdgeHex: `#${accent.color.getHexString()}`,
      selectedEdgeHex: `#${selected.color.getHexString()}`,
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme])

  // Register mesh ref for outline post-processing
  useEffect(() => {
    if (meshRef.current) {
      registerMeshRef(object.id, meshRef.current)
    }
    return () => unregisterMeshRef(object.id)
  }, [object.id, registerMeshRef, unregisterMeshRef])

  const geometry = useMemo(() => {
    const geom = new THREE.BufferGeometry()
    geom.setAttribute(
      'position',
      new THREE.BufferAttribute(object.mesh.vertices, 3),
    )
    if (object.mesh.indices.length > 0) {
      geom.setIndex(new THREE.BufferAttribute(object.mesh.indices, 1))
    }
    if (object.mesh.normals.length > 0) {
      geom.setAttribute(
        'normal',
        new THREE.BufferAttribute(object.mesh.normals, 3),
      )
    } else {
      geom.computeVertexNormals()
    }
    geom.computeBoundingBox()
    geom.computeBoundingSphere()
    return geom
  }, [object.mesh])

  // Section-view clipping plane. The plane normal is the +axis world
  // direction (negated when `flipped`); `constant` is the signed
  // distance along that normal from the origin to the plane. The
  // half-space the normal points away from is the surviving half:
  // material is hidden where `dot(normal, p) + constant < 0`. So
  // `constant = -offset` for a non-flipped slice along +axis at
  // world coordinate `offset`.
  const clippingPlanes = useMemo(() => {
    if (!sectionView.enabled) return []
    const sign = sectionView.flipped ? -1 : 1
    const normal =
      sectionView.axis === 'x' ? new THREE.Vector3(sign, 0, 0)
      : sectionView.axis === 'y' ? new THREE.Vector3(0, sign, 0)
      : new THREE.Vector3(0, 0, sign)
    return [new THREE.Plane(normal, -sign * sectionView.offset)]
  }, [sectionView.enabled, sectionView.axis, sectionView.offset, sectionView.flipped])

  // Material — no color swap for selection (outline post-processing handles that)
  // In edit-sketch mode the solid is rendered at low opacity with
  // depthWrite disabled so the in-canvas dimension labels and
  // 2D profile lines read clearly through the geometry.
  const material = useMemo(() => {
    const editingOpacity = 0.18
    const opacity = editingSketch ? editingOpacity : object.material.opacity
    return new THREE.MeshStandardMaterial({
      color: object.material.color,
      metalness: object.material.metalness,
      roughness: object.material.roughness,
      transparent: editingSketch || object.material.opacity < 1,
      opacity,
      depthWrite: editingSketch ? false : true,
      // When section view is active, switch to `FrontSide` so the cut
      // opening doesn't leak the back-facing inner walls of the solid
      // through the clipped half-space. The matching SectionCap mesh
      // (one per closed cross-section loop) fills the opening with
      // real geometry on the cutting plane — see SectionCap.tsx.
      // Without this gate the solid reads as hollow whenever section
      // is enabled (CADMesh.tsx pre-SEC.3 behaviour).
      side: sectionView.enabled ? THREE.FrontSide : THREE.DoubleSide,
      // Section View. Toggling `clippingPlanes` between [] and [plane]
      // doesn't trigger a re-mount; Three honours the new array on the
      // next frame as long as `gl.localClippingEnabled` is true (set
      // once in CADViewport's onCreated).
      clippingPlanes,
    })
  }, [object.material, editingSketch, clippingPlanes])

  // Dispose GPU resources when geometry/material are replaced (every mesh edit
  // swaps object.mesh / object.material, recomputing the memos) or when this
  // mesh unmounts. useMemo does NOT auto-dispose, so without these each edit
  // orphans the previous BufferGeometry + Material on the GPU — VRAM leaks
  // across an edit session.
  useEffect(() => () => geometry.dispose(), [geometry])
  useEffect(() => () => material.dispose(), [material])

  const toggleSubElementSelection = useSceneStore((s) => s.toggleSubElementSelection)
  const addPendingPick = useAssemblyStore((s) => s.addPendingPick)

  // Resolve a Three.js raycast triangle index to the kernel `FaceId`. The
  // backend tessellator stores one FaceId per triangle in `mesh.faceIds`
  // (length = indices/3); when present we use it directly. When absent
  // (legacy frame, or merged client-side mesh) we fall back to the raw
  // triangle index — backend topology lookup will reject anything stale.
  const resolveFaceId = useCallback(
    (triangleIndex: number): number => {
      const map = object.mesh.faceIds
      if (map && triangleIndex < map.length) {
        return map[triangleIndex]
      }
      return triangleIndex
    },
    [object.mesh.faceIds],
  )

  const handleClick = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      // While a sketch is active, the SketchOverlay capture plane owns
      // left-click. Bail without stopPropagation so R3F continues
      // dispatching the event to the next intersected mesh (the plane).
      if (sketchActive) return
      e.stopPropagation()

      if (selectionMode === 'object') {
        selectObject(object.id, e.shiftKey)
        reportPointer(object.id, undefined, e.point.toArray() as [number, number, number])
        return
      }

      // Assembly mate-pick fast path. In assembly doc mode, a face
      // click on an `asm-comp:*` mesh feeds the pending-mate flow
      // instead of the part-mode sub-element selection. We capture
      // the pick in *component-local* coordinates so the kernel's
      // `MateReference::Plane` carries through correctly when the
      // solver later applies the component transform.
      const docMode = useDocModeStore.getState().mode
      const isAssemblyComponent = object.id.startsWith(ASM_OBJ_PREFIX)
      if (docMode === 'assembly' && isAssemblyComponent && selectionMode === 'face') {
        const mesh = meshRef.current
        const face = e.face
        if (!mesh || !face) return

        const worldPoint: [number, number, number] = [
          e.point.x,
          e.point.y,
          e.point.z,
        ]

        // Origin → local frame via the inverse world matrix.
        const localOrigin = mesh.worldToLocal(e.point.clone())

        // Normal → local frame. The face normal is in object-local
        // space already (it lives on the BufferGeometry), so we
        // forward it directly. We *do* normalize defensively in case
        // the tessellator shipped a non-unit normal.
        const ln = face.normal.clone().normalize()

        const componentId = object.id.slice(ASM_OBJ_PREFIX.length)
        const native = e.nativeEvent
        addPendingPick({
          componentId,
          origin: [localOrigin.x, localOrigin.y, localOrigin.z],
          normal: [ln.x, ln.y, ln.z],
          worldPoint,
          screen: { x: native.clientX, y: native.clientY },
        })
        return
      }

      // Sub-element picking: triangle index from the raycast → kernel
      // FaceId via the per-triangle face_map shipped on the mesh. Edge
      // and vertex modes still fall back to the raw triangle index for
      // now — the kernel doesn't ship per-triangle edge/vertex maps yet,
      // so the backend resolves those from the picked point.
      const triangleIndex = e.faceIndex ?? 0
      const point = e.point.toArray() as [number, number, number]
      const subType = selectionMode as 'face' | 'edge' | 'vertex'
      const elementIndex = subType === 'face' ? resolveFaceId(triangleIndex) : triangleIndex

      // Optimistic local selection so the UI feels instant
      toggleSubElementSelection({
        objectId: object.id,
        type: subType,
        index: elementIndex,
      })

      // Shared-perception bridge: tell the server (and through it, any
      // agent in conversation) what the user just pointed at. Fire and
      // forget — pointer state is attention, not a transaction.
      if (subType === 'face') {
        reportPointer(object.id, elementIndex, point)
      }

      // Send pick request to backend for authoritative topology resolution.
      // Backend responds with a SubElementResult message handled by ws-bridge.
      wsClient.send({
        type: 'Query',
        data: {
          query_type: {
            SubElementPick: {
              object_id: object.id,
              face_index: elementIndex,
              triangle_index: triangleIndex,
              point,
              mode: selectionMode,
            },
          },
        },
        request_id: `pick-${object.id}-${elementIndex}-${Date.now()}`,
      })
    },
    [sketchActive, selectObject, toggleSubElementSelection, resolveFaceId, object.id, selectionMode, addPendingPick],
  )

  const setHoveredSubElement = useSceneStore((s) => s.setHoveredSubElement)
  const openContextMenu = useSceneStore((s) => s.openContextMenu)

  const handleContextMenu = useCallback(
    (e: ThreeEvent<MouseEvent>) => {
      e.stopPropagation()
      // Three.js gives us a synthetic event — pull screen coords from the
      // underlying native event so the menu lands at the cursor.
      const native = e.nativeEvent
      native.preventDefault?.()
      // Right-click implicitly selects the object so subsequent menu
      // actions (delete, hide) act on the visible target.
      selectObject(object.id, false)
      // Resolve the picked triangle to its kernel FaceId so the menu
      // can offer "Sketch on this face". Falls back to the raw triangle
      // index if the mesh hasn't shipped a per-triangle face_map — the
      // backend's plane-from-face endpoint will reject anything stale.
      const triangleIndex = e.faceIndex
      const faceId =
        typeof triangleIndex === 'number' ? resolveFaceId(triangleIndex) : undefined
      openContextMenu({
        x: native.clientX,
        y: native.clientY,
        objectId: object.id,
        faceId,
      })
    },
    [object.id, selectObject, openContextMenu, resolveFaceId],
  )

  const handlePointerOver = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      // Sketch active → let hover fall through to the capture plane
      // (which manages its own crosshair cursor and snap state). If we
      // ran the body, body.cursor would oscillate between 'pointer' and
      // 'crosshair' as the user moved on/off CAD meshes.
      if (sketchActive) return
      e.stopPropagation()
      setHovered(object.id)
      document.body.style.cursor = selectionMode === 'face' ? 'cell'
        : selectionMode === 'edge' || selectionMode === 'vertex' ? 'crosshair'
        : 'pointer'
    },
    [sketchActive, setHovered, object.id, selectionMode],
  )

  const handlePointerMove = useCallback(
    (e: ThreeEvent<PointerEvent>) => {
      if (sketchActive) return
      // The status bar's cursor readout: the point on the part and its face
      // -- from the nearest part only (this event reaches every part the ray
      // crosses when nothing stops it, as in Part pick mode).
      if (isNearestHit(e.intersections, e.eventObject)) useCursorStore.getState().setHit({
        objectId: object.id,
        point: e.point.toArray() as [number, number, number],
        // Only a real kernel face id -- never the triangle-index fallback.
        faceId: typeof e.faceIndex === 'number' ? object.mesh.faceIds?.[e.faceIndex] : undefined,
      })
      if (selectionMode === 'object') return
      e.stopPropagation()
      const triangleIndex = e.faceIndex ?? 0
      const subType = selectionMode as 'face' | 'edge' | 'vertex'
      // Face hover must resolve the picked triangle to its kernel
      // FaceId so `SubElementHighlight` can fan out across every
      // triangle sharing that face id. Storing the raw triangle
      // index here is what made face hover render a single triangle
      // (the fan-out loop compares `faceIds[t] === sel.index`, and
      // a triangle index will never match a FaceId).
      // Edge / vertex still get the raw triangle index; the backend
      // resolves those from the picked point on click.
      const elementIndex =
        subType === 'face' ? resolveFaceId(triangleIndex) : triangleIndex
      setHoveredSubElement({
        objectId: object.id,
        type: subType,
        index: elementIndex,
      })
    },
    [sketchActive, selectionMode, object.id, object.mesh.faceIds, setHoveredSubElement, resolveFaceId],
  )

  const handlePointerOut = useCallback(() => {
    useCursorStore.getState().setHit(null)
    if (sketchActive) return
    setHovered(null)
    setHoveredSubElement(null)
    document.body.style.cursor = 'default'
  }, [sketchActive, setHovered, setHoveredSubElement])

  // Skip the (un-BVH'd, triangle-precise) raycast while the camera is being
  // orbited — R3F would otherwise run it on every pointermove across every
  // pickable mesh, stalling the drag once a full assembly is on screen. Hover
  // is restored the instant the drag ends (see orbit-raycast-gate).
  const handleRaycast = useCallback(
    (raycaster: THREE.Raycaster, intersects: THREE.Intersection[]) => {
      if (isRaycastSuppressed()) return
      const mesh = meshRef.current
      if (mesh) THREE.Mesh.prototype.raycast.call(mesh, raycaster, intersects)
    },
    [],
  )

  // Edge colour follows hover WITHOUT remounting `<Edges>` (a remount
  // recomputes the part's whole EdgesGeometry). Written straight onto the
  // existing line material so the colour reaches the GPU on the next frame,
  // and that frame is requested: the canvas renders on demand.
  const edgesRef = useRef<ComponentRef<typeof Edges>>(null)
  const invalidate = useThree((s) => s.invalidate)
  // Selected parts carry the selection yellow on their own edges too, so a
  // selection reads even where the outline pass is hidden behind other parts.
  // OBJECT selection only: a face / edge / vertex pick also puts the part in
  // `selectedIds`, and tinting the whole part would drown the picked element
  // (which `SubElementHighlight` paints).
  const wholePartSelected = isSelected && selectionMode === 'object'
  const edgeHex = wholePartSelected
    ? selectedEdgeHex
    : isHovered
      ? accentEdgeHex
      : edgeSettings.color || defaultEdgeHex
  useEffect(() => {
    edgesRef.current?.material?.color?.set(edgeHex)
    invalidate()
  }, [edgeHex, invalidate])

  // And a faint yellow glow on the surface: a selected part must stand out
  // in a dense assembly at a glance, not only along its silhouette.
  useEffect(() => {
    material.emissive.set(wholePartSelected ? selectedEdgeHex : '#000000')
    material.emissiveIntensity = wholePartSelected ? 0.35 : 0
    invalidate()
  }, [material, wholePartSelected, selectedEdgeHex, invalidate])

  return (
    <mesh
      ref={meshRef}
      geometry={geometry}
      material={material}
      position={object.position}
      rotation={object.rotation}
      scale={object.scale}
      raycast={handleRaycast}
      castShadow
      receiveShadow
      onClick={handleClick}
      onContextMenu={handleContextMenu}
      onPointerOver={handlePointerOver}
      onPointerMove={handlePointerMove}
      onPointerOut={handlePointerOut}
      userData={{ cadObjectId: object.id }}
    >
      {edgeSettings.visible && (
        // Hover recolours the edges IN PLACE (see the effect above). This
        // used to be `key={isHovered ? 'hov' : 'def'}`, which remounted the
        // drei `<Edges>` on every hover change and threw away its cached
        // EdgesGeometry, so each hover recomputed every edge of the part —
        // measured 1.3–2.1 s main-thread freezes hovering the large parts of
        // the 327-part robot-arm import (2026-11-05).
        //
        // A WHOLE-PART selection (object mode) paints these edges yellow too
        // (`wholePartSelected` above) — the outline alone was too easy to miss
        // on dark parts. A face / edge / vertex pick does NOT: those also set
        // `selectedIds`, and `SubElementHighlight` paints the picked element.
        <Edges
          ref={edgesRef}
          threshold={edgeSettings.threshold}
          color={edgeHex}
          lineWidth={edgeSettings.lineWidth}
        />
      )}
    </mesh>
  )
})
