import { create } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import type * as THREE from 'three'
import { sketchApi, type ServerSketchSession } from '@/lib/sketch-api'

// ─── Selection granularity ───────────────────────────────────────────
export type SelectionMode = 'object' | 'face' | 'edge' | 'vertex'

export interface SubElementSelection {
  objectId: string
  type: 'face' | 'edge' | 'vertex'
  index: number
}

// ─── Transform tools ─────────────────────────────────────────────────
export type TransformTool = 'select' | 'translate' | 'rotate' | 'scale'
export type TransformSpace = 'world' | 'local'
export type SnapMode = 'none' | 'grid' | 'vertex' | 'edge' | 'face'

// ─── CAD Object ──────────────────────────────────────────────────────
export interface CADMaterial {
  color: string
  metalness: number
  roughness: number
  opacity: number
}

export interface CADMesh {
  vertices: Float32Array
  indices: Uint32Array
  normals: Float32Array
  /**
   * Per-triangle B-Rep `FaceId` array. Length = `indices.length / 3`.
   * Optional because legacy frames (and frontend-side merged meshes)
   * may not carry it; when present, the viewport uses it to resolve a
   * Three.js raycast hit (which gives a triangle index) back to a
   * kernel face for face picking and face-extrude.
   */
  faceIds?: Uint32Array
}

export interface AnalyticalGeometry {
  type: string
  // Mirrors backend `analytical_geometry.parameters`: primitives ship
  // numbers, booleans/extrudes ship strings/arrays. Consumers (e.g.
  // PropertiesPanel) format defensively per-value.
  params: Record<string, unknown>
}

export interface CADObject {
  id: string
  name: string
  objectType: string
  mesh: CADMesh
  analyticalGeometry?: AnalyticalGeometry
  material: CADMaterial
  position: [number, number, number]
  rotation: [number, number, number]
  scale: [number, number, number]
  visible: boolean
  locked: boolean
  parentId?: string
}

// ─── Camera projection ───────────────────────────────────────────────
/**
 * Lens projection. Perspective is the default (foreshortens depth);
 * orthographic projects parallel rays — preferred for engineering
 * drawings where parallel lines must stay parallel and dimensions
 * remain measurable on-screen.
 */
export type CameraProjection = 'perspective' | 'orthographic'

// ─── Camera presets ──────────────────────────────────────────────────
export interface CameraPreset {
  name: string
  position: [number, number, number]
  target: [number, number, number]
  up: [number, number, number]
}

export const CAMERA_PRESETS: Record<string, CameraPreset> = {
  front:       { name: 'Front',       position: [0, 0, 30],  target: [0, 0, 0], up: [0, 1, 0] },
  back:        { name: 'Back',        position: [0, 0, -30], target: [0, 0, 0], up: [0, 1, 0] },
  top:         { name: 'Top',         position: [0, 30, 0],  target: [0, 0, 0], up: [0, 0, -1] },
  bottom:      { name: 'Bottom',      position: [0, -30, 0], target: [0, 0, 0], up: [0, 0, 1] },
  right:       { name: 'Right',       position: [30, 0, 0],  target: [0, 0, 0], up: [0, 1, 0] },
  left:        { name: 'Left',        position: [-30, 0, 0], target: [0, 0, 0], up: [0, 1, 0] },
  isometric:   { name: 'Isometric',   position: [20, 15, 20], target: [0, 0, 0], up: [0, 1, 0] },
}

/**
 * Resolve the (camera-preset key, full preset data) pair the viewport
 * should snap to when a sketch is opened on `plane`.
 *
 *   * Standard planes map to the matching axis-aligned built-in preset.
 *   * Custom planes (sketches anchored to a B-Rep face) synthesise a
 *     camera looking along the face normal at the plane's origin, with
 *     `up` aligned to the plane's v_axis. This gives the user a
 *     flat-on view of the face regardless of orientation.
 */
function sketchCameraSetup(plane: SketchPlane): {
  presetKey: string
  preset: CameraPreset | undefined
} {
  if (isStandardPlane(plane)) {
    // Y-up world: Top view looks down -Y (sees XZ); Front looks down
    // -Z (sees XY); Right looks down -X (sees YZ). Match each plane
    // to the view that shows it face-on.
    const standardPresetKey: Record<StandardPlane, string> = {
      xy: 'front',
      xz: 'top',
      yz: 'right',
    }
    const presetKey = standardPresetKey[plane]
    return { presetKey, preset: CAMERA_PRESETS[presetKey] }
  }
  const distance = 30
  // u × v gives the outward-facing normal under the right-hand rule
  // (matches the backend's SketchPlane::Custom::normal).
  const normal: [number, number, number] = [
    plane.u_axis[1] * plane.v_axis[2] - plane.u_axis[2] * plane.v_axis[1],
    plane.u_axis[2] * plane.v_axis[0] - plane.u_axis[0] * plane.v_axis[2],
    plane.u_axis[0] * plane.v_axis[1] - plane.u_axis[1] * plane.v_axis[0],
  ]
  return {
    presetKey: 'sketch-custom',
    preset: {
      name: 'Sketch Plane',
      position: [
        plane.origin[0] + normal[0] * distance,
        plane.origin[1] + normal[1] * distance,
        plane.origin[2] + normal[2] * distance,
      ],
      target: plane.origin,
      up: plane.v_axis,
    },
  }
}

// ─── Edge display settings ──────────────────────────────────────────
export interface EdgeSettings {
  visible: boolean
  threshold: number
  lineWidth: number
  color: string
}

// ─── Grid settings ───────────────────────────────────────────────────
export interface GridSettings {
  visible: boolean
  cellSize: number
  sectionSize: number
  fadeDistance: number
  infiniteGrid: boolean
}

// ─── Sketch mode ─────────────────────────────────────────────────────
/**
 * 2D sketch plane. Points are stored in plane-local (u, v) coordinates
 * and lifted to 3D when finalised.
 *
 *   * `'xy'` → (u, v, 0); `'xz'` → (u, 0, v); `'yz'` → (0, u, v).
 *   * `CustomSketchPlane` is a free plane derived from a B-Rep planar
 *     face (or, in the future, typed in by hand). Lift is
 *     `origin + u*u_axis + v*v_axis`; the implied outward normal —
 *     and therefore the default extrude direction — is
 *     `u_axis × v_axis`.
 *
 * The wire form is shape-disambiguated: standard planes serialise as
 * the bare lowercase string; custom planes serialise as the bare
 * `CustomSketchPlane` object. Use {@link isStandardPlane} /
 * {@link isCustomPlane} to discriminate at use sites.
 */
export type StandardPlane = 'xy' | 'xz' | 'yz'

export interface CustomSketchPlane {
  origin: [number, number, number]
  u_axis: [number, number, number]
  v_axis: [number, number, number]
}

export type SketchPlane = StandardPlane | CustomSketchPlane

export function isStandardPlane(p: SketchPlane): p is StandardPlane {
  return typeof p === 'string'
}

export function isCustomPlane(p: SketchPlane): p is CustomSketchPlane {
  return typeof p !== 'string'
}

/** Drawing tool inside sketch mode. */
export type SketchTool = 'polyline' | 'rectangle' | 'circle'

export interface SketchState {
  active: boolean
  /**
   * Backend session id (UUID) once the `POST /api/sketch` round-trip
   * has completed. `null` between `enterSketch` being called and the
   * server response landing — actions taken in that window are
   * queued and replayed once the id is known.
   */
  serverId: string | null
  plane: SketchPlane
  tool: SketchTool
  /** Confirmed sketch points in plane-local (u, v) coordinates. */
  points: Array<[number, number]>
  /**
   * Live cursor position on the sketch plane for the rubber-band preview.
   * `null` when the cursor is off-plane or sketch mode is idle.
   *
   * This field is *not* persisted on the backend — it's transient UX
   * state only the local client cares about.
   */
  hover: [number, number] | null
  /** Number of segments to tessellate a circle into for extrude. */
  circleSegments: number
  /** Snap step in plane units. 0 disables snapping. */
  snapStep: number
  /** Show angles + perimeter + area annotations during sketching. */
  measure: boolean
  /** Default extrude thickness applied by the panel's "Finish" button. */
  thickness: number
  /**
   * When set, the active sketch session was reopened via "Edit sketch"
   * from the model tree, and this id is the existing extruded solid
   * that was originally produced from this sketch. The Finish handler
   * deletes that solid before re-running `extrude`, so editing the
   * profile *replaces* the feature instead of stamping out a duplicate.
   * `null` for a fresh sketch (the toolbar's "New Sketch" path).
   */
  editingSourceObjectId: string | null
}

// ─── Scene store ─────────────────────────────────────────────────────
interface SceneState {
  // Objects
  objects: Map<string, CADObject>
  objectOrder: string[]

  // Selection
  selectedIds: Set<string>
  hoveredId: string | null
  selectionMode: SelectionMode
  subElementSelections: SubElementSelection[]
  hoveredSubElement: SubElementSelection | null

  // Mesh ref registry for outline effects
  meshRefs: Map<string, THREE.Mesh>

  /**
   * True while a viewport gizmo (e.g. `ExtrudeGizmo`) owns the pointer.
   * `OrbitControls` listens at the canvas DOM level — outside R3F's
   * synthetic event tree — so `e.stopPropagation()` from a gizmo
   * pointerdown does NOT prevent orbit-rotation from firing on the
   * same gesture. Gating `enableRotate` on this flag is the only
   * reliable suppression. Cleared on pointerup.
   */
  gizmoDragging: boolean

  // Transform
  activeTool: TransformTool
  transformSpace: TransformSpace
  snapMode: SnapMode
  snapValue: number

  // Camera
  cameraPreset: string | null
  pendingCameraPreset: CameraPreset | null
  /**
   * Object id the viewport should auto-frame on next animation tick.
   * Set by `ws-bridge` when `ObjectCreated` arrives so newly-created
   * geometry doesn't land off-screen. Cleared by CameraController
   * once the animation has been seeded.
   */
  pendingFrameObjectId: string | null

  /** Active lens projection. See {@link CameraProjection}. */
  cameraProjection: CameraProjection

  // Edges
  edgeSettings: EdgeSettings

  // Grid
  gridSettings: GridSettings

  // Viewport
  viewportSize: { width: number; height: number }

  /**
   * Right-click context-menu state for the viewport. `objectId` is the
   * CADObject the menu is acting on (set when the user right-clicks a
   * mesh). `faceId` is set when the click resolved to a kernel face —
   * used to gate the "Sketch on this face" item. `null` while the menu
   * is closed.
   */
  contextMenu: {
    x: number
    y: number
    objectId: string
    faceId?: number
  } | null

  /** Interactive 2D sketch state. See {@link SketchState}. */
  sketch: SketchState

  /**
   * All sketch sessions known to the backend, keyed by server id. The
   * active local sketch is *also* present in this map (under
   * `sketch.serverId`) so the model tree can list every sketch
   * uniformly. Hydrated on connect via `sketchApi.list()`, kept in
   * sync by `Sketch{Created,Updated,Deleted}` WS frames.
   */
  serverSketches: Map<string, ServerSketchSession>

  // Three.js refs (set by canvas components)
  sceneRef: THREE.Scene | null
  cameraRef: THREE.Camera | null
  glRef: THREE.WebGLRenderer | null

  // Actions
  addObject: (obj: CADObject) => void
  updateObject: (id: string, patch: Partial<CADObject>) => void
  removeObject: (id: string) => void
  clearScene: () => void

  selectObject: (id: string, additive: boolean) => void
  deselectAll: () => void
  setHovered: (id: string | null) => void
  setSelectionMode: (mode: SelectionMode) => void
  addSubElementSelection: (sel: SubElementSelection) => void
  clearSubElementSelections: () => void
  toggleSubElementSelection: (sel: SubElementSelection) => void
  setHoveredSubElement: (sel: SubElementSelection | null) => void

  registerMeshRef: (id: string, mesh: THREE.Mesh) => void
  unregisterMeshRef: (id: string) => void

  setGizmoDragging: (dragging: boolean) => void

  setActiveTool: (tool: TransformTool) => void
  setTransformSpace: (space: TransformSpace) => void
  setSnapMode: (mode: SnapMode) => void
  setSnapValue: (value: number) => void

  setCameraPreset: (preset: string) => void
  clearPendingCameraPreset: () => void
  setPendingFrameObject: (id: string | null) => void
  setCameraProjection: (projection: CameraProjection) => void
  toggleCameraProjection: () => void

  setEdgeSettings: (settings: Partial<EdgeSettings>) => void
  setGridSettings: (settings: Partial<GridSettings>) => void
  setViewportSize: (size: { width: number; height: number }) => void
  openContextMenu: (menu: { x: number; y: number; objectId: string; faceId?: number }) => void
  closeContextMenu: () => void

  enterSketch: (plane?: SketchPlane, tool?: SketchTool) => void
  /**
   * Tear down the local sketch UI. By default this also deletes the
   * backend session (matches user-initiated cancel via Esc / X / toolbar
   * exit). Pass `{ deleteBackend: false }` from paths that want the
   * session to survive — most importantly the post-extrude finish path,
   * which uses `consume: false` so the user can later "Edit sketch"
   * from the model tree and reopen the same profile.
   */
  exitSketch: (options?: { deleteBackend?: boolean }) => void
  setSketchTool: (tool: SketchTool) => void
  setSketchPlane: (plane: SketchPlane) => void
  addSketchPoint: (point: [number, number]) => void
  popSketchPoint: () => void
  clearSketchPoints: () => void
  setSketchHover: (point: [number, number] | null) => void
  /**
   * Replace a single sketch point in place. Used by the panel's
   * dimension inputs so the user can type exact coordinates / sizes
   * rather than only approximate by click.
   */
  setSketchPoint: (index: number, point: [number, number]) => void
  setSketchView: (
    patch: Partial<Pick<SketchState, 'snapStep' | 'measure' | 'thickness'>>,
  ) => void
  /**
   * Reconcile a server `SketchSession` snapshot into the local store.
   * Called by `ws-bridge` on every `SketchCreated` / `SketchUpdated`
   * frame. The local state for the active session is overwritten so
   * the backend stays the single source of truth — optimistic local
   * mutations are clobbered by the broadcast that follows.
   */
  applyServerSketchSnapshot: (session: ServerSketchSession) => void
  /**
   * Drop a server-side session id from the local store. Called by
   * `ws-bridge` on `SketchDeleted` (so peer-initiated deletes flow
   * through correctly) and by `extrude` flows after `consume=true`
   * removes the session.
   */
  clearServerSketchId: (id: string) => void
  /**
   * Replace the entire `serverSketches` map. Called once on
   * connect by `ws-bridge` after `sketchApi.list()` resolves so the
   * model tree shows existing sketches even before any
   * `SketchCreated` frame arrives (e.g. after a page reload while
   * sketches are live on the backend).
   */
  setServerSketches: (sessions: ServerSketchSession[]) => void
  /**
   * Re-enter an existing server-side sketch as the active editing
   * session. Used by the model tree's "Edit sketch" context menu.
   * Sets the camera preset to look face-on at the plane and seeds
   * the local `sketch.points` from the server snapshot.
   */
  editServerSketch: (id: string) => void
  setSceneRef: (scene: THREE.Scene | null) => void
  setCameraRef: (camera: THREE.Camera | null) => void
  setGlRef: (gl: THREE.WebGLRenderer | null) => void
}

export const useSceneStore = create<SceneState>()(
  subscribeWithSelector((set, get) => ({
    objects: new Map(),
    objectOrder: [],
    selectedIds: new Set(),
    hoveredId: null,
    selectionMode: 'object',
    subElementSelections: [],
    hoveredSubElement: null,
    meshRefs: new Map(),
    gizmoDragging: false,
    activeTool: 'select',
    transformSpace: 'world',
    snapMode: 'grid',
    snapValue: 1,
    edgeSettings: {
      visible: true,
      threshold: 15,
      lineWidth: 1,
      color: '#000000',
    },
    cameraPreset: 'isometric',
    pendingCameraPreset: null,
    pendingFrameObjectId: null,
    cameraProjection: 'perspective',
    gridSettings: {
      visible: true,
      cellSize: 1,
      sectionSize: 10,
      fadeDistance: 80,
      infiniteGrid: true,
    },
    viewportSize: { width: 0, height: 0 },
    contextMenu: null,
    sketch: {
      active: false,
      serverId: null,
      plane: 'xy',
      tool: 'polyline',
      points: [],
      hover: null,
      circleSegments: 64,
      snapStep: 0.5,
      measure: true,
      thickness: 5,
      editingSourceObjectId: null,
    },
    serverSketches: new Map(),
    sceneRef: null,
    cameraRef: null,
    glRef: null,

    addObject: (obj) =>
      set((state) => {
        const objects = new Map(state.objects)
        const existed = objects.has(obj.id)
        objects.set(obj.id, obj)
        // `addObject` is also the path the WS bridge uses to apply an
        // `ObjectCreated` for an id we've already seen (e.g., REST
        // response landed first, then the broadcast). Treat it as an
        // upsert: the Map entry is replaced, but `objectOrder` must
        // not grow with duplicates or `SceneObjects` would render the
        // same id twice and React would emit duplicate-key warnings.
        const objectOrder = existed
          ? state.objectOrder
          : [...state.objectOrder, obj.id]
        return { objects, objectOrder }
      }),

    updateObject: (id, patch) =>
      set((state) => {
        const existing = state.objects.get(id)
        if (!existing) return state
        const objects = new Map(state.objects)
        objects.set(id, { ...existing, ...patch })
        return { objects }
      }),

    removeObject: (id) =>
      set((state) => {
        const objects = new Map(state.objects)
        objects.delete(id)
        const selectedIds = new Set(state.selectedIds)
        selectedIds.delete(id)
        return {
          objects,
          objectOrder: state.objectOrder.filter((oid) => oid !== id),
          selectedIds,
          hoveredId: state.hoveredId === id ? null : state.hoveredId,
        }
      }),

    clearScene: () =>
      set({
        objects: new Map(),
        objectOrder: [],
        selectedIds: new Set(),
        hoveredId: null,
        subElementSelections: [],
        serverSketches: new Map(),
      }),

    selectObject: (id, additive) =>
      set((state) => {
        if (state.selectionMode !== 'object') {
          return state
        }
        const selectedIds = new Set(additive ? state.selectedIds : [])
        if (selectedIds.has(id)) {
          selectedIds.delete(id)
        } else {
          selectedIds.add(id)
        }
        return { selectedIds, subElementSelections: [] }
      }),

    deselectAll: () =>
      set({ selectedIds: new Set(), subElementSelections: [] }),

    setHovered: (id) => set({ hoveredId: id }),

    setSelectionMode: (mode) =>
      set({ selectionMode: mode, subElementSelections: [], hoveredSubElement: null }),

    setHoveredSubElement: (sel) => set({ hoveredSubElement: sel }),

    addSubElementSelection: (sel) =>
      set((state) => ({
        subElementSelections: [...state.subElementSelections, sel],
        selectedIds: new Set([sel.objectId]),
      })),

    clearSubElementSelections: () => set({ subElementSelections: [] }),

    toggleSubElementSelection: (sel) =>
      set((state) => {
        const existing = state.subElementSelections.findIndex(
          (s) =>
            s.objectId === sel.objectId &&
            s.type === sel.type &&
            s.index === sel.index,
        )
        if (existing >= 0) {
          const next = [...state.subElementSelections]
          next.splice(existing, 1)
          return { subElementSelections: next }
        }
        return {
          subElementSelections: [...state.subElementSelections, sel],
          selectedIds: new Set([sel.objectId]),
        }
      }),

    registerMeshRef: (id, mesh) =>
      set((state) => {
        const meshRefs = new Map(state.meshRefs)
        meshRefs.set(id, mesh)
        return { meshRefs }
      }),

    unregisterMeshRef: (id) =>
      set((state) => {
        const meshRefs = new Map(state.meshRefs)
        meshRefs.delete(id)
        return { meshRefs }
      }),

    setGizmoDragging: (dragging) => set({ gizmoDragging: dragging }),

    setActiveTool: (tool) => set({ activeTool: tool }),
    setTransformSpace: (space) => set({ transformSpace: space }),
    setSnapMode: (mode) => set({ snapMode: mode }),
    setSnapValue: (value) => set({ snapValue: value }),

    setCameraPreset: (preset) => {
      const presetData = CAMERA_PRESETS[preset]
      if (presetData) {
        set({ cameraPreset: preset, pendingCameraPreset: presetData })
      }
    },

    clearPendingCameraPreset: () => set({ pendingCameraPreset: null }),

    setPendingFrameObject: (id) => set({ pendingFrameObjectId: id }),

    setCameraProjection: (projection) => set({ cameraProjection: projection }),
    // Toggling projection also snaps the camera back to the isometric
    // preset. The button is a "show me the part in 3D" affordance —
    // if the user was on a 2D preset (Top / Front / Side), keeping
    // that angle in orthographic mode collapses the model to a single
    // face and reads as "the toggle flattened everything". Snapping
    // to iso guarantees a recognisable 3D view in either projection;
    // the user can still hit Top/Front/Side after the toggle to lock
    // in a measurement-accurate ortho elevation.
    toggleCameraProjection: () =>
      set((state) => ({
        cameraProjection:
          state.cameraProjection === 'perspective' ? 'orthographic' : 'perspective',
        cameraPreset: 'isometric',
        pendingCameraPreset: CAMERA_PRESETS.isometric,
      })),

    setEdgeSettings: (settings) =>
      set((state) => ({
        edgeSettings: { ...state.edgeSettings, ...settings },
      })),

    setGridSettings: (settings) =>
      set((state) => ({
        gridSettings: { ...state.gridSettings, ...settings },
      })),

    setViewportSize: (size) => set({ viewportSize: size }),

    openContextMenu: (menu) => set({ contextMenu: menu }),
    closeContextMenu: () => set({ contextMenu: null }),

    enterSketch: (plane, tool) => {
      const targetPlane = plane ?? useSceneStore.getState().sketch.plane
      const targetTool = tool ?? useSceneStore.getState().sketch.tool
      // Snap the camera flat to the chosen plane so the user sees the
      // sketch face-on. Standard planes hit one of the axis presets;
      // custom (face-anchored) planes synthesise a fresh preset along
      // the face normal — see sketchCameraSetup.
      const { presetKey, preset } = sketchCameraSetup(targetPlane)
      set((state) => ({
        sketch: {
          ...state.sketch,
          active: true,
          serverId: null,
          plane: targetPlane,
          tool: targetTool,
          points: [],
          hover: null,
          // Fresh sketch from the toolbar — not editing an existing
          // feature, so the post-extrude path will create a new solid.
          editingSourceObjectId: null,
        },
        // Drop any object/sub-element selection so the picker can't fire
        // beneath the sketch overlay.
        selectedIds: new Set(),
        subElementSelections: [],
        hoveredSubElement: null,
        contextMenu: null,
        cameraPreset: presetKey,
        pendingCameraPreset: preset ?? state.pendingCameraPreset,
      }))
      // Fire the backend create. The server is the source of truth —
      // a `SketchCreated` WS frame will reconcile the snapshot, but
      // we also stamp `serverId` from the direct response in case the
      // broadcast is delayed.
      sketchApi
        .create(targetPlane, targetTool)
        .then((session) => {
          // Drop the response if the user already exited or restarted
          // the sketch; otherwise stamp the id and reconcile.
          const cur = useSceneStore.getState().sketch
          if (!cur.active || cur.serverId !== null) return
          useSceneStore.getState().applyServerSketchSnapshot(session)
        })
        .catch((err) => {
          console.error('[sketch] create failed:', err)
        })
    },

    exitSketch: (options) => {
      const cur = useSceneStore.getState().sketch
      const id = cur.serverId
      // When the panel is closing on a sketch that was reopened from
      // an existing feature ("Edit sketch" path), the backend session
      // *is* that feature's profile — deleting it would orphan the
      // visible solid. Default the implicit-delete to false in that
      // case; a caller that genuinely wants to wipe the session can
      // still pass `deleteBackend: true` explicitly.
      const defaultDelete = cur.editingSourceObjectId === null
      const deleteBackend = options?.deleteBackend ?? defaultDelete
      set((state) => ({
        sketch: {
          ...state.sketch,
          active: false,
          serverId: null,
          points: [],
          hover: null,
          editingSourceObjectId: null,
        },
      }))
      // Best-effort delete on the backend. If the session never
      // materialised (id null) there's nothing to do; the in-flight
      // create is orphaned but harmless — the manager keeps it until
      // a future GC sweep or process restart.
      // Skipped when `deleteBackend` is false: the extrude finish path
      // wants the session preserved (consume:false) so the user can
      // reopen it from the model tree's "Edit sketch" action.
      if (id && deleteBackend) {
        sketchApi.delete(id).catch((err) => {
          console.error('[sketch] delete failed:', err)
        })
      }
    },

    setSketchTool: (tool) => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => ({
        // Switching tools mid-sketch wipes the in-progress points; the
        // primitives don't share semantics (polyline = N points, rectangle
        // = 2 corners, circle = center+radius), so reusing prior clicks
        // would always be wrong.
        sketch: { ...state.sketch, tool, points: [], hover: state.sketch.hover },
      }))
      if (id) {
        sketchApi.setTool(id, tool).catch((err) => {
          console.error('[sketch] setTool failed:', err)
        })
      }
    },

    setSketchPlane: (plane) => {
      // Mirror enterSketch: re-orient the camera to face the new plane
      // so the sketcher always sees the surface flat-on. Standard
      // planes hit one of the axis presets; custom (face-anchored)
      // planes synthesise a fresh preset along the face normal — see
      // sketchCameraSetup.
      const { presetKey, preset } = sketchCameraSetup(plane)
      const id = useSceneStore.getState().sketch.serverId
      set((state) => ({
        sketch: { ...state.sketch, plane, points: [], hover: null },
        cameraPreset: presetKey,
        pendingCameraPreset: preset ?? state.pendingCameraPreset,
      }))
      if (id) {
        sketchApi.setPlane(id, plane).catch((err) => {
          console.error('[sketch] setPlane failed:', err)
        })
      }
    },

    addSketchPoint: (point) => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => ({
        sketch: { ...state.sketch, points: [...state.sketch.points, point] },
      }))
      if (id) {
        sketchApi.addPoint(id, point).catch((err) => {
          console.error('[sketch] addPoint failed:', err)
        })
      }
    },

    popSketchPoint: () => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => ({
        sketch: { ...state.sketch, points: state.sketch.points.slice(0, -1) },
      }))
      if (id) {
        sketchApi.popPoint(id).catch((err) => {
          console.error('[sketch] popPoint failed:', err)
        })
      }
    },

    clearSketchPoints: () => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => ({ sketch: { ...state.sketch, points: [] } }))
      if (id) {
        sketchApi.clearPoints(id).catch((err) => {
          console.error('[sketch] clearPoints failed:', err)
        })
      }
    },

    setSketchHover: (point) =>
      // Hover position is local-only UX — never sent to the backend.
      set((state) => ({ sketch: { ...state.sketch, hover: point } })),

    setSketchPoint: (index, point) => {
      const id = useSceneStore.getState().sketch.serverId
      let updated = false
      set((state) => {
        if (index < 0 || index >= state.sketch.points.length) return state
        const points = state.sketch.points.slice()
        points[index] = point
        updated = true
        return { sketch: { ...state.sketch, points } }
      })
      if (updated && id) {
        sketchApi.setPoint(id, index, point).catch((err) => {
          console.error('[sketch] setPoint failed:', err)
        })
      }
    },

    setSketchView: (patch) =>
      // snapStep / measure / thickness are panel UX state; the backend
      // session has no equivalent fields, so this stays purely local.
      set((state) => ({ sketch: { ...state.sketch, ...patch } })),

    applyServerSketchSnapshot: (session) =>
      set((state) => {
        // Maintain the full `serverSketches` map regardless of which
        // session is currently active locally — the model tree needs
        // every known sketch, not just the one being edited. Cloning
        // the map keeps zustand's reference-equality selectors honest.
        const serverSketches = new Map(state.serverSketches)
        serverSketches.set(session.id, session)

        // The active sketch state only mirrors the *local* user's
        // session. A peer's snapshot lands in `serverSketches` (so
        // the tree shows it) but doesn't clobber the local edit.
        if (state.sketch.serverId !== null && state.sketch.serverId !== session.id) {
          return { serverSketches }
        }
        // First snapshot for an in-progress local sketch: stamp the
        // id and accept the server's view of points/tool/plane. The
        // local plane/tool already match (set on `enterSketch`), but
        // we trust the server in case of races.
        return {
          serverSketches,
          sketch: {
            ...state.sketch,
            serverId: session.id,
            plane: session.plane,
            tool: session.tool,
            points: session.points,
            circleSegments: session.circle_segments,
          },
        }
      }),

    clearServerSketchId: (id) =>
      set((state) => {
        const serverSketches = new Map(state.serverSketches)
        const removed = serverSketches.delete(id)

        if (state.sketch.serverId !== id) {
          // Nothing to do for the active sketch; just commit the map
          // mutation if anything actually changed so we avoid a
          // pointless re-render.
          return removed ? { serverSketches } : state
        }
        return {
          serverSketches,
          sketch: {
            ...state.sketch,
            // Drop the id so the panel knows the session is gone, but
            // leave `active` to whatever it currently is — the
            // ObjectCreated frame on extrude races with the
            // SketchDeleted frame, so we mustn't tear down `active`
            // here unless the user explicitly exits.
            serverId: null,
          },
        }
      }),

    setServerSketches: (sessions) =>
      set(() => {
        const serverSketches = new Map<string, ServerSketchSession>()
        for (const s of sessions) {
          serverSketches.set(s.id, s)
        }
        return { serverSketches }
      }),

    editServerSketch: async (id) => {
      // Resolve the session: prefer the locally-mirrored copy, but fall
      // back to a REST GET when the id isn't in the store. This matters
      // for sketches surfaced as children of an extruded solid — the
      // model-tree synthesizes those nodes from `analyticalGeometry.
      // params.sketch_id`, which survives even when the polling refresh
      // hasn't yet repopulated `serverSketches`. If the backend itself
      // 404s, the sketch was consumed by the extrude (`consume: true`
      // is the default on `/sketch/{id}/extrude`); surface a clear
      // warning so the click doesn't appear to silently no-op.
      let session = get().serverSketches.get(id)
      if (!session) {
        try {
          session = await sketchApi.get(id)
          // Mirror the freshly-fetched session into the store so the
          // model tree (and any other consumer) sees it consistently.
          set((state) => {
            const next = new Map(state.serverSketches)
            next.set(session!.id, session!)
            return { serverSketches: next }
          })
        } catch (err) {
          console.warn(
            `[scene-store] editServerSketch(${id}): backend has no session ` +
              `for this sketch. It was likely consumed by an extrude — ` +
              `re-create the sketch to edit it.`,
            err,
          )
          return
        }
      }

      // Match enterSketch / setSketchPlane: standard planes hit one of
      // the axis presets; custom (face-anchored) planes synthesise a
      // fresh preset along the face normal. See sketchCameraSetup.
      const { presetKey, preset } = sketchCameraSetup(session.plane)

      // Find the existing extruded solid produced from this sketch so
      // the Finish handler can replace it on re-extrude. We scan the
      // local object map for `analyticalGeometry.params.sketch_id ===
      // session.id`. If no match (e.g. the source solid has been
      // deleted in this session), the post-extrude path falls back to
      // append behaviour, which is the right thing to do.
      let editingSourceObjectId: string | null = null
      for (const obj of get().objects.values()) {
        const params = obj.analyticalGeometry?.params as
          | Record<string, unknown>
          | undefined
        if (params && params['sketch_id'] === session.id) {
          editingSourceObjectId = obj.id
          break
        }
      }

      set((state) => ({
        sketch: {
          ...state.sketch,
          active: true,
          serverId: session!.id,
          plane: session!.plane,
          tool: session!.tool,
          points: session!.points,
          circleSegments: session!.circle_segments,
          hover: null,
          editingSourceObjectId,
        },
        // Clear any stale selection / sub-element / context-menu state
        // so the sketch overlay receives clicks cleanly. Mirrors the
        // teardown enterSketch already does on first-time entry.
        selectedIds: new Set(),
        subElementSelections: [],
        hoveredSubElement: null,
        contextMenu: null,
        cameraPreset: presetKey,
        pendingCameraPreset: preset ?? state.pendingCameraPreset,
      }))
    },

    setSceneRef: (scene) => set({ sceneRef: scene }),
    setCameraRef: (camera) => set({ cameraRef: camera }),
    setGlRef: (gl) => set({ glRef: gl }),
  })),
)
