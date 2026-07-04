import { create, type StateCreator } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import type * as THREE from 'three'
import {
  sketchApi,
  type ServerSketchSession,
  type ServerSketchShape,
  type SketchRegion,
} from '@/lib/sketch-api'
import {
  csketchApi,
  type AddCircleRequest,
  type AddLineRequest,
  type AddPointRequest,
  type Constraint,
  type CSketchSummary,
  type DofReport,
  type DragRequest,
  type SketchSolveReport,
  type SolveOptions,
} from '@/lib/csketch-api'
import type { SectionCapMesh } from '@/lib/section-api'
import type { DimensionRow } from '@/lib/measure-api'

// Re-exported so consumers (panels, overlays) can import the
// SketchShape wire type from the same module they import the rest
// of the sketch types from.
export type { ServerSketchShape, SketchRegion } from '@/lib/sketch-api'

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
  /**
   * Kernel-local integer id for the solid (matches `solid_id` on the
   * wire). Used by `PartDimensions` to call
   * `GET /api/agent/parts/{solidId}/dimensions` without knowing the
   * backend part UUID — the kernel dimensions endpoint is keyed on
   * this integer, matching how drawings and the agent tools address it.
   *
   * Optional as an honest type: today the backend ships `solid_id` on
   * every `analytical_geometry` payload, but a future shape that omits
   * it must surface as `undefined` at the type level (consumers already
   * guard with `?.solidId !== undefined`), not as a lying `number`.
   */
  solidId?: number
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

// ─── Sketch session readiness ────────────────────────────────────────
//
// `enterSketch` fires `sketchApi.create` asynchronously and the
// resulting `serverId` only lands when the response arrives. Every
// mutation action awaits this readiness promise before dispatching its
// REST call — there is no local op queue, and there is no optimistic
// local mutation. The backend is the only source of truth for sketch
// geometry: state updates flow back exclusively through the
// `SketchUpdated` WebSocket echo → `applyServerSketchSnapshot`. This
// matches the "frontend is a thin display layer" architectural rule.
//
// Module-level state because there's at most one in-flight create
// per browser tab.
let sketchSessionReady: Promise<string> | null = null
let sketchSessionReadyResolve: ((id: string) => void) | null = null
let sketchSessionReadyReject: ((err: unknown) => void) | null = null

/**
 * Arm a fresh readiness promise for a new `enterSketch`. Rejects any
 * prior in-flight promise so consumers waiting on a now-abandoned
 * session fail fast rather than racing against the new one. Returns
 * the resolve/reject pair for the *current* session — callers compare
 * identity against the module-level handles to detect supersession.
 */
function armSketchReady(): {
  resolve: (id: string) => void
  reject: (err: unknown) => void
} {
  if (sketchSessionReadyReject) {
    sketchSessionReadyReject(new Error('sketch session superseded'))
  }
  let resolve!: (id: string) => void
  let reject!: (err: unknown) => void
  sketchSessionReady = new Promise<string>((res, rej) => {
    resolve = res
    reject = rej
  })
  // Suppress UnhandledPromiseRejection if nothing ever awaits this.
  sketchSessionReady.catch(() => {})
  sketchSessionReadyResolve = resolve
  sketchSessionReadyReject = reject
  return { resolve, reject }
}

/**
 * Reject and drop the readiness promise — called by `exitSketch` and
 * the supersession path in `enterSketch`.
 */
function clearSketchReady(reason: string): void {
  if (sketchSessionReadyReject) {
    sketchSessionReadyReject(new Error(reason))
  }
  sketchSessionReady = null
  sketchSessionReadyResolve = null
  sketchSessionReadyReject = null
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
   * server response landing — every mutation action awaits
   * `awaitSketchReady` before dispatching, so callers never need to
   * special-case the null window.
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
  /**
   * Server-authoritative region decomposition for the active sketch
   * (Slice D). Each entry pairs an outer-shape index with the indices
   * of the shapes the kernel will subtract as holes — i.e. exactly
   * what `extrude_sketch` will pass to `extrude_face`. Populated by
   * the `SketchRegionsUpdated` WS frame, refreshed on every shape /
   * point mutation. Empty when the sketch has no extrudable shapes
   * (and also empty when `regionError` is non-null).
   */
  regions: SketchRegion[]
  /**
   * Kernel diagnostic for the region classifier — `null` on success,
   * a short message (e.g. "island-in-hole nesting not supported")
   * when the layout is unextrudable. The overlay surfaces this so the
   * user knows hovering Extrude won't succeed.
   */
  regionError: string | null
  /**
   * Pure UX flag set by `onPointerEnter`/`onPointerLeave` on the
   * Finish & Extrude button. When `active`, the in-canvas overlay
   * paints the region preview (blue outer loops, red holes) so the
   * user sees "what will be extruded" before committing. Never
   * serialised; lives purely in the local store.
   */
  extrudeHover: { active: boolean }
}

// ─── Constrained sketch (csketch) state ──────────────────────────────
//
// The constrained sketch is the parametric Newton-Raphson sketcher
// (kernel `Sketch` over `/api/csketch/*`). It is intentionally a
// separate state slice from the click-to-place `sketch` above: the
// two surfaces speak different REST APIs, hold different entity
// shapes, and serve different user flows. Bundling them would force
// every consumer to discriminate on a "mode" tag at every access.
//
// The frontend never mutates a csketch optimistically. Every action
// dispatches a REST call and then folds the response (or a follow-up
// `get` / `listConstraints`) into the slice. WebSocket sync for
// csketches is not wired in this slice — the editing surface is
// strictly request/response — so a peer-side edit will not surface
// until the local view explicitly refreshes. Slice N-3 / D-3 will
// revisit this once the editor has live-collaboration value.
/**
 * Drawing tool inside the constrained sketcher. Mirrors the legacy
 * click-to-place `SketchTool` but speaks the csketch vocabulary
 * (`point` / `line` / `circle` map to `POST /point` / `/line` /
 * `/circle` respectively).
 *
 * `null` is the idle / "no draw tool active" state — the overlay
 * falls through to the legacy capture-plane click handler when
 * idle. Set via `setCSketchTool`; cleared on `closeCSketch` /
 * `deleteCSketch` (active).
 */
export type CSketchTool = 'point' | 'line' | 'circle'

export interface CSketchState {
  /**
   * Every known csketch keyed by server id, populated by
   * `refreshCSketches`, `createCSketch`, and per-mutation refreshes.
   * Used by the model tree / pickers to list every live csketch
   * without forcing the caller to hold a separate cache.
   */
  summaries: Map<string, CSketchSummary>
  /**
   * Server id of the csketch the editor is currently bound to.
   * `null` when no csketch is open. Mutations target this id; the
   * caller must `openCSketch(id)` before they can dispatch.
   */
  activeId: string | null
  /**
   * Constraints of the active csketch. Mirrors `GET
   * /api/csketch/{id}/constraints`. Empty when no csketch is open
   * or the active one has zero constraints.
   */
  activeConstraints: Constraint[]
  /**
   * Most recent solver verdict on the active csketch, or `null`
   * when the editor has not solved since opening it. Carries the
   * status (converged / under-/over-constrained / unstable), the
   * residuals, and the wall-clock cost. Updated by every
   * `solveCSketch` and `dragCSketch` call; also updated by
   * `updateCSketchConstraintValue` on the success path (the server
   * returns the full report in the same response).
   */
  lastReport: SketchSolveReport | null
  /**
   * Structural-DOF analysis for the active csketch, or `null` when
   * the editor has not requested one. Cheap (no Newton-Raphson),
   * suitable for refresh on every constraint add / remove.
   */
  lastDofReport: DofReport | null
  /**
   * Active draw tool for the csketch editor, or `null` when no
   * tool is selected (the legacy capture-plane handler then
   * processes clicks normally). Set by the SketchPanel tool row
   * (D-2-b). The overlay reads this on every pointer down: when
   * non-null, clicks are routed through the csketch REST surface
   * (`addPoint` / `addLine` / `addCircle`) instead of the legacy
   * sketch handler.
   */
  activeTool: CSketchTool | null
}

// ─── Pinned measurements ─────────────────────────────────────────────
/**
 * One user-pinned interactive measurement (spec section 3, "Interactive
 * measure"). Session-local only — never persisted kernel-side.
 *
 * `a` / `b` record which faces were measured (`faceId` is the kernel
 * `FaceId`, the same value `SubElementSelection.index` carries in face
 * mode) so the pin can be re-validated against `/api/agent/measure`
 * whenever the geometry changes. `b` is `null` for single-face
 * measurements (e.g. a lone cylindrical face measuring its diameter).
 *
 * `row` is the kernel's DimensionRecord-shaped result verbatim; the
 * viewport renders it with the same annotation component as ambient
 * dimensions, in a distinct accent tone with a dismiss control.
 */
export interface PinnedMeasurement {
  /** Client-generated UUID identifying this pin. */
  id: string
  a: { objectId: string; faceId: number }
  b: { objectId: string; faceId: number } | null
  row: DimensionRow
}

// ─── Dimension kind filter ───────────────────────────────────────────
/**
 * The chip-filterable dimension kinds (ui-units polish, spec section 4).
 * `diameter` also covers kernel `radius` rows (one Ø chip); kinds not
 * listed here (angles, interactive-measure kinds) are never filtered.
 * Shared by the scene store default, the `DimensionKindChips` UI, and
 * the `PartDimensions` row filter so all three stay in lock-step.
 */
export const ALL_DIMENSION_KINDS = [
  'extent',
  'diameter',
  'length',
  'position',
] as const

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

  /**
   * Whether the LABELLER overlay is shown in the viewport — named
   * billboard callouts anchored at each label's world point (see
   * `PartLabels`). Off by default so the viewport stays clean; the user
   * toggles it from the viewport controls when they want to read (or
   * frame a question around) the part's named features.
   */
  labelsVisible: boolean

  /**
   * Set of object ids whose kernel dimension table is currently visible
   * in the viewport as leader + billboard annotations (see
   * `PartDimensions`). Empty by default. Uses Set replacement (never
   * mutation) matching the `selectedIds` pattern.
   */
  showDimensions: Set<string>

  /**
   * Which dimension-kind chips are visible in the dimension layer.
   * Members are chip keys from {@link ALL_DIMENSION_KINDS}; default is
   * all-on. Kinds not governed by a chip (angles, measure results) are
   * always visible. Reset to all-on when the layer empties
   * (`removeObject` dropping the last dimensioned object, `clearScene`)
   * so a new session never inherits a half-filtered view. Set
   * replacement, never mutation.
   */
  dimensionKindFilter: Set<string>

  /**
   * User-pinned interactive measurements. See {@link PinnedMeasurement}.
   * Session-local; re-validated on geometry change (dismissed with a
   * chat note when the measured faces no longer resolve — a stale
   * number is worse than none). Array replacement, never mutation.
   */
  pinnedMeasurements: PinnedMeasurement[]

  /** Cutting-plane state for Section View. See {@link SectionViewState}. */
  sectionView: SectionViewState

  /**
   * Triangulated cross-section caps keyed by the parent solid's id
   * (which IS the kernel UUID — see ws-bridge.ts::ObjectCreated).
   * Populated on demand by `CADViewport`'s section effect whenever
   * the plane state changes; cleared whenever section view is turned
   * off. One entry per cap loop, so a single solid that the plane
   * crosses in two disjoint places contributes two entries — the map
   * stores `SectionCapMesh[]` to preserve that multiplicity.
   */
  sectionCaps: Map<string, SectionCapMesh[]>

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
  /** Sketch ids the user has hidden from the model tree. Parallels
   *  `CADObject.visible` — a committed sketch is a first-class, HIDEABLE
   *  scene entity (not a transient session), so it gets the same show/hide
   *  affordance a solid has. `ServerSketches` skips ids in this set. */
  hiddenSketchIds: Set<string>

  // Three.js refs (set by canvas components)
  sceneRef: THREE.Scene | null
  cameraRef: THREE.Camera | null
  glRef: THREE.WebGLRenderer | null

  // Actions
  addObject: (obj: CADObject) => void
  updateObject: (id: string, patch: Partial<CADObject>) => void
  setObjectColor: (id: string, rgb: [number, number, number]) => void
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
  /** Toggle the LABELLER viewport overlay on/off. */
  toggleLabelsVisible: () => void
  /**
   * Toggle the dimension layer for a single object on or off. Adds the
   * id to `showDimensions` when absent; removes it when present. Uses
   * Set replacement (never mutation), matching the `selectedIds` pattern.
   */
  toggleDimensions: (objectId: string) => void
  /**
   * Toggle one dimension-kind chip on/off. `kind` is a member of
   * {@link ALL_DIMENSION_KINDS}. Set replacement, never mutation.
   */
  toggleDimensionKind: (kind: string) => void
  /** Append a pinned measurement (id must be unique — caller generates it). */
  pinMeasurement: (measurement: PinnedMeasurement) => void
  /** Remove a pinned measurement by pin id. No-op when absent. */
  dismissMeasurement: (id: string) => void
  /**
   * Replace a pin's kernel result row in place (same pin identity,
   * fresh value/anchor). Used by re-validation after a geometry change
   * — a successful re-measure updates the annotation rather than
   * dismiss-and-re-pin, so the pin's id (and React key) stays stable.
   */
  updatePinnedMeasurement: (id: string, row: DimensionRow) => void
  setSectionView: (settings: Partial<SectionViewState>) => void
  toggleSectionView: () => void
  /** Replace the live cap set with the result of the most recent
   *  section-preview fetch. Caps grouped by parent solid id. */
  setSectionCaps: (caps: SectionCapMesh[]) => void
  /** Drop every cap. Called when section view is turned off so the
   *  viewport stops drawing stale geometry. */
  clearSectionCaps: () => void
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
   * Resolve once the active sketch's `serverId` is known. Resolves
   * immediately with the id when serverId is already set; rejects
   * when the create round-trip failed or the user exited before it
   * landed. Every mutation action awaits this before dispatching its
   * REST call, and the Finish handler awaits it so a fast user can't
   * extrude before the session lands.
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
   * frame, and once directly from the `enterSketch` create-response
   * path in case the WS echo is delayed. This is the **sole writer**
   * of sketch geometry state — mutation actions (`addSketchPoint`,
   * `setSketchTool`, …) never touch state directly; they dispatch a
   * REST call and rely on the resulting WS echo to reach this method.
   */
  applyServerSketchSnapshot: (session: ServerSketchSession) => void
  /**
   * Apply a `SketchRegionsUpdated` payload to the local store. Only
   * updates the active session's `regions` / `regionError`; payloads
   * for other sessions are dropped (the overlay never paints them).
   */
  applySketchRegions: (payload: {
    sketch_id: string
    regions: SketchRegion[]
    region_error: string | null
  }) => void
  /**
   * Toggle the extrude-hover overlay. Pure UX state; wired to
   * `onPointerEnter`/`onPointerLeave` on the Finish & Extrude button.
   */
  setExtrudeHover: (active: boolean) => void
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
  /** Show/hide a committed sketch (model-tree visibility toggle). */
  toggleSketchVisibility: (id: string) => void
  /**
   * Re-enter an existing server-side sketch as the active editing
   * session. Used by the model tree's "Edit sketch" context menu.
   * Sets the camera preset to look face-on at the plane and seeds
   * the local `sketch.points` from the server snapshot.
   */
  editServerSketch: (id: string) => void

  // ─── Constrained sketch (csketch) ──────────────────────────────────
  /**
   * The constrained-sketch slice. See {@link CSketchState}. Mutations
   * flow exclusively through the `*CSketch*` actions below — never
   * patch this object directly.
   */
  csketch: CSketchState
  /**
   * Create a fresh empty csketch on the server, refresh the summary
   * map, and make it the active csketch. Returns the new id so
   * callers can chain follow-up dispatches without re-reading state.
   */
  createCSketch: () => Promise<string>
  /**
   * Load an existing csketch as active. Fetches the entity summary
   * and the full constraint list in parallel; clears `lastReport`
   * and `lastDofReport` because both belonged to the previous
   * active csketch. Throws if the server returns 404.
   */
  openCSketch: (id: string) => Promise<void>
  /**
   * Clear `activeId`, `activeConstraints`, `lastReport`,
   * `lastDofReport`. Does not delete the csketch on the server.
   */
  closeCSketch: () => void
  /**
   * Delete a csketch on the server, drop it from `summaries`, and —
   * if it was the active one — clear all active-csketch state.
   */
  deleteCSketch: (id: string) => Promise<void>
  /**
   * Re-fetch every live csketch id from the server and replace the
   * `summaries` map. Each id is paid one `GET /csketch/{id}` so the
   * map stays full-fidelity. Intended as a connect-time hydration
   * call, not a hot-path action.
   */
  refreshCSketches: () => Promise<void>
  /**
   * Re-fetch a single csketch's summary (and, when it is the active
   * one, its constraints). Cheaper than `refreshCSketches` for
   * post-mutation reconciliation.
   */
  refreshCSketch: (id: string) => Promise<void>
  /**
   * Add a point to the csketch identified by `id`. Returns the new
   * point's UUID once the round-trip resolves; the summary is
   * refreshed before this resolves so observers reading state see
   * the new point.
   */
  addCSketchPoint: (id: string, body: AddPointRequest) => Promise<string>
  /** Add a line segment between two existing points. */
  addCSketchLine: (id: string, body: AddLineRequest) => Promise<string>
  /** Add a circle by centre + radius. */
  addCSketchCircle: (id: string, body: AddCircleRequest) => Promise<string>
  /**
   * Add a fully-formed kernel constraint. The caller is responsible
   * for filling in `id` (typically `crypto.randomUUID()`), the
   * `constraint_type`, the involved `entities`, the `priority`, and
   * the initial `status`. `name` may be `null`.
   */
  addCSketchConstraint: (id: string, constraint: Constraint) => Promise<string>
  /** Remove a constraint by id. */
  deleteCSketchConstraint: (id: string, cid: string) => Promise<void>
  /**
   * Edit the scalar target of a dimensional constraint and re-solve.
   * The success response carries an up-to-date summary and the
   * solver report, both of which are folded into the slice. On a
   * 409 the server has already reverted; the typed
   * `CSketchConstraintConflictError` thrown by the API client is
   * re-thrown so the caller can surface it in the UI.
   */
  updateCSketchConstraintValue: (
    id: string,
    cid: string,
    value: number,
  ) => Promise<SketchSolveReport>
  /**
   * Run the Newton-Raphson solver. Pass `undefined` to use server
   * defaults (`max_iterations=100`, `tolerance=1e-10`,
   * `damping_factor=0.5`). The report is stored in `lastReport` and
   * the summary is refreshed (the solver may have moved entities).
   */
  solveCSketch: (id: string, options?: SolveOptions) => Promise<SketchSolveReport>
  /**
   * Drag a single entity toward a target while honouring every
   * other constraint. Stores the resulting report and refreshes the
   * summary.
   */
  dragCSketch: (id: string, body: DragRequest) => Promise<SketchSolveReport>
  /**
   * Run the cheap structural-DOF analysis (no Newton-Raphson). The
   * result is stored in `lastDofReport`. Safe to call after every
   * constraint add / remove for a reactive "DOF: 3" badge.
   */
  analyseCSketchDof: (id: string) => Promise<DofReport>
  /**
   * Set or clear the active csketch draw tool (D-2-b). Pass `null`
   * to return the editor to read-only / drag-only mode. Toggling
   * the same tool twice is a no-op at the caller's discretion —
   * the panel uses a click-to-toggle pattern.
   *
   * No round-trip; pure local state. Cleared automatically when
   * the active csketch closes / is deleted.
   */
  setCSketchTool: (tool: CSketchTool | null) => void

  setSceneRef: (scene: THREE.Scene | null) => void
  setCameraRef: (camera: THREE.Camera | null) => void
  setGlRef: (gl: THREE.WebGLRenderer | null) => void
}

// Explicit `StateCreator` annotation. Without it, TypeScript infers
// the returned object literal's type (with `objectOrder: never[]`
// etc.) instead of widening to `SceneState`, and every inner arrow
// method loses contextual parameter typing — producing the cascade
// of TS7006 "implicit any" errors. The middleware-tuple argument
// `[['zustand/subscribeWithSelector', never]]` tells Zustand the
// returned creator carries the `subscribe.withSelector` shape.
const sceneCreator: StateCreator<
  SceneState,
  [['zustand/subscribeWithSelector', never]]
> = (set, get) => ({
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
    labelsVisible: false,
    showDimensions: new Set<string>(),
    dimensionKindFilter: new Set<string>(ALL_DIMENSION_KINDS),
    pinnedMeasurements: [],
    sectionView: {
      enabled: false,
      axis: 'x',
      offset: 0,
      flipped: false,
    },
    sectionCaps: new Map(),
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
      regions: [],
      regionError: null,
      extrudeHover: { active: false },
    },
    serverSketches: new Map(),
    hiddenSketchIds: new Set(),
    csketch: {
      summaries: new Map(),
      activeId: null,
      activeConstraints: [],
      lastReport: null,
      lastDofReport: null,
      activeTool: null,
    },
    sceneRef: null,
    cameraRef: null,
    glRef: null,

    addObject: (obj) =>
      set((state) => {
        const objects = new Map(state.objects)
        const prior = objects.get(obj.id)
        const existed = prior !== undefined
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
        // Topology rebuild → invalidate stale face/edge/vertex picks.
        // The kernel reassigns FaceIds (monotonic `FaceStore.next_id`)
        // on every rebuild, so any sub-element selection captured
        // against the prior mesh now points at a different physical
        // face. Letting them persist makes the highlight overlay
        // appear to drift across faces as operations are applied.
        const meshChanged =
          existed && prior !== undefined && prior.mesh !== obj.mesh
        const subElementSelections = meshChanged
          ? state.subElementSelections.filter((s) => s.objectId !== obj.id)
          : state.subElementSelections
        const hoveredSubElement =
          meshChanged && state.hoveredSubElement?.objectId === obj.id
            ? null
            : state.hoveredSubElement
        return { objects, objectOrder, subElementSelections, hoveredSubElement }
      }),

    updateObject: (id, patch) =>
      set((state) => {
        const existing = state.objects.get(id)
        if (!existing) return state
        const objects = new Map(state.objects)
        const next = { ...existing, ...patch }
        objects.set(id, next)
        // Same rationale as `addObject`: when the patch swaps the
        // mesh (modify ops broadcast `ObjectUpdated` with a fresh
        // tessellation), face/edge/vertex indices captured against
        // the prior mesh are no longer meaningful.
        const meshChanged = patch.mesh !== undefined && patch.mesh !== existing.mesh
        const subElementSelections = meshChanged
          ? state.subElementSelections.filter((s) => s.objectId !== id)
          : state.subElementSelections
        const hoveredSubElement =
          meshChanged && state.hoveredSubElement?.objectId === id
            ? null
            : state.hoveredSubElement
        return { objects, subElementSelections, hoveredSubElement }
      }),

    // Recolour a live object from a backend `ObjectColor` broadcast
    // (`set_part_color` → `broadcast_object_color`). `rgb` is 0..=255 per
    // channel. CRITICAL: CADMesh's material `useMemo` is keyed on
    // `object.material`, so a NEW material object must be installed (not a
    // mutated `color` field) or the mesh never re-renders. We keep the rest
    // of the material (metalness/roughness/opacity) and swap only `color`,
    // expressed as the `#rrggbb` hex string the rest of the material path
    // (`convertMaterial` → `MeshStandardMaterial({ color })`) already uses.
    setObjectColor: (id, rgb) =>
      set((state) => {
        const existing = state.objects.get(id)
        if (!existing) return state
        const clamp = (n: number) =>
          Math.max(0, Math.min(255, Math.round(n)))
        const hex = `#${rgb
          .map((c) => clamp(c).toString(16).padStart(2, '0'))
          .join('')}`
        const objects = new Map(state.objects)
        objects.set(id, {
          ...existing,
          material: { ...existing.material, color: hex },
        })
        return { objects }
      }),

    removeObject: (id) =>
      set((state) => {
        const objects = new Map(state.objects)
        objects.delete(id)
        const selectedIds = new Set(state.selectedIds)
        selectedIds.delete(id)
        // Dimension-layer lifecycle: a deleted object must not leave its
        // id in `showDimensions` (stale-set leak; a reused id would
        // inherit a toggled-on state it never asked for), and any pinned
        // measurement referencing it is dead — re-validation would only
        // discover that later; drop it now.
        const showDimensions = new Set(state.showDimensions)
        showDimensions.delete(id)
        const pinnedMeasurements = state.pinnedMeasurements.filter(
          (p) => p.a.objectId !== id && p.b?.objectId !== id,
        )
        // Kind filter is layer-global (not per-object), so per-id
        // cleanup doesn't apply — but when the last dimensioned object
        // leaves, reset to all-on so the next session never inherits a
        // half-filtered view from a scene that no longer exists.
        const dimensionKindFilter =
          showDimensions.size === 0
            ? new Set<string>(ALL_DIMENSION_KINDS)
            : state.dimensionKindFilter
        return {
          objects,
          objectOrder: state.objectOrder.filter((oid) => oid !== id),
          selectedIds,
          hoveredId: state.hoveredId === id ? null : state.hoveredId,
          showDimensions,
          pinnedMeasurements,
          dimensionKindFilter,
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
        hiddenSketchIds: new Set(),
        showDimensions: new Set(),
        dimensionKindFilter: new Set<string>(ALL_DIMENSION_KINDS),
        pinnedMeasurements: [],
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

    toggleLabelsVisible: () =>
      set((state) => ({ labelsVisible: !state.labelsVisible })),

    toggleDimensions: (objectId) =>
      set((state) => {
        const next = new Set(state.showDimensions)
        if (next.has(objectId)) {
          next.delete(objectId)
        } else {
          next.add(objectId)
        }
        return { showDimensions: next }
      }),

    toggleDimensionKind: (kind) =>
      set((state) => {
        const next = new Set(state.dimensionKindFilter)
        if (next.has(kind)) {
          next.delete(kind)
        } else {
          next.add(kind)
        }
        return { dimensionKindFilter: next }
      }),

    pinMeasurement: (measurement) =>
      set((state) => ({
        pinnedMeasurements: [...state.pinnedMeasurements, measurement],
      })),

    dismissMeasurement: (id) =>
      set((state) => ({
        pinnedMeasurements: state.pinnedMeasurements.filter(
          (p) => p.id !== id,
        ),
      })),

    updatePinnedMeasurement: (id, row) =>
      set((state) => ({
        pinnedMeasurements: state.pinnedMeasurements.map((p) =>
          p.id === id ? { ...p, row } : p,
        ),
      })),

    setSectionView: (settings) =>
      set((state) => ({
        sectionView: { ...state.sectionView, ...settings },
      })),

    toggleSectionView: () =>
      set((state) => ({
        sectionView: { ...state.sectionView, enabled: !state.sectionView.enabled },
      })),

    setSectionCaps: (caps) =>
      set(() => {
        const grouped = new Map<string, SectionCapMesh[]>()
        for (const cap of caps) {
          const bucket = grouped.get(cap.solidId)
          if (bucket) {
            bucket.push(cap)
          } else {
            grouped.set(cap.solidId, [cap])
          }
        }
        return { sectionCaps: grouped }
      }),

    clearSectionCaps: () => set({ sectionCaps: new Map() }),

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
          // Seed an empty shape so dimension-input / overlay code that
          // reads `shapes[last]` has a valid placeholder until the
          // `SketchCreated` WS frame lands. WS echo will replace this
          // with the canonical session shapes.
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
          // Region preview is server-authoritative — clear until the
          // first `SketchRegionsUpdated` frame for this session lands.
          regions: [],
          regionError: null,
          extrudeHover: { active: false },
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
      // Arm a fresh readiness promise. Captures the resolve/reject
      // pair for *this* session; later supersession (user enters a
      // second sketch before this create resolves) replaces the
      // module-level handles, and the identity check below lets us
      // bail without clobbering the new session.
      const { resolve: myResolve, reject: myReject } = armSketchReady()
      // Fire the backend create. Server is source of truth — the
      // `SketchCreated` WS frame will reconcile shapes/plane via
      // `applyServerSketchSnapshot`. We also apply the direct response
      // here in case the broadcast is delayed by the network.
      sketchApi
        .create(targetPlane, targetTool)
        .then((session) => {
          // Supersession guard: if a newer `enterSketch` has run,
          // our resolver identity no longer matches the module-level
          // handle. Drop the response on the floor.
          if (sketchSessionReadyResolve !== myResolve) {
            return
          }
          if (!useSceneStore.getState().sketch.active) {
            myReject(new Error('sketch exited before create resolved'))
            return
          }
          useSceneStore.getState().applyServerSketchSnapshot(session)
          myResolve(session.id)
        })
        .catch((err) => {
          console.error('[sketch] create failed:', err)
          if (sketchSessionReadyResolve === myResolve) {
            myReject(err)
          }
        })
    },

    exitSketch: (options) => {
      const cur = useSceneStore.getState().sketch
      const id = cur.serverId
      // Default delete only an EMPTY, standalone session. Two cases must
      // survive an implicit exit:
      //  - a sketch reopened from a feature ("Edit sketch") — the backend
      //    session IS that feature's profile; deleting it orphans the solid.
      //  - a standalone sketch that has DRAWN CONTENT — a finished/closed
      //    sketch must persist as a visible curve (the generating profile),
      //    not vanish the moment the panel closes. Previously any standalone
      //    session was deleted on exit, so a drawn curve disappeared on finish.
      // A caller that genuinely wants to wipe the session passes
      // `deleteBackend: true` explicitly (the Cancel/discard path).
      const hasContent = cur.shapes.some((s) => s.points.length > 0)
      const defaultDelete = cur.editingSourceObjectId === null && !hasContent
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
          regions: [],
          regionError: null,
          extrudeHover: { active: false },
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
      // Reject the readiness promise so any in-flight `awaitSketchReady`
      // consumer (e.g. SketchPanel.handleFinish racing the user's
      // Cancel, or a click-handler still resolving its REST dispatch)
      // fails fast rather than touching a now-defunct session.
      clearSketchReady('sketch exited')
    },

    setSketchTool: (tool) => {
      const state0 = useSceneStore.getState().sketch
      // If the active shape already carries enough points to be a
      // valid loop on its own, switching tools should commit it as a
      // finished shape and start a fresh empty shape with the new
      // tool — not discard the user's work. Mirrors the multi-shape
      // flow in SketchOverlay: the sketch session keeps rolling
      // forward and only ends on explicit Finish / Cancel.
      //
      // Threshold per tool matches the click-handler's auto-commit:
      // polyline needs ≥3 points (the minimum non-degenerate
      // polygon); rectangle / circle need both anchor points.
      const activeTool = state0.tool
      const activeCount = state0.points.length
      const activeIsValid =
        (activeTool === 'polyline' && activeCount >= 3) ||
        ((activeTool === 'rectangle' || activeTool === 'circle') &&
          activeCount >= 2)
      if (activeIsValid && tool !== activeTool) {
        // Commit the current shape via `addNewSketchShape`, which the
        // backend implements as "snapshot active shape, append fresh
        // empty shape with `tool`". WS echo carries the new state.
        useSceneStore.getState().addNewSketchShape(tool)
        return
      }
      // Tool switch invalidates the previous anchor: drop snap /
      // inference axis locally (pure UX state, never round-tripped).
      // The shapes/points fields stay untouched here — backend's
      // setTool clears the active shape's points and the WS echo
      // propagates that authoritatively.
      set((state) => ({
        sketch: {
          ...state.sketch,
          snapTarget: null,
          inferenceAxis: null,
        },
      }))
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.setTool(id, tool)
        } catch (err) {
          console.error('[sketch] setTool failed:', err)
        }
      })()
    },

    setSketchPlane: (plane) => {
      // Mirror enterSketch: re-orient the camera to face the new plane
      // so the sketcher always sees the surface flat-on. Standard
      // planes hit one of the axis presets; custom (face-anchored)
      // planes synthesise a fresh preset along the face normal — see
      // sketchCameraSetup. Camera preset is pure UX state and lives
      // only on the client.
      const { presetKey, preset } = sketchCameraSetup(plane)
      set((state) => ({
        sketch: {
          ...state.sketch,
          // Clear transient UX state (cursor preview / snap / inference)
          // because their values reference the old plane's coordinate
          // frame. Authoritative shapes/plane/points fields wait for
          // the WS echo from the backend's `set_plane`.
          hover: null,
          snapTarget: null,
          inferenceAxis: null,
        },
        cameraPreset: presetKey,
        pendingCameraPreset: preset ?? state.pendingCameraPreset,
      }))
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.setPlane(id, plane)
        } catch (err) {
          console.error('[sketch] setPlane failed:', err)
        }
      })()
    },

    addSketchPoint: (point) => {
      // Pure backend dispatch — state lands via `SketchUpdated` WS
      // echo, never written locally here. The redraw lag is the WS
      // round-trip (~5-15 ms on localhost) and keeps the kernel as
      // the single source of truth.
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.addPoint(id, point)
        } catch (err) {
          console.error('[sketch] addPoint failed:', err)
        }
      })()
    },

    popSketchPoint: () => {
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.popPoint(id)
        } catch (err) {
          console.error('[sketch] popPoint failed:', err)
        }
      })()
    },

    clearSketchPoints: () => {
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.clearPoints(id)
        } catch (err) {
          console.error('[sketch] clearPoints failed:', err)
        }
      })()
    },

    setSketchHover: (point) =>
      // Hover position is local-only UX — never sent to the backend.
      set((state) => ({ sketch: { ...state.sketch, hover: point } })),

    applySketchRegions: (payload) =>
      // Mirror the backend's region decomposition for the active
      // session only. Peer-edited sketches push their own frames; we
      // drop them because the overlay only ever paints the user's
      // current edit. When the local sketch hasn't yet acquired its
      // `serverId` (the create round-trip is mid-flight) we also drop
      // — the next snapshot from `applyServerSketchSnapshot` will be
      // followed by a fresh regions frame for the live session.
      set((state) => {
        if (state.sketch.serverId !== payload.sketch_id) {
          return state
        }
        return {
          sketch: {
            ...state.sketch,
            regions: payload.regions,
            regionError: payload.region_error,
          },
        }
      }),

    setExtrudeHover: (active) =>
      // Pure UX toggle for the in-canvas region preview. Wired to
      // the Finish & Extrude button's pointer-enter/leave.
      set((state) => ({
        sketch: { ...state.sketch, extrudeHover: { active } },
      })),

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
      // Bounds check against the local mirror of the active shape so
      // dimension-input UI doesn't fire a guaranteed-to-fail REST
      // call for an out-of-range index. The authoritative update
      // still arrives via the WS echo.
      const cur = useSceneStore.getState().sketch
      if (index < 0 || index >= cur.points.length) {
        return
      }
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.setPoint(id, index, point)
        } catch (err) {
          console.error('[sketch] setPoint failed:', err)
        }
      })()
    },

    setSketchView: (patch) =>
      // snapStep / measure / thickness are panel UX state; the backend
      // session has no equivalent fields, so this stays purely local.
      set((state) => ({ sketch: { ...state.sketch, ...patch } })),

    addNewSketchShape: (tool) => {
      const cur = useSceneStore.getState().sketch
      // No-op when the active shape has no points: we'd commit an
      // empty shape and end up with two trailing empties. Backend
      // refuses this case too, but checking locally avoids the
      // round-trip. The shape count read here is from the latest WS
      // snapshot, which is the authoritative view.
      if (cur.points.length === 0) {
        return
      }
      const nextTool: SketchTool = tool ?? cur.tool
      // Clear transient UX state (hover / snap / inference). Shapes
      // and tool fields wait for the WS echo from `addShape`.
      set((state) => ({
        sketch: {
          ...state.sketch,
          hover: null,
          snapTarget: null,
          inferenceAxis: null,
        },
      }))
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.addShape(id, { tool: nextTool })
        } catch (err) {
          console.error('[sketch] addShape failed:', err)
        }
      })()
    },

    deleteSketchShape: (idx) => {
      const cur = useSceneStore.getState().sketch
      // Refuse to remove the only remaining shape — matches backend.
      // Skipping the round-trip on guaranteed-invalid input keeps
      // server logs clean.
      if (cur.shapes.length <= 1) {
        console.warn('[sketch] deleteSketchShape: cannot remove the last shape')
        return
      }
      if (idx < 0 || idx >= cur.shapes.length) return
      void (async () => {
        try {
          const id = await useSceneStore.getState().awaitSketchReady()
          await sketchApi.deleteShape(id, idx)
        } catch (err) {
          console.error('[sketch] deleteShape failed:', err)
        }
      })()
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
            // Region cache is keyed to a specific server id; the next
            // session's `SketchRegionsUpdated` will repopulate.
            regions: [],
            regionError: null,
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

    toggleSketchVisibility: (id) =>
      set((state) => {
        const hiddenSketchIds = new Set(state.hiddenSketchIds)
        if (hiddenSketchIds.has(id)) hiddenSketchIds.delete(id)
        else hiddenSketchIds.add(id)
        return { hiddenSketchIds }
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
      clearSketchReady('switched to existing sketch')

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

    // ─── Constrained sketch actions ─────────────────────────────────
    //
    // Every mutating action follows the same shape: dispatch the REST
    // call, then refresh the local mirror from the server's response
    // (either the response body itself when it carries a `summary` —
    // `updateCSketchConstraintValue` does — or a follow-up GET).
    // There is no optimistic local mutation; the backend is the only
    // source of truth, matching the architectural rule the
    // click-to-place sketch follows.
    createCSketch: async () => {
      const { id } = await csketchApi.create()
      const summary = await csketchApi.get(id)
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, summary)
        return {
          csketch: {
            ...state.csketch,
            summaries,
            activeId: id,
            activeConstraints: [],
            lastReport: null,
            lastDofReport: null,
          },
        }
      })
      return id
    },

    openCSketch: async (id) => {
      // Fetch summary + constraints in parallel — they are
      // independent endpoints and the round-trip latency dominates
      // either call on its own.
      const [summary, constraints] = await Promise.all([
        csketchApi.get(id),
        csketchApi.listConstraints(id),
      ])
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, summary)
        return {
          csketch: {
            ...state.csketch,
            summaries,
            activeId: id,
            activeConstraints: constraints,
            lastReport: null,
            lastDofReport: null,
            // Opening a fresh csketch always lands in read-only /
            // drag-only mode; the user picks a tool from the panel.
            activeTool: null,
          },
        }
      })
    },

    closeCSketch: () => {
      set((state) => ({
        csketch: {
          ...state.csketch,
          activeId: null,
          activeConstraints: [],
          lastReport: null,
          lastDofReport: null,
          activeTool: null,
        },
      }))
    },

    deleteCSketch: async (id) => {
      await csketchApi.delete(id)
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.delete(id)
        const wasActive = state.csketch.activeId === id
        return {
          csketch: {
            ...state.csketch,
            summaries,
            activeId: wasActive ? null : state.csketch.activeId,
            activeConstraints: wasActive ? [] : state.csketch.activeConstraints,
            lastReport: wasActive ? null : state.csketch.lastReport,
            lastDofReport: wasActive ? null : state.csketch.lastDofReport,
            activeTool: wasActive ? null : state.csketch.activeTool,
          },
        }
      })
    },

    refreshCSketches: async () => {
      const ids = await csketchApi.list()
      // Pay one `GET /csketch/{id}` per id so the map stays
      // full-fidelity. The list endpoint itself returns only ids by
      // design — sketches with many entities would otherwise force a
      // heavy payload on every reconnect.
      const summaries = await Promise.all(ids.map((id) => csketchApi.get(id)))
      set((state) => {
        const next = new Map<string, CSketchSummary>()
        for (const s of summaries) next.set(s.id, s)
        return {
          csketch: { ...state.csketch, summaries: next },
        }
      })
    },

    refreshCSketch: async (id) => {
      const summary = await csketchApi.get(id)
      const isActive = get().csketch.activeId === id
      // For the active csketch we additionally pull the constraint
      // list AND the DOF report so any UI bound to either updates in
      // the same tick. Without the DOF refresh, the floating HUD
      // would lag mutations by a full pointer-up cycle. The two
      // GETs are independent → fire them in parallel.
      const [constraints, dofReport] = isActive
        ? await Promise.all([
            csketchApi.listConstraints(id),
            csketchApi.dof(id),
          ])
        : [null, null]
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, summary)
        return {
          csketch: {
            ...state.csketch,
            summaries,
            activeConstraints: constraints ?? state.csketch.activeConstraints,
            lastDofReport: isActive
              ? dofReport
              : state.csketch.lastDofReport,
          },
        }
      })
    },

    addCSketchPoint: async (id, body) => {
      const { id: pid } = await csketchApi.addPoint(id, body)
      await get().refreshCSketch(id)
      return pid
    },

    addCSketchLine: async (id, body) => {
      const { id: lid } = await csketchApi.addLine(id, body)
      await get().refreshCSketch(id)
      return lid
    },

    addCSketchCircle: async (id, body) => {
      const { id: cid } = await csketchApi.addCircle(id, body)
      await get().refreshCSketch(id)
      return cid
    },

    addCSketchConstraint: async (id, constraint) => {
      const { id: cid } = await csketchApi.addConstraint(id, constraint)
      await get().refreshCSketch(id)
      return cid
    },

    deleteCSketchConstraint: async (id, cid) => {
      await csketchApi.deleteConstraint(id, cid)
      await get().refreshCSketch(id)
    },

    updateCSketchConstraintValue: async (id, cid, value) => {
      // The PATCH response carries summary + report + constraint in a
      // single payload, so no follow-up GETs are needed. Conflicts
      // throw `CSketchConstraintConflictError` — propagate untouched.
      const resp = await csketchApi.updateConstraintValue(id, cid, value)
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, resp.summary)
        const isActive = state.csketch.activeId === id
        // Splice the updated constraint into `activeConstraints` if
        // we hold a copy. Use a stable in-place replace so unrelated
        // entries' identity is preserved (helps React memo paths in
        // the constraint list view).
        const activeConstraints = isActive
          ? state.csketch.activeConstraints.map((c) =>
              c.id === resp.constraint.id ? resp.constraint : c,
            )
          : state.csketch.activeConstraints
        return {
          csketch: {
            ...state.csketch,
            summaries,
            activeConstraints,
            lastReport: isActive ? resp.report : state.csketch.lastReport,
          },
        }
      })
      return resp.report
    },

    solveCSketch: async (id, options) => {
      const report = await csketchApi.solve(id, options)
      // The solver may have moved entities; refresh the summary so
      // any downstream renderer sees the new positions.
      const summary = await csketchApi.get(id)
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, summary)
        const isActive = state.csketch.activeId === id
        return {
          csketch: {
            ...state.csketch,
            summaries,
            lastReport: isActive ? report : state.csketch.lastReport,
          },
        }
      })
      return report
    },

    dragCSketch: async (id, body) => {
      const report = await csketchApi.drag(id, body)
      const summary = await csketchApi.get(id)
      set((state) => {
        const summaries = new Map(state.csketch.summaries)
        summaries.set(id, summary)
        const isActive = state.csketch.activeId === id
        return {
          csketch: {
            ...state.csketch,
            summaries,
            lastReport: isActive ? report : state.csketch.lastReport,
          },
        }
      })
      return report
    },

    analyseCSketchDof: async (id) => {
      const report = await csketchApi.dof(id)
      set((state) => {
        const isActive = state.csketch.activeId === id
        return {
          csketch: {
            ...state.csketch,
            lastDofReport: isActive ? report : state.csketch.lastDofReport,
          },
        }
      })
      return report
    },

    setCSketchTool: (tool) => {
      set((state) => ({
        csketch: { ...state.csketch, activeTool: tool },
      }))
    },

    setSceneRef: (scene) => set({ sceneRef: scene }),
    setCameraRef: (camera) => set({ cameraRef: camera }),
    setGlRef: (gl) => set({ glRef: gl }),
  })

export const useSceneStore = create<SceneState>()(
  subscribeWithSelector(sceneCreator),
)
