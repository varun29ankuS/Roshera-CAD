/**
 * REST client for the kernel assembly surface (`/api/assemblies/...`).
 *
 * The wire shape mirrors `api-server/src/assembly_mgr.rs` exactly:
 * `AssemblySummary` carries components + mates + exploded-view config,
 * and is built by the manager under a read lock. Components and mates
 * never leak the kernel `Arc<BRepModel>` payloads — every response is a
 * snapshot.
 *
 * # Tagged unions
 *
 * `MateType` and `MateReference` are Rust enums with default serde
 * ("externally tagged"):
 *
 *   - Unit variant   → `"Coincident"` (just the string)
 *   - Tuple variant  → `{ "Distance": 42.0 }`
 *   - Struct variant → `{ "Gear": { "ratio": 1.5 } }`
 *
 * The TypeScript unions below match that exactly.
 */

const API_HOST = import.meta.env.VITE_API_URL || ''

// ── Tagged unions ────────────────────────────────────────────────────

/**
 * Mate constraint type. Unit variants serialise as the bare string
 * name; parameterised variants serialise as `{ <Name>: payload }`.
 * Mirror of `geometry_engine::assembly::MateType`.
 */
export type MateType =
  | 'Coincident'
  | 'Concentric'
  | 'Parallel'
  | 'Perpendicular'
  | 'Tangent'
  | 'Symmetric'
  | 'Cam'
  | 'Path'
  | 'Lock'
  | { Distance: number }
  | { Angle: number }
  | { Gear: { ratio: number } }

/** Discriminant tag (the variant name) for a `MateType` value. */
export type MateTypeTag =
  | 'Coincident'
  | 'Concentric'
  | 'Parallel'
  | 'Perpendicular'
  | 'Tangent'
  | 'Symmetric'
  | 'Cam'
  | 'Path'
  | 'Lock'
  | 'Distance'
  | 'Angle'
  | 'Gear'

/** All twelve mate-type tags in display order. */
export const MATE_TYPE_TAGS: readonly MateTypeTag[] = [
  'Coincident',
  'Concentric',
  'Parallel',
  'Perpendicular',
  'Tangent',
  'Symmetric',
  'Distance',
  'Angle',
  'Lock',
  'Gear',
  'Cam',
  'Path',
] as const

/** Inverse of {@link MATE_TYPE_TAGS}: get the tag from a runtime value. */
export function mateTypeTag(t: MateType): MateTypeTag {
  return typeof t === 'string' ? t : (Object.keys(t)[0] as MateTypeTag)
}

/** Convenience: human-readable label for a `MateType` tag. */
export function mateTypeLabel(tag: MateTypeTag): string {
  switch (tag) {
    case 'Coincident':
      return 'Coincident'
    case 'Concentric':
      return 'Concentric'
    case 'Parallel':
      return 'Parallel'
    case 'Perpendicular':
      return 'Perpendicular'
    case 'Tangent':
      return 'Tangent'
    case 'Symmetric':
      return 'Symmetric'
    case 'Cam':
      return 'Cam'
    case 'Path':
      return 'Path'
    case 'Lock':
      return 'Lock'
    case 'Distance':
      return 'Distance (mm)'
    case 'Angle':
      return 'Angle (rad)'
    case 'Gear':
      return 'Gear (ratio)'
  }
}

/** Vector / Point payload: kernel uses `{x, y, z}` field structs. */
export interface Vec3 {
  x: number
  y: number
  z: number
}

/**
 * Geometric reference attached to a component under a slot name.
 * Mirror of `geometry_engine::assembly::MateReference`. References
 * are registered via `POST /api/assemblies/{id}/references`; the
 * mate itself only carries the slot name.
 */
export type MateReference =
  | { Face: { face_id: string; normal: Vec3 } }
  | { Edge: { edge_id: string; direction: Vec3 } }
  | { Point: { position: Vec3 } }
  | { Axis: { origin: Vec3; direction: Vec3 } }
  | { Plane: { origin: Vec3; normal: Vec3 } }

// ── Wire DTOs ────────────────────────────────────────────────────────

export interface MateReferenceSummary {
  name: string
  // The kernel uses `#[serde(flatten)]` here, so the wire shape is
  // `{ name, <VariantTag>: payload }`. We expose it as the same union
  // members merged with `name` — consumers can switch on the present
  // key (Face/Edge/Point/Axis/Plane).
  Face?: { face_id: string; normal: Vec3 }
  Edge?: { edge_id: string; direction: Vec3 }
  Point?: { position: Vec3 }
  Axis?: { origin: Vec3; direction: Vec3 }
  Plane?: { origin: Vec3; normal: Vec3 }
}

export interface ComponentSummary {
  id: string
  name: string
  /** Row-major 4×4 transform. */
  transform: [
    [number, number, number, number],
    [number, number, number, number],
    [number, number, number, number],
    [number, number, number, number],
  ]
  is_fixed: boolean
  parent: string | null
  degrees_of_freedom: number
  mate_references: MateReferenceSummary[]
}

export interface MateSummary {
  id: string
  name: string
  mate_type: MateType
  component1: string
  reference1: string
  component2: string
  reference2: string
  suppressed: boolean
  flip: boolean
  solved: boolean
  error: string | null
}

export interface ExplosionStep {
  component: string
  translation: Vec3
  rotation: { x: number; y: number; z: number; w: number } | null
  duration: number
}

export interface ExplodedViewConfig {
  steps: ExplosionStep[]
  current_step: number
  auto_explode: boolean
  scale: number
}

export interface AssemblySummary {
  id: string
  name: string
  root_component: string | null
  components: ComponentSummary[]
  mates: MateSummary[]
  exploded: ExplodedViewConfig | null
}

// ── REST helpers ─────────────────────────────────────────────────────

async function jsonOrThrow<T>(resp: Response, context: string): Promise<T> {
  if (!resp.ok) {
    let detail = ''
    try {
      detail = await resp.text()
    } catch {
      /* swallow — status code is the primary signal */
    }
    throw new Error(
      `${context}: ${resp.status} ${resp.statusText}${detail ? ` — ${detail}` : ''}`,
    )
  }
  return resp.json() as Promise<T>
}

export async function listAssemblies(): Promise<string[]> {
  const r = await fetch(`${API_HOST}/api/assemblies`)
  return jsonOrThrow<string[]>(r, 'listAssemblies')
}

export async function getAssembly(id: string): Promise<AssemblySummary> {
  const r = await fetch(`${API_HOST}/api/assemblies/${id}`)
  return jsonOrThrow<AssemblySummary>(r, 'getAssembly')
}

export async function createAssembly(name: string): Promise<string> {
  const r = await fetch(`${API_HOST}/api/assemblies`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name }),
  })
  const { id } = await jsonOrThrow<{ id: string }>(r, 'createAssembly')
  return id
}

export async function deleteAssembly(id: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/assemblies/${id}`, { method: 'DELETE' })
  await jsonOrThrow<unknown>(r, 'deleteAssembly')
}

/**
 * Primitive geometry kinds accepted by `addComponent`. Externally
 * tagged on the wire to match the kernel's `ComponentPrimitive` enum
 * (`{ "type": "Box", "dx": 10, "dy": 10, "dz": 10 }`).
 */
export type ComponentPrimitive =
  | { type: 'Box'; dx: number; dy: number; dz: number }
  | { type: 'Cylinder'; radius: number; height: number }
  | { type: 'Sphere'; radius: number }

/** Tag-only union; useful for UI pickers. */
export type ComponentPrimitiveTag = ComponentPrimitive['type']

export const COMPONENT_PRIMITIVE_TAGS: readonly ComponentPrimitiveTag[] = [
  'Box',
  'Cylinder',
  'Sphere',
] as const

/**
 * Wire form of a tessellated component mesh — Three.js-shaped.
 * `vertices`/`normals` are flat (3 floats per vertex); `indices` is
 * a triangle list (3 indices per triangle). All in the component's
 * *local* frame — apply the component's transform on the rendered
 * Object3D.
 */
export interface ComponentMesh {
  component_id: string
  vertices: number[]
  normals: number[]
  indices: number[]
  triangle_count: number
}

export async function addComponent(
  assemblyId: string,
  body: { name: string; transform?: number[][]; primitive?: ComponentPrimitive },
): Promise<string> {
  const r = await fetch(`${API_HOST}/api/assemblies/${assemblyId}/components`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const { component_id } = await jsonOrThrow<{ component_id: string }>(r, 'addComponent')
  return component_id
}

export async function removeComponent(assemblyId: string, componentId: string): Promise<void> {
  const r = await fetch(
    `${API_HOST}/api/assemblies/${assemblyId}/components/${componentId}`,
    { method: 'DELETE' },
  )
  await jsonOrThrow<unknown>(r, 'removeComponent')
}

export async function setComponentTransform(
  assemblyId: string,
  componentId: string,
  transform: number[][],
): Promise<AssemblySummary> {
  const r = await fetch(
    `${API_HOST}/api/assemblies/${assemblyId}/components/${componentId}/transform`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ transform }),
    },
  )
  return jsonOrThrow<AssemblySummary>(r, 'setComponentTransform')
}

/**
 * Build a row-major translation-only 4×4 matrix. Useful for the
 * "drop component at this position" path before the user dials in
 * a real orientation.
 */
export function translationMatrix(x: number, y: number, z: number): number[][] {
  return [
    [1, 0, 0, x],
    [0, 1, 0, y],
    [0, 0, 1, z],
    [0, 0, 0, 1],
  ]
}

/**
 * Read the translation column (rightmost) from a row-major 4×4. Used
 * by the inline component-move editor to seed its x/y/z inputs from
 * the current snapshot.
 */
export function translationOf(m: number[][]): [number, number, number] {
  return [m[0]?.[3] ?? 0, m[1]?.[3] ?? 0, m[2]?.[3] ?? 0]
}

/**
 * Compose a row-major 4×4 wire matrix from a Three.js
 * position / quaternion (xyzw) / scale triple. The transform gizmo
 * reads these straight off the dragged Object3D after the user
 * releases the mouse and we commit via `setComponentTransform`.
 *
 * Three.js `Matrix4` stores elements in column-major order; we
 * transpose at the boundary so the kernel (row-major) sees what it
 * expects.
 */
export function composeRowMajor(
  position: [number, number, number],
  quaternion: [number, number, number, number],
  scale: [number, number, number],
): number[][] {
  const [px, py, pz] = position
  const [qx, qy, qz, qw] = quaternion
  const [sx, sy, sz] = scale

  // Rotation matrix from a unit quaternion. Standard derivation; equal
  // to what THREE.Matrix4.makeRotationFromQuaternion would give us, but
  // computed inline to avoid taking a dep on three for a 4×4 build.
  const xx = qx * qx, xy = qx * qy, xz = qx * qz, xw = qx * qw
  const yy = qy * qy, yz = qy * qz, yw = qy * qw
  const zz = qz * qz, zw = qz * qw
  const r00 = 1 - 2 * (yy + zz)
  const r01 = 2 * (xy - zw)
  const r02 = 2 * (xz + yw)
  const r10 = 2 * (xy + zw)
  const r11 = 1 - 2 * (xx + zz)
  const r12 = 2 * (yz - xw)
  const r20 = 2 * (xz - yw)
  const r21 = 2 * (yz + xw)
  const r22 = 1 - 2 * (xx + yy)

  // Multiply rotation by diagonal scale (right side) and put translation
  // in the rightmost column. Bottom row is the affine `[0 0 0 1]`.
  return [
    [r00 * sx, r01 * sy, r02 * sz, px],
    [r10 * sx, r11 * sy, r12 * sz, py],
    [r20 * sx, r21 * sy, r22 * sz, pz],
    [0,        0,        0,        1 ],
  ]
}

/**
 * Tessellate a component and return its mesh in the component's
 * local frame. The caller is responsible for applying the component
 * transform (see {@link translationOf} / `setComponentTransform`).
 *
 * Returns an empty-buffer payload (triangle_count = 0) when the
 * component carries no solids — this is normal for a fresh
 * `addComponent` call with no `primitive`.
 */
export async function getComponentMesh(
  assemblyId: string,
  componentId: string,
): Promise<ComponentMesh> {
  const r = await fetch(
    `${API_HOST}/api/assemblies/${assemblyId}/components/${componentId}/mesh`,
  )
  return jsonOrThrow<ComponentMesh>(r, 'getComponentMesh')
}

export async function registerMateReference(
  assemblyId: string,
  body: { component: string; name: string; reference: MateReference },
): Promise<void> {
  const r = await fetch(`${API_HOST}/api/assemblies/${assemblyId}/references`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  await jsonOrThrow<unknown>(r, 'registerMateReference')
}

export interface AddMateBody {
  mate_type: MateType
  component1: string
  reference1: string
  component2: string
  reference2: string
}

export async function addMate(assemblyId: string, body: AddMateBody): Promise<string> {
  const r = await fetch(`${API_HOST}/api/assemblies/${assemblyId}/mates`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const { mate_id } = await jsonOrThrow<{ mate_id: string }>(r, 'addMate')
  return mate_id
}

export async function removeMate(assemblyId: string, mateId: string): Promise<void> {
  const r = await fetch(`${API_HOST}/api/assemblies/${assemblyId}/mates/${mateId}`, {
    method: 'DELETE',
  })
  await jsonOrThrow<unknown>(r, 'removeMate')
}

export interface PatchMateBody {
  suppressed?: boolean
  flip?: boolean
}

export async function patchMate(
  assemblyId: string,
  mateId: string,
  patch: PatchMateBody,
): Promise<AssemblySummary> {
  const r = await fetch(`${API_HOST}/api/assemblies/${assemblyId}/mates/${mateId}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(patch),
  })
  return jsonOrThrow<AssemblySummary>(r, 'patchMate')
}

export async function solveAssembly(id: string): Promise<AssemblySummary> {
  const r = await fetch(`${API_HOST}/api/assemblies/${id}/solve`, { method: 'POST' })
  return jsonOrThrow<AssemblySummary>(r, 'solveAssembly')
}

/**
 * One leg of the pick-driven mate flow. Carries everything the kernel
 * needs to construct a `MateReference::Plane` in the component's
 * local frame.
 */
export interface PickRef {
  componentId: string
  origin: [number, number, number]
  normal: [number, number, number]
}

/**
 * Auto-derive a short, deterministic reference name from a
 * local-frame origin. Two picks at the same coordinate produce the
 * same name so re-clicking the same face deduplicates against the
 * component's existing references and avoids a wire-shape rejection.
 *
 * The name is purely UI-side bookkeeping — the kernel cares only that
 * it's unique among the component's references at registration time.
 */
function autoRefName(prefix: 'auto_face' | 'auto_edge', origin: [number, number, number]): string {
  const fmt = (n: number) => n.toFixed(3).replace('-', 'n')
  return `${prefix}_${fmt(origin[0])}_${fmt(origin[1])}_${fmt(origin[2])}`
}

/**
 * Decide whether a component already carries a reference with the
 * same auto-generated slot name. The wire-shape `MateReferenceSummary`
 * uses `#[serde(flatten)]` so the variant key is alongside `name`.
 */
function componentHasReferenceNamed(
  components: ComponentSummary[],
  componentId: string,
  name: string,
): boolean {
  const c = components.find((x) => x.id === componentId)
  if (!c) return false
  return c.mate_references.some((r) => r.name === name)
}

/**
 * Pick-driven mate creation. Registers each pick as a `Plane` mate
 * reference (idempotent against the per-component slot name), creates
 * the mate, and runs the solver. Returns the post-solve snapshot.
 *
 * Why `Plane` instead of `Face`: the per-component mesh handler ships
 * positions + normals + indices but no per-triangle FaceId map, so
 * we cannot resolve a clicked triangle to a kernel `face_id` without
 * a backend change. A `Plane { origin, normal }` reference carries
 * the same geometric information for Coincident / Distance /
 * Parallel mates without that dependency.
 */
export async function addMateFromPicks(
  assemblyId: string,
  current: AssemblySummary,
  pick1: PickRef,
  pick2: PickRef,
  mateType: MateType,
): Promise<AssemblySummary> {
  const name1 = autoRefName('auto_face', pick1.origin)
  const name2 = autoRefName('auto_face', pick2.origin)

  const toVec3 = (a: [number, number, number]): Vec3 => ({ x: a[0], y: a[1], z: a[2] })

  if (!componentHasReferenceNamed(current.components, pick1.componentId, name1)) {
    await registerMateReference(assemblyId, {
      component: pick1.componentId,
      name: name1,
      reference: { Plane: { origin: toVec3(pick1.origin), normal: toVec3(pick1.normal) } },
    })
  }
  if (!componentHasReferenceNamed(current.components, pick2.componentId, name2)) {
    await registerMateReference(assemblyId, {
      component: pick2.componentId,
      name: name2,
      reference: { Plane: { origin: toVec3(pick2.origin), normal: toVec3(pick2.normal) } },
    })
  }

  await addMate(assemblyId, {
    mate_type: mateType,
    component1: pick1.componentId,
    reference1: name1,
    component2: pick2.componentId,
    reference2: name2,
  })

  return solveAssembly(assemblyId)
}

// ── MateType constructors ────────────────────────────────────────────

/**
 * Build a wire-shape `MateType` value from a tag + numeric parameter
 * (used by parameterised variants Distance/Angle/Gear).
 */
export function makeMateType(tag: MateTypeTag, parameter: number): MateType {
  switch (tag) {
    case 'Distance':
      return { Distance: parameter }
    case 'Angle':
      return { Angle: parameter }
    case 'Gear':
      return { Gear: { ratio: parameter } }
    default:
      return tag
  }
}

/** True if the given tag requires a numeric parameter. */
export function mateTypeNeedsParameter(tag: MateTypeTag): boolean {
  return tag === 'Distance' || tag === 'Angle' || tag === 'Gear'
}
