// ─── Types matching backend shared-types ─────────────────────────────

export type ObjectId = string // UUID

export interface Position3D {
  x: number
  y: number
  z: number
}

export interface MeshData {
  vertices: number[]
  indices: number[]
  normals: number[]
  uvs?: number[]
  colors?: number[]
  /**
   * Per-triangle B-Rep `FaceId` array. Length = `indices.length / 3`.
   * Drives face picking on the frontend.
   */
  face_ids?: number[]
}

export interface AnalyticalGeometry {
  solid_id: number
  primitive_type: string
  parameters: Record<string, number>
  properties: {
    volume: number
    surface_area: number
    bounding_box: { min: [number, number, number]; max: [number, number, number] }
    center_of_mass: [number, number, number]
  }
}

export interface MaterialProperties {
  diffuse_color: [number, number, number, number]
  metallic: number
  roughness: number
  emission: [number, number, number]
  name: string
}

export interface Transform3D {
  translation: [number, number, number]
  rotation: [number, number, number, number] // quaternion xyzw
  scale: [number, number, number]
}

export interface CADObject {
  id: ObjectId
  name: string
  mesh: MeshData
  analytical_geometry?: AnalyticalGeometry
  transform: Transform3D
  material: MaterialProperties
  visible: boolean
  locked: boolean
  parent?: ObjectId
  children: ObjectId[]
  metadata: Record<string, unknown>
  created_at: number
  modified_at: number
}

// ─── API request/response types ──────────────────────────────────────

export interface GeometryCreateRequest {
  shape_type: string
  parameters: Record<string, number>
  position: [number, number, number]
  material?: string
}

export interface GeometryResponse {
  object: CADObject
  success: boolean
  execution_time_ms: number
  message: string
}

export interface NaturalLanguageRequest {
  command: string
  session_id: string
  context?: Record<string, unknown>
}

export interface NaturalLanguageResponse {
  results: CommandResult[]
  success: boolean
  processing_time_ms: number
  parsed_commands?: string[]
}

export interface CommandResult {
  success: boolean
  execution_time_ms: number
  objects_affected: ObjectId[]
  message: string
  data?: unknown
  object_id?: string
  error?: string
}

// ─── WebSocket message types ─────────────────────────────────────────

export interface ServerMessage {
  type: string
  payload: unknown
}

export interface GeometryUpdate {
  type: 'Tessellated'
  object: CADObject
}

export interface SessionUpdate {
  type: string
  session_id: string
}

export interface CollaboratorInfo {
  id: string
  name: string
  color: [number, number, number, number]
  cursor_position?: [number, number, number]
  selected_objects: string[]
}

// ─── AI Command types ────────────────────────────────────────────────

export interface AICommandRequest {
  type: 'AICommand'
  command: {
    cmd: string
    text?: string
    object_id?: string
    transform_type?: unknown
  }
}

export interface ExportRequest {
  format: 'STL' | 'OBJ' | 'STEP' | 'IGES' | 'glTF' | 'ROS' | 'FBX'
  objects: ObjectId[]
  options: {
    binary: boolean
    include_materials: boolean
    merge_objects: boolean
  }
}
