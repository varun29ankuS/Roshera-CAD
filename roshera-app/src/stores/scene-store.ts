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

  // Edges
  edgeSettings: EdgeSettings

  // Grid
  gridSettings: GridSettings

  // Viewport
  viewportSize: { width: number; height: number }

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

  registerMeshRef: (id: string, mesh: THREE.Mesh) => void
  unregisterMeshRef: (id: string) => void

  setActiveTool: (tool: TransformTool) => void
  setTransformSpace: (space: TransformSpace) => void
  setSnapMode: (mode: SnapMode) => void
  setSnapValue: (value: number) => void

  setCameraPreset: (preset: string) => void
  clearPendingCameraPreset: () => void

  setEdgeSettings: (settings: Partial<EdgeSettings>) => void
  setGridSettings: (settings: Partial<GridSettings>) => void
  setViewportSize: (size: { width: number; height: number }) => void
  setSceneRef: (scene: THREE.Scene | null) => void
  setCameraRef: (camera: THREE.Camera | null) => void
  setGlRef: (gl: THREE.WebGLRenderer | null) => void
}

export const useSceneStore = create<SceneState>()(
  subscribeWithSelector((set, _get) => ({
    objects: new Map(),
    objectOrder: [],
    selectedIds: new Set(),
    hoveredId: null,
    selectionMode: 'object',
    subElementSelections: [],
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
    gridSettings: {
      visible: true,
      cellSize: 1,
      sectionSize: 10,
      fadeDistance: 80,
      infiniteGrid: true,
    },
    viewportSize: { width: 0, height: 0 },
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
      set({ selectionMode: mode, subElementSelections: [] }),

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

    setEdgeSettings: (settings) =>
      set((state) => ({
        edgeSettings: { ...state.edgeSettings, ...settings },
      })),

    setGridSettings: (settings) =>
      set((state) => ({
        gridSettings: { ...state.gridSettings, ...settings },
      })),

    setViewportSize: (size) => set({ viewportSize: size }),
    setSceneRef: (scene) => set({ sceneRef: scene }),
    setCameraRef: (camera) => set({ cameraRef: camera }),
    setGlRef: (gl) => set({ glRef: gl }),
  })),
)
