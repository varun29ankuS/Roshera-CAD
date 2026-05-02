import { create } from 'zustand'
import { subscribeWithSelector } from 'zustand/middleware'
import type * as THREE from 'three'

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
  params: Record<string, number>
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
 * and lifted to 3D when finalised. xy → (u, v, 0); xz → (u, 0, v); yz → (0, u, v).
 * The corresponding extrude direction is the plane's outward normal.
 */
export type SketchPlane = 'xy' | 'xz' | 'yz'

/** Drawing tool inside sketch mode. */
export type SketchTool = 'polyline' | 'rectangle' | 'circle'

export interface SketchState {
  active: boolean
  plane: SketchPlane
  tool: SketchTool
  /** Confirmed sketch points in plane-local (u, v) coordinates. */
  points: Array<[number, number]>
  /**
   * Live cursor position on the sketch plane for the rubber-band preview.
   * `null` when the cursor is off-plane or sketch mode is idle.
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
   * mesh). `null` while the menu is closed.
   */
  contextMenu: { x: number; y: number; objectId: string } | null

  /** Interactive 2D sketch state. See {@link SketchState}. */
  sketch: SketchState

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
  openContextMenu: (menu: { x: number; y: number; objectId: string }) => void
  closeContextMenu: () => void

  enterSketch: (plane?: SketchPlane, tool?: SketchTool) => void
  exitSketch: () => void
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
  setSceneRef: (scene: THREE.Scene | null) => void
  setCameraRef: (camera: THREE.Camera | null) => void
  setGlRef: (gl: THREE.WebGLRenderer | null) => void
}

export const useSceneStore = create<SceneState>()(
  subscribeWithSelector((set) => ({
    objects: new Map(),
    objectOrder: [],
    selectedIds: new Set(),
    hoveredId: null,
    selectionMode: 'object',
    subElementSelections: [],
    hoveredSubElement: null,
    meshRefs: new Map(),
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
      plane: 'xy',
      tool: 'polyline',
      points: [],
      hover: null,
      circleSegments: 64,
      snapStep: 0.5,
      measure: true,
      thickness: 5,
    },
    sceneRef: null,
    cameraRef: null,
    glRef: null,

    addObject: (obj) =>
      set((state) => {
        const objects = new Map(state.objects)
        objects.set(obj.id, obj)
        return { objects, objectOrder: [...state.objectOrder, obj.id] }
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
    toggleCameraProjection: () =>
      set((state) => ({
        cameraProjection:
          state.cameraProjection === 'perspective' ? 'orthographic' : 'perspective',
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
      // Snap the camera flat to the chosen plane so the user sees the
      // sketch face-on. xy → top, xz → front, yz → right.
      // Y-up world: Top view looks down -Y (sees XZ plane); Front
      // view looks down -Z (sees XY plane); Right view looks down -X
      // (sees YZ plane). Match the plane to the view that shows it
      // face-on.
      const presetForPlane: Record<SketchPlane, string> = {
        xy: 'front',
        xz: 'top',
        yz: 'right',
      }
      const presetData = CAMERA_PRESETS[presetForPlane[targetPlane]]
      set((state) => ({
        sketch: {
          ...state.sketch,
          active: true,
          plane: targetPlane,
          tool: tool ?? state.sketch.tool,
          points: [],
          hover: null,
        },
        // Drop any object/sub-element selection so the picker can't fire
        // beneath the sketch overlay.
        selectedIds: new Set(),
        subElementSelections: [],
        hoveredSubElement: null,
        contextMenu: null,
        cameraPreset: presetForPlane[targetPlane],
        pendingCameraPreset: presetData ?? state.pendingCameraPreset,
      }))
    },

    exitSketch: () =>
      set((state) => ({
        sketch: { ...state.sketch, active: false, points: [], hover: null },
      })),

    setSketchTool: (tool) =>
      set((state) => ({
        // Switching tools mid-sketch wipes the in-progress points; the
        // primitives don't share semantics (polyline = N points, rectangle
        // = 2 corners, circle = center+radius), so reusing prior clicks
        // would always be wrong.
        sketch: { ...state.sketch, tool, points: [], hover: state.sketch.hover },
      })),

    setSketchPlane: (plane) => {
      // Mirror enterSketch: re-orient the camera to face the new plane
      // so the sketcher always sees the surface flat-on.
      // Y-up world: Top view looks down -Y (sees XZ plane); Front
      // view looks down -Z (sees XY plane); Right view looks down -X
      // (sees YZ plane). Match the plane to the view that shows it
      // face-on.
      const presetForPlane: Record<SketchPlane, string> = {
        xy: 'front',
        xz: 'top',
        yz: 'right',
      }
      const presetData = CAMERA_PRESETS[presetForPlane[plane]]
      set((state) => ({
        sketch: { ...state.sketch, plane, points: [], hover: null },
        cameraPreset: presetForPlane[plane],
        pendingCameraPreset: presetData ?? state.pendingCameraPreset,
      }))
    },

    addSketchPoint: (point) =>
      set((state) => ({
        sketch: { ...state.sketch, points: [...state.sketch.points, point] },
      })),

    popSketchPoint: () =>
      set((state) => ({
        sketch: { ...state.sketch, points: state.sketch.points.slice(0, -1) },
      })),

    clearSketchPoints: () =>
      set((state) => ({ sketch: { ...state.sketch, points: [] } })),

    setSketchHover: (point) =>
      set((state) => ({ sketch: { ...state.sketch, hover: point } })),

    setSketchPoint: (index, point) =>
      set((state) => {
        if (index < 0 || index >= state.sketch.points.length) return state
        const points = state.sketch.points.slice()
        points[index] = point
        return { sketch: { ...state.sketch, points } }
      }),

    setSketchView: (patch) =>
      set((state) => ({ sketch: { ...state.sketch, ...patch } })),

    setSceneRef: (scene) => set({ sceneRef: scene }),
    setCameraRef: (camera) => set({ cameraRef: camera }),
    setGlRef: (gl) => set({ glRef: gl }),
  })),
)
