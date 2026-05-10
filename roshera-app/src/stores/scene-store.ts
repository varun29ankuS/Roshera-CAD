import { create } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import type * as THREE from 'three'
import {
  sketchApi,
  type ServerSketchSession,
  type ServerSketchShape,
} from '@/lib/sketch-api'

// Re-exported so consumers (panels, overlays) can import the
// SketchShape wire type from the same module they import the rest
// of the sketch types from.
export type { ServerSketchShape } from '@/lib/sketch-api'

// ─── Selection granularity ───────────────────────────────────────────
export type SelectionMode = 'object' | 'face' | 'edge' | 'vertex'

export interface SubElementSelection {
  objectId: string
  type: 'face' | 'edge' | 'vertex'
  index: number
  /**
   * Optional flat polyline `[x,y,z, x,y,z, ...]` sampled by the kernel
   * for edge selections. When present the viewport renders this exact
   * curve; otherwise it falls back to outlining the picked triangle.
   */
  polyline?: number[]
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
/**
 * Apply `update` to the *last* element of `shapes` (the active /
 * in-progress shape) and return a new array. No-op when `shapes` is
 * empty — caller should never hit that path under the
 * "shapes invariantly non-empty while sketch is active" rule, but we
 * defensively pass through rather than throw so a transient empty
 * doesn't crash an action.
 */
function withActiveShapeUpdate(
  shapes: ServerSketchShape[],
  update: (s: ServerSketchShape) => ServerSketchShape,
): ServerSketchShape[] {
  if (shapes.length === 0) return shapes
  const next = shapes.slice()
  next[next.length - 1] = update(next[next.length - 1])
  return next
}

// ─── Pending-op queue for the sketch session ─────────────────────────
//
// `enterSketch` fires `sketchApi.create` asynchronously and the
// resulting `serverId` only lands when the response arrives. Anything
// the user does in that window (place a point, add a second shape,
// switch tool, hit Finish) needs to be replayed to the backend in
// order once the id is known — otherwise the second shape is silently
// dropped by the API layer and Finish bails with "Preparing sketch
// session". Module-level state because there's at most one in-flight
// create at a time.
type PendingSketchOp =
  | { type: 'addPoint'; point: [number, number] }
  | { type: 'popPoint' }
  | { type: 'clearPoints' }
  | { type: 'setPoint'; index: number; point: [number, number] }
  | { type: 'setTool'; tool: SketchTool }
  | { type: 'setPlane'; plane: SketchPlane }
  | { type: 'addShape'; tool: SketchTool }
  | { type: 'deleteShape'; idx: number }

let sketchPendingOps: PendingSketchOp[] = []
let sketchSessionReady: Promise<string> | null = null
let sketchSessionReadyResolve: ((id: string) => void) | null = null
let sketchSessionReadyReject: ((err: unknown) => void) | null = null

function resetSketchPendingState(reason?: string): void {
  sketchPendingOps = []
  if (sketchSessionReadyReject) {
    sketchSessionReadyReject(new Error(reason ?? 'sketch session ended'))
  }
  sketchSessionReady = null
  sketchSessionReadyResolve = null
  sketchSessionReadyReject = null
}

async function replayPendingSketchOp(
  id: string,
  op: PendingSketchOp,
): Promise<ServerSketchSession> {
  switch (op.type) {
    case 'addPoint':
      return sketchApi.addPoint(id, op.point)
    case 'popPoint':
      return sketchApi.popPoint(id)
    case 'clearPoints':
      return sketchApi.clearPoints(id)
    case 'setPoint':
      return sketchApi.setPoint(id, op.index, op.point)
    case 'setTool':
      return sketchApi.setTool(id, op.tool)
    case 'setPlane':
      return sketchApi.setPlane(id, op.plane)
    case 'addShape':
      return sketchApi.addShape(id, { tool: op.tool })
    case 'deleteShape':
      return sketchApi.deleteShape(id, op.idx)
  }
}

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

/**
 * Cutting-plane configuration applied to every CAD mesh material when
 * `enabled === true`. Frontend-only — no backend round-trip; the
 * kernel keeps the full topology and the renderer simply hides one
 * half-space behind a `THREE.Plane`.
 *
 * `axis` selects the world axis the plane normal points along. `offset`
 * is the plane's signed distance from the world origin along that
 * axis (in world units). `flipped` inverts which half-space is
 * culled — the kernel-side topology never changes, so toggling
 * `flipped` is a free O(1) way to inspect the other half.
 */
export interface SectionViewState {
  enabled: boolean
  axis: 'x' | 'y' | 'z'
  offset: number
  flipped: boolean
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

/**
 * Kind of snap target the cursor is currently locked to. Drives the
 * visual marker rendered by the sketch overlay (cyan ring at the
 * snapped (u, v)) so the user can read snap state before clicking.
 */
export type SnapKind = 'vertex' | 'midpoint' | 'center' | 'quadrant'

/**
 * A magnetic snap target on the sketch plane. Computed every
 * pointermove from the committed shapes' feature points (polyline
 * vertices + segment midpoints; rectangle corners + edge midpoints +
 * center; circle center + 4 quadrant points). Pure UX state — never
 * sent to the backend.
 */
export interface SnapTarget {
  uv: [number, number]
  kind: SnapKind
}

/**
 * Inference axis the cursor is currently locked to relative to the
 * previous anchor point — `'h'` = horizontal (cursor v == anchor v),
 * `'v'` = vertical (cursor u == anchor u). Drives the dashed
 * construction line rendered through the anchor. Pure UX state.
 */
export type InferenceAxis = 'h' | 'v'

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
  /**
   * Mirror of the backend `SketchSession.shapes` (list of
   * `{id, tool, points}`). Invariantly non-empty: the *last* element
   * is the active (in-progress) shape — its `tool` and `points` are
   * also surfaced as top-level convenience fields (`tool`, `points`)
   * so the existing panel + overlay code that only ever cared about
   * the in-progress drawing keeps working unchanged. Earlier entries
   * are committed shapes the overlay renders as faint guides while
   * the user lays out a new shape inside / next to them. Outer-vs-
   * hole classification is decided geometrically at extrude time, so
   * shapes carry no role tag.
   */
  shapes: ServerSketchShape[]
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
  /**
   * Magnetic snap target the cursor is currently locked to (cyan ring
   * marker in the overlay). `null` when the cursor is free or off-plane.
   * Refreshed on every pointermove; never serialized to the backend.
   */
  snapTarget: SnapTarget | null
  /**
   * Horizontal/vertical inference axis the cursor is currently locked
   * to, relative to the previous anchor point. Drives the dashed
   * construction line through the anchor. `null` when not inferring.
   */
  inferenceAxis: InferenceAxis | null
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

  /**
   * Live preview state for the fillet/chamfer modify dialog. While the
   * dialog is open and edges are picked, the viewport renders a
   * cross-section indicator at the midpoint of each picked edge so the
   * user can visually couple the numeric input to a geometric scale.
   * `null` when no modify dialog is open.
   *
   * `value` is in plane units (mm). For fillet it's the radius; for
   * chamfer it's the equal-distance setback. Shell does not appear here
   * — shell preview is whole-solid and would require a backend round
   * trip, deferred to a follow-up.
   */
  modifyPreview: { mode: 'fillet' | 'chamfer'; value: number } | null

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

  /** Cutting-plane state for Section View. See {@link SectionViewState}. */
  sectionView: SectionViewState

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
  setModifyPreview: (next: { mode: 'fillet' | 'chamfer'; value: number } | null) => void

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
  setSectionView: (settings: Partial<SectionViewState>) => void
  toggleSectionView: () => void
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
   * Update the transient snap descriptor (snap target + inference axis)
   * the overlay computes on every pointermove. Drives the cyan ring
   * marker and dashed inference line. Pure UX state — never serialized.
   */
  setSketchSnapState: (state: {
    snapTarget: SnapTarget | null
    inferenceAxis: InferenceAxis | null
  }) => void
  /**
   * Resolve once the active sketch's `serverId` is known and the
   * pending-op queue has fully drained. Resolves immediately with the
   * id when serverId is already set; rejects when the create round-
   * trip failed or the user exited before it landed. The Finish
   * handler awaits this so a fast user can't trip the "Preparing
   * sketch session" race.
   */
  awaitSketchReady: () => Promise<string>
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
   * Commit the active shape and append a fresh empty shape with the
   * given tool to draw next. The new shape becomes active. If the
   * active shape currently has no points, this is a no-op (no point
   * committing an empty shape, the user would just end up with two
   * empty shapes in a row).
   */
  addNewSketchShape: (tool?: SketchTool) => void
  /**
   * Drop the shape at `idx`. Refuses to remove the last remaining
   * shape (matches the backend invariant). When the active shape is
   * deleted, the new last entry becomes active.
   */
  deleteSketchShape: (idx: number) => void
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
    modifyPreview: null,
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
    sectionView: {
      enabled: false,
      axis: 'x',
      offset: 0,
      flipped: false,
    },
    viewportSize: { width: 0, height: 0 },
    contextMenu: null,
    sketch: {
      active: false,
      serverId: null,
      plane: 'xy',
      shapes: [],
      tool: 'polyline',
      points: [],
      hover: null,
      circleSegments: 64,
      snapStep: 0.5,
      measure: true,
      thickness: 5,
      editingSourceObjectId: null,
      snapTarget: null,
      inferenceAxis: null,
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
    setModifyPreview: (next) => set({ modifyPreview: next }),

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

    setSectionView: (settings) =>
      set((state) => ({
        sectionView: { ...state.sectionView, ...settings },
      })),

    toggleSectionView: () =>
      set((state) => ({
        sectionView: { ...state.sectionView, enabled: !state.sectionView.enabled },
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
          // Seed a single shape locally so the overlay / dimension-
          // input code paths that read `shapes[last]` have an entry
          // before the backend snapshot lands.
          shapes: [
            {
              id: 'pending',
              tool: targetTool,
              points: [],
            },
          ],
          tool: targetTool,
          points: [],
          hover: null,
          // Fresh sketch from the toolbar — not editing an existing
          // feature, so the post-extrude path will create a new solid.
          editingSourceObjectId: null,
          snapTarget: null,
          inferenceAxis: null,
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
      // Reset and re-arm the readiness promise. Any prior in-flight
      // create from a previous (now abandoned) session is rejected so
      // its `awaitSketchReady` consumers fail fast rather than racing
      // against the new session.
      resetSketchPendingState('sketch session restarted')
      sketchSessionReady = new Promise<string>((resolve, reject) => {
        sketchSessionReadyResolve = resolve
        sketchSessionReadyReject = reject
      })
      // Capture the resolve/reject references for *this* session.
      // If the user exits and re-enters, `enterSketch` swaps the
      // module-level handles to a new pair; comparing identity inside
      // our `.then` lets us detect that we've been superseded and
      // bail without touching the new session's queue.
      const mySessionResolve = sketchSessionReadyResolve
      const mySessionReject = sketchSessionReadyReject
      // Suppress UnhandledPromiseRejection if no one ever awaits this
      // session (e.g. user enters and immediately exits).
      sketchSessionReady.catch(() => {})
      // Fire the backend create. The server is the source of truth —
      // a `SketchCreated` WS frame will reconcile the snapshot, but
      // we also stamp `serverId` from the direct response in case the
      // broadcast is delayed.
      sketchApi
        .create(targetPlane, targetTool)
        .then(async (session) => {
          // If a newer `enterSketch` has superseded us, our resolver
          // identity won't match the module-level handle anymore.
          // Bail without touching the new session's queue.
          if (sketchSessionReadyResolve !== mySessionResolve) {
            return
          }
          // Drop the response if the user exited.
          const cur = useSceneStore.getState().sketch
          if (!cur.active) {
            mySessionReject?.(new Error('sketch exited before create resolved'))
            return
          }
          // Drain the pending-op queue *before* stamping serverId or
          // applying the snapshot. New actions taken during the drain
          // see serverId still null and so also queue (the outer
          // while-loop picks them up). When the queue is empty we
          // stamp serverId via `applyServerSketchSnapshot(lastSnap)`
          // and only then do further actions fire directly.
          let lastSnap: ServerSketchSession = session
          while (sketchPendingOps.length > 0) {
            const op = sketchPendingOps.shift()
            if (!op) break
            try {
              lastSnap = await replayPendingSketchOp(session.id, op)
            } catch (err) {
              console.error('[sketch] replay op failed:', op, err)
            }
            // A user exit during an awaited replay swaps our handles
            // out; bail before clobbering the new session's state.
            if (sketchSessionReadyResolve !== mySessionResolve) {
              return
            }
          }
          // Re-check active in case the user exited mid-drain.
          if (!useSceneStore.getState().sketch.active) {
            mySessionReject?.(new Error('sketch exited during drain'))
            return
          }
          useSceneStore.getState().applyServerSketchSnapshot(lastSnap)
          mySessionResolve?.(session.id)
        })
        .catch((err) => {
          console.error('[sketch] create failed:', err)
          if (sketchSessionReadyResolve === mySessionResolve) {
            mySessionReject?.(err)
          }
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
          shapes: [],
          points: [],
          hover: null,
          editingSourceObjectId: null,
          snapTarget: null,
          inferenceAxis: null,
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
      // Tear down the pending-op queue. Anything still waiting was
      // staged for a session the user just abandoned; rejecting the
      // readiness promise unblocks any `awaitSketchReady` consumer
      // (e.g. SketchPanel.handleFinish racing the user's Cancel).
      resetSketchPendingState('sketch exited')
    },

    setSketchTool: (tool) => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => {
        // Switching tools mid-sketch wipes the in-progress points; the
        // primitives don't share semantics (polyline = N points,
        // rectangle = 2 corners, circle = center+radius), so reusing
        // prior clicks would always be wrong. Mirror this onto the
        // last entry of `shapes` so the active-shape view stays
        // consistent.
        const shapes = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          tool,
          points: [],
        }))
        return {
          sketch: {
            ...state.sketch,
            tool,
            points: [],
            shapes,
            hover: state.sketch.hover,
            // Tool switch invalidates the previous anchor, so any
            // active inference axis is meaningless. Snap targets
            // recompute on the next pointermove.
            snapTarget: null,
            inferenceAxis: null,
          },
        }
      })
      if (id) {
        sketchApi.setTool(id, tool).catch((err) => {
          console.error('[sketch] setTool failed:', err)
        })
      } else {
        sketchPendingOps.push({ type: 'setTool', tool })
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
      set((state) => {
        // Backend `set_plane` clears every shape's points (plane swap
        // invalidates all UV). Mirror that locally.
        const shapes = state.sketch.shapes.map((s) => ({
          ...s,
          points: [],
        }))
        return {
          sketch: {
            ...state.sketch,
            plane,
            points: [],
            shapes,
            hover: null,
            snapTarget: null,
            inferenceAxis: null,
          },
          cameraPreset: presetKey,
          pendingCameraPreset: preset ?? state.pendingCameraPreset,
        }
      })
      if (id) {
        sketchApi.setPlane(id, plane).catch((err) => {
          console.error('[sketch] setPlane failed:', err)
        })
      } else {
        sketchPendingOps.push({ type: 'setPlane', plane })
      }
    },

    addSketchPoint: (point) => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => {
        const points = [...state.sketch.points, point]
        const shapes = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          points,
        }))
        return { sketch: { ...state.sketch, points, shapes } }
      })
      if (id) {
        sketchApi.addPoint(id, point).catch((err) => {
          console.error('[sketch] addPoint failed:', err)
        })
      } else {
        sketchPendingOps.push({ type: 'addPoint', point })
      }
    },

    popSketchPoint: () => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => {
        const points = state.sketch.points.slice(0, -1)
        const shapes = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          points,
        }))
        return { sketch: { ...state.sketch, points, shapes } }
      })
      if (id) {
        sketchApi.popPoint(id).catch((err) => {
          console.error('[sketch] popPoint failed:', err)
        })
      } else {
        sketchPendingOps.push({ type: 'popPoint' })
      }
    },

    clearSketchPoints: () => {
      const id = useSceneStore.getState().sketch.serverId
      set((state) => {
        const shapes = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          points: [],
        }))
        return { sketch: { ...state.sketch, points: [], shapes } }
      })
      if (id) {
        sketchApi.clearPoints(id).catch((err) => {
          console.error('[sketch] clearPoints failed:', err)
        })
      } else {
        sketchPendingOps.push({ type: 'clearPoints' })
      }
    },

    setSketchHover: (point) =>
      // Hover position is local-only UX — never sent to the backend.
      set((state) => ({ sketch: { ...state.sketch, hover: point } })),

    setSketchSnapState: ({ snapTarget, inferenceAxis }) =>
      // Snap target + inference axis are pure UX descriptors recomputed
      // every pointermove; the overlay reads them to draw the cyan ring
      // and dashed inference line.
      set((state) => ({
        sketch: { ...state.sketch, snapTarget, inferenceAxis },
      })),

    awaitSketchReady: () => {
      const cur = useSceneStore.getState().sketch
      if (cur.serverId !== null) {
        return Promise.resolve(cur.serverId)
      }
      if (sketchSessionReady) {
        return sketchSessionReady
      }
      return Promise.reject(new Error('no active sketch session'))
    },

    setSketchPoint: (index, point) => {
      const id = useSceneStore.getState().sketch.serverId
      let updated = false
      set((state) => {
        if (index < 0 || index >= state.sketch.points.length) return state
        const points = state.sketch.points.slice()
        points[index] = point
        const shapes = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          points,
        }))
        updated = true
        return { sketch: { ...state.sketch, points, shapes } }
      })
      if (updated) {
        if (id) {
          sketchApi.setPoint(id, index, point).catch((err) => {
            console.error('[sketch] setPoint failed:', err)
          })
        } else {
          sketchPendingOps.push({ type: 'setPoint', index, point })
        }
      }
    },

    setSketchView: (patch) =>
      // snapStep / measure / thickness are panel UX state; the backend
      // session has no equivalent fields, so this stays purely local.
      set((state) => ({ sketch: { ...state.sketch, ...patch } })),

    addNewSketchShape: (tool) => {
      const cur = useSceneStore.getState().sketch
      const id = cur.serverId
      // No-op when the active shape has no points: we'd just be
      // committing an empty shape and ending up with two trailing
      // empties. The user must place at least one point first.
      if (cur.points.length === 0) {
        return
      }
      const nextTool: SketchTool = tool ?? cur.tool
      set((state) => {
        const newShape: ServerSketchShape = {
          id: `pending-${Date.now()}`,
          tool: nextTool,
          points: [],
        }
        // Snapshot the active shape's current state into `shapes`
        // (so it stays in the committed list with its final points)
        // and append the empty new active.
        const committed = withActiveShapeUpdate(state.sketch.shapes, (s) => ({
          ...s,
          tool: state.sketch.tool,
          points: state.sketch.points,
        }))
        return {
          sketch: {
            ...state.sketch,
            shapes: [...committed, newShape],
            tool: nextTool,
            points: [],
            hover: null,
            snapTarget: null,
            inferenceAxis: null,
          },
        }
      })
      if (id) {
        sketchApi
          .addShape(id, { tool: nextTool })
          .then((session) => {
            useSceneStore.getState().applyServerSketchSnapshot(session)
          })
          .catch((err) => {
            console.error('[sketch] addShape failed:', err)
          })
      } else {
        sketchPendingOps.push({ type: 'addShape', tool: nextTool })
      }
    },

    deleteSketchShape: (idx) => {
      const cur = useSceneStore.getState().sketch
      const id = cur.serverId
      // Refuse to remove the only remaining shape — matches backend.
      if (cur.shapes.length <= 1) {
        console.warn('[sketch] deleteSketchShape: cannot remove the last shape')
        return
      }
      if (idx < 0 || idx >= cur.shapes.length) return
      set((state) => {
        const next = state.sketch.shapes.slice()
        next.splice(idx, 1)
        const last = next[next.length - 1]
        return {
          sketch: {
            ...state.sketch,
            shapes: next,
            // The new active shape's tool/points become the top-
            // level convenience view.
            tool: last.tool,
            points: last.points,
            hover: null,
          },
        }
      })
      if (id) {
        sketchApi
          .deleteShape(id, idx)
          .then((session) => {
            useSceneStore.getState().applyServerSketchSnapshot(session)
          })
          .catch((err) => {
            console.error('[sketch] deleteShape failed:', err)
          })
      } else {
        sketchPendingOps.push({ type: 'deleteShape', idx })
      }
    },

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
        // id and accept the server's view of shapes/plane. Active
        // shape = last entry; promote its tool/points to the
        // convenience view so existing UI keeps working.
        //
        // Defensive: a session deserialised from an older format (or
        // a partial WS frame the bridge let through) may be missing
        // `shapes` entirely. Reading `.length` on `undefined` throws
        // and the rejection propagates through `awaitSketchReady` as
        // "Sketch session unavailable: can't access property length".
        // Mirrors the same defense in `ModelTree.tsx:399`.
        const shapes = Array.isArray(session.shapes) ? session.shapes : []
        const last = shapes.length > 0 ? shapes[shapes.length - 1] : undefined
        return {
          serverSketches,
          sketch: {
            ...state.sketch,
            serverId: session.id,
            plane: session.plane,
            shapes,
            tool: last ? last.tool : state.sketch.tool,
            points: last ? last.points : [],
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

      // Drop any pending-op queue / readiness promise from a prior
      // (now superseded) `enterSketch`. The edit path stamps serverId
      // synchronously below, so there's nothing for the queue to
      // hold; clearing it prevents stale ops from leaking into the
      // edited session.
      resetSketchPendingState('switched to existing sketch')

      const shapes = session!.shapes
      const last = shapes[shapes.length - 1]
      set((state) => ({
        sketch: {
          ...state.sketch,
          active: true,
          serverId: session!.id,
          plane: session!.plane,
          shapes,
          tool: last ? last.tool : state.sketch.tool,
          points: last ? last.points : [],
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
