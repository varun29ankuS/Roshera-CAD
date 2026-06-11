/**
 * Typed REST client for the backend `/api/csketch/*` surface.
 *
 * The constrained sketch (`csketch`) is a parametric Newton-Raphson
 * sketcher — points / lines / circles plus a constraint store and a
 * solver — distinct from the click-to-place `sketch` system this
 * directory's `sketch-api.ts` wraps. The two coexist by design: the
 * click-to-place path is a fast "outline-and-extrude" workflow,
 * while csketch is the industry-standard
 * fully-constrained sketcher. See `roshera-backend/api-server/src/
 * csketch.rs` for the canonical wire-shape definitions.
 *
 * Concurrency / state ownership: each csketch is a server-side
 * `Arc<Sketch>` keyed by `SketchId`. Mutations take `&self` on the
 * kernel handle, so the wire surface is plain REST — there is no
 * per-sketch lock the client must coordinate. The frontend never
 * mutates the sketch optimistically; every editor action waits for
 * the server response before reflecting the change in the store.
 *
 * Wire-shape conventions:
 *
 *  • Most enum types use serde's default (externally-tagged) format:
 *      EntityRef          → { "Point": "<uuid>" }
 *      ConstraintType     → { "Geometric": "Coincident" }
 *                         or { "Dimensional": { "Distance": 10 } }
 *      ConstraintStatus   → "Satisfied" | "Disabled" | "Conflicting"
 *                         or { "Violated": { "error": …, "suggestion": … } }
 *      ConstraintPriority → "Required" | "High" | "Medium" | "Low"
 *      LineGeometry       → { "Infinite": Line2d } | { "Ray": Ray2d }
 *                         or { "Segment": LineSegment2d }
 *
 *  • A few enums are explicitly tagged via `#[serde(tag = "kind", …)]`
 *    with snake_case variant names:
 *      SolverStatus       → { kind: "converged", iterations, final_error }
 *      DofStatus          → { kind: "under_constrained", dofs }
 *      DragTarget         → { kind: "point", params: { x, y } }
 *      SketchSolveError   → { kind: "invalid_damping", value }
 *
 * Mirroring these as discriminated unions in TypeScript lets call
 * sites pattern-match without parsing prose.
 */

const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

// ── Primitive geometry wire shapes ───────────────────────────────────

/** 2D point used inside `LineGeometry`, `DragTarget::Point`, etc. */
export interface Point2d {
  x: number
  y: number
}

/** 2D unit-length direction vector (server normalises on construction). */
export interface Vector2d {
  x: number
  y: number
}

/** Infinite 2D line: a point on the line plus a unit direction. */
export interface Line2d {
  point: Point2d
  direction: Vector2d
}

/** Ray: a 2D origin plus a unit direction. */
export interface Ray2d {
  origin: Point2d
  direction: Vector2d
}

/** Bounded line segment between two endpoints. */
export interface LineSegment2d {
  start: Point2d
  end: Point2d
}

/**
 * Externally-tagged union over the three line-kind variants the
 * kernel supports. Each `LineSummary` carries one of these.
 */
export type LineGeometry =
  | { Infinite: Line2d }
  | { Ray: Ray2d }
  | { Segment: LineSegment2d }

// ── Entity summaries ────────────────────────────────────────────────

/**
 * Wire form of a kernel `ParametricPoint2d`. `is_fixed` flips when
 * the user pins both DOFs at once (e.g. `add_point` with
 * `fixed: true`); `is_construction` is reserved for future
 * construction-geometry UX.
 */
export interface CSketchPointSummary {
  id: string
  x: number
  y: number
  is_fixed: boolean
  is_construction: boolean
}

/** Wire form of a kernel `ParametricLine2d`. */
export interface CSketchLineSummary {
  id: string
  geometry: LineGeometry
  is_construction: boolean
}

/** Wire form of a kernel `ParametricCircle2d` (centre + radius). */
export interface CSketchCircleSummary {
  id: string
  cx: number
  cy: number
  radius: number
  is_construction: boolean
}

/**
 * Snapshot of an entire csketch — the shape returned by `GET
 * /api/csketch/{id}` and embedded in `ConstraintUpdateResponse`.
 *
 * Constraints are intentionally not embedded here: the
 * `/constraints` endpoint serves them separately so that sketches
 * with many constraints don't pay the round-trip cost on every
 * entity-only refresh.
 */
export interface CSketchSummary {
  id: string
  points: CSketchPointSummary[]
  lines: CSketchLineSummary[]
  circles: CSketchCircleSummary[]
  constraint_count: number
}

// ── Constraint wire types ───────────────────────────────────────────

/**
 * Reference to a sketch entity by kind + id. The kind tag is the
 * outer object key; the value is the entity's UUID string.
 */
export type EntityRef =
  | { Point: string }
  | { Line: string }
  | { Arc: string }
  | { Circle: string }
  | { Rectangle: string }
  | { Ellipse: string }
  | { Spline: string }
  | { Polyline: string }

/**
 * Geometric (non-dimensional) constraint kind. The kernel's enum is
 * mostly unit-variant; `IntersectionAngle(f64)` is the only variant
 * carrying a payload, so it serialises as
 * `{ "IntersectionAngle": <radians> }`.
 */
export type GeometricConstraint =
  | 'Coincident'
  | 'Parallel'
  | 'Perpendicular'
  | 'Tangent'
  | 'Concentric'
  | 'Equal'
  | 'Horizontal'
  | 'Vertical'
  | 'Symmetric'
  | 'PointOnCurve'
  | 'Midpoint'
  | 'Collinear'
  | 'SmoothTangent'
  | 'CurvatureContinuity'
  | 'Offset'
  | 'MultiTangent'
  | 'EqualArea'
  | 'EqualPerimeter'
  | 'Centroid'
  | 'CurvatureExtremum'
  | { IntersectionAngle: number }
  | 'ContactConstraint'

/**
 * Dimensional (scalar-valued) constraint kind. All single-value
 * variants serialise as `{ "<Variant>": <scalar> }`; `CenterOfMass`
 * is the one two-value variant and serialises as
 * `{ "CenterOfMass": { x, y } }`.
 *
 * The PATCH `/constraint/{cid}/value` endpoint can edit any
 * single-scalar variant; `CenterOfMass` is rejected by the kernel
 * (`DimensionalUpdateError::UnsupportedVariant`).
 */
export type DimensionalConstraint =
  | { Distance: number }
  | { Angle: number }
  | { Radius: number }
  | { Diameter: number }
  | { Length: number }
  | { XCoordinate: number }
  | { YCoordinate: number }
  | { Area: number }
  | { Perimeter: number }
  | { ArcLength: number }
  | { Curvature: number }
  | { Slope: number }
  | { OffsetDistance: number }
  | { AspectRatio: number }
  | { MinDistance: number }
  | { MaxDistance: number }
  | { MomentOfInertia: number }
  | { CenterOfMass: { x: number; y: number } }

/** Composite constraint kind: either geometric or dimensional. */
export type ConstraintType =
  | { Geometric: GeometricConstraint }
  | { Dimensional: DimensionalConstraint }

/**
 * Live solver verdict on a single constraint. `Satisfied` is the
 * default after every successful re-solve; `Violated` carries the
 * residual magnitude (and an optional suggested correction the
 * kernel may emit). `Disabled` is reserved for future
 * `disable_constraint` API; `Conflicting` is set when the
 * over-constrained detector singles out the constraint.
 */
export type ConstraintStatus =
  | 'Satisfied'
  | { Violated: { error: number; suggestion: number | null } }
  | 'Disabled'
  | 'Conflicting'

/**
 * Solver priority. `Required` constraints cannot be relaxed (used
 * for fixed-point pins); `Low` is reserved for soft drag pulls.
 */
export type ConstraintPriority = 'Required' | 'High' | 'Medium' | 'Low'

/**
 * Full kernel `Constraint` record. POST-able as the body of
 * `/api/csketch/{id}/constraint`; returned as part of
 * `ConstraintUpdateResponse`.
 *
 * `id` may be supplied by the caller (the kernel honours the
 * provided UUID) — usually pass a fresh `crypto.randomUUID()`.
 */
export interface Constraint {
  id: string
  constraint_type: ConstraintType
  entities: EntityRef[]
  priority: ConstraintPriority
  status: ConstraintStatus
  name: string | null
}

// ── Solver wire types ───────────────────────────────────────────────

/**
 * Newton-Raphson outcome. Tagged via `#[serde(tag = "kind",
 * rename_all = "snake_case")]`. `iterations` and `final_error`
 * accompany `converged` / `not_converged`; the structural-DOF
 * verdicts carry their own counters.
 */
export type SolverStatus =
  | { kind: 'converged'; iterations: number; final_error: number }
  | { kind: 'not_converged'; iterations: number; final_error: number }
  | { kind: 'over_constrained'; conflicting_constraints: number }
  | { kind: 'under_constrained'; degrees_of_freedom: number }
  | { kind: 'unstable' }

/**
 * Pure structural-DOF classification, independent of any
 * Newton-Raphson iteration. `under_constrained` carries the count
 * of remaining free DOFs; `over_constrained` carries the count of
 * excess constraints.
 */
export type DofStatus =
  | { kind: 'fully_constrained' }
  | { kind: 'under_constrained'; dofs: number }
  | { kind: 'over_constrained'; conflicting_constraints: number }

/** Output of `GET /api/csketch/{id}/dof`. */
export interface DofReport {
  total_free_dofs: number
  constraint_dofs_removed: number
  status: DofStatus
  entities_analysed: number
  constraints_analysed: number
  constraints_skipped: number
  entities_skipped: EntityRef[]
  /**
   * Constraint ids the diagnose pass classified as linearly
   * dependent on an earlier row AND whose post-solve residual is
   * within tolerance — i.e. duplicates of an already-satisfied
   * constraint. Removing them does not change the solution. Empty
   * when the structural verdict is `fully_constrained` (the rank
   * pass is O(rows²·cols) and is skipped on the fast path).
   *
   * Slice H, added 2026-05-12.
   */
  redundant: string[]
  /**
   * Constraint ids the diagnose pass classified as linearly
   * dependent AND whose post-solve residual exceeds tolerance —
   * i.e. constraints that are inconsistent with the ones already
   * accepted (e.g. two `X = 3` and `X = 7` constraints on the
   * same point). Removing them removes the inconsistency.
   */
  conflicts: string[]
}

/** Output of `POST /api/csketch/{id}/solve` and `/drag`. */
export interface SketchSolveReport {
  status: SolverStatus
  /** Pairs of `(constraint_id, residual_magnitude)`. */
  violations: Array<[string, number]>
  solve_time_ms: number
  entities_solved: number
  constraints_solved: number
  entities_skipped: EntityRef[]
}

/**
 * Newton-Raphson tunables. Pass `undefined` (or omit the field) to
 * use server-side defaults: `max_iterations = 100`, `tolerance =
 * 1e-10`, `damping_factor = 0.5`.
 */
export interface SolveOptions {
  max_iterations: number
  tolerance: number
  damping_factor: number
}

/**
 * Drag target. Tagged via `#[serde(tag = "kind", content =
 * "params", rename_all = "snake_case")]`. Slice B-2 supports
 * dragging points only; lines / circles will gain drag targets as
 * the frontend grows handles for them.
 */
export type DragTarget = { kind: 'point'; params: Point2d }

// ── Conflict (409) payload ──────────────────────────────────────────

/**
 * Detail rows on a `SketchConstraintConflict` (HTTP 409) returned
 * by `PATCH /constraint/{cid}/value`. Only residuals above the
 * server's conflict threshold (1e-3) survive.
 */
export interface ConstraintResidual {
  id: string
  residual: number
}

/**
 * Body of the `details` field on a 409 from PATCH constraint value.
 * The server has already reverted both the constraint value and
 * every entity geometry by the time this payload returns, so a
 * follow-up `GET /api/csketch/{id}` will reflect the pre-PATCH
 * state byte-for-byte.
 */
export interface ConstraintUpdateConflict {
  reason: 'over_constrained' | 'not_converged' | 'unstable'
  status: SolverStatus
  violations: ConstraintResidual[]
  previous_value: number
  attempted_value: number
}

/** 200 response from a successful PATCH constraint value. */
export interface ConstraintUpdateResponse {
  constraint: Constraint
  report: SketchSolveReport
  summary: CSketchSummary
}

// ── Request bodies ──────────────────────────────────────────────────

/** Body of `POST /point`. `fixed` defaults to false on the server. */
export interface AddPointRequest {
  x: number
  y: number
  fixed?: boolean
}

/** Body of `POST /line`. Both endpoints must be pre-existing points. */
export interface AddLineRequest {
  start: string
  end: string
}

/** Body of `POST /circle`. `radius` must be strictly positive. */
export interface AddCircleRequest {
  cx: number
  cy: number
  radius: number
}

/** Body of `POST /solve`. Omit `options` to use server defaults. */
export interface SolveRequest {
  options?: SolveOptions
}

/** Body of `POST /drag`. */
export interface DragRequest {
  entity: EntityRef
  target: DragTarget
  options?: SolveOptions
}

/** Body of `PATCH /constraint/{cid}/value`. */
export interface UpdateConstraintValueRequest {
  value: number
}

// ── Snap + inference wire types (D-1) ───────────────────────────────

/**
 * Kind tag on a `SnapCandidate`. Mirrors the kernel enum (every
 * variant is unit, `#[serde(rename_all = "snake_case")]`), so each
 * value travels on the wire as a plain string — no payload, no
 * outer key.
 *
 * Two-level ranking inside the kernel: `priority()` returns 0 for
 * the discrete vertex-like kinds (`point`, `line_endpoint`,
 * `arc_endpoint`, `rectangle_corner`), 1 for the discrete
 * mid/centre/quadrant kinds (`line_midpoint`, `circle_center`,
 * `circle_quadrant`, `arc_center`, `arc_midpoint`,
 * `rectangle_edge_midpoint`, `rectangle_center`, `ellipse_center`,
 * `ellipse_quadrant`), and 2 for the continuous on-curve kinds
 * (`on_line`, `on_circle`, `on_arc`, `on_ellipse`). Lower priority
 * wins ties; within a tier the smaller Euclidean distance wins.
 */
export type SnapKind =
  | 'point'
  | 'line_endpoint'
  | 'line_midpoint'
  | 'on_line'
  | 'circle_center'
  | 'circle_quadrant'
  | 'on_circle'
  | 'arc_center'
  | 'arc_endpoint'
  | 'arc_midpoint'
  | 'on_arc'
  | 'rectangle_corner'
  | 'rectangle_edge_midpoint'
  | 'rectangle_center'
  | 'ellipse_center'
  | 'ellipse_quadrant'
  | 'on_ellipse'

/**
 * One snap result from `POST /api/csketch/{id}/snap`. The full
 * response is `SnapCandidate[]` sorted by (priority, distance), so
 * `[0]` is the best snap.
 */
export interface SnapCandidate {
  entity: EntityRef
  point: Point2d
  distance: number
  kind: SnapKind
}

/** Body of `POST /api/csketch/{id}/snap`. */
export interface SnapRequest {
  cursor: Point2d
  radius: number
}

/**
 * In-flight draft entity for inference. Tagged via
 * `#[serde(tag = "kind", rename_all = "snake_case")]` — the
 * discriminator field is literally `kind`, no outer wrapping.
 *
 * Draft entities are NOT inserted into the sketch; they exist only
 * inside the inference pipeline. Lines carry two cursor endpoints,
 * circles carry a centre + radius, points carry a position.
 */
export type DraftEntity =
  | { kind: 'line'; start: Point2d; end: Point2d }
  | { kind: 'circle'; center: Point2d; radius: number }
  | { kind: 'point'; position: Point2d }

/**
 * Which part of a `DraftEntity` a `ProposedConstraint` applies to.
 * Unit enum, `#[serde(rename_all = "snake_case")]`, so the wire
 * form is a plain string.
 */
export type DraftSlot =
  | 'line_start'
  | 'line_end'
  | 'line_self'
  | 'circle_center'
  | 'circle_self'
  | 'point_self'

/**
 * One inferred constraint proposal from
 * `POST /api/csketch/{id}/infer-constraints`. The `constraint`
 * carries the kernel's `GeometricConstraint` directly (re-uses the
 * existing type at the top of this file — every variant except
 * `IntersectionAngle` is a plain string). `target` is `null` for
 * unary proposals (Horizontal / Vertical), otherwise the existing
 * sketch entity the constraint pairs with.
 *
 * `confidence` is in `[0, 1]`: 1.0 for snap-driven proposals (the
 * draft endpoint coincides exactly with an existing feature),
 * `1 - misalignment / angle_tol` for direction-driven proposals
 * (Horizontal, Parallel, …).
 *
 * `reason` is a short human-readable tag from a fixed kernel-side
 * set, suitable as a tooltip without further translation.
 */
export interface ProposedConstraint {
  constraint: GeometricConstraint
  draft_slot: DraftSlot
  target: EntityRef | null
  confidence: number
  reason: string
}

/**
 * Tolerances for inference. Pass `undefined` (or omit the field) to
 * use the kernel defaults: `angle_tol = 3° in radians`,
 * `snap_radius = 5`, `equal_radius_tol = 0.5`.
 *
 * Callers driving inference from a zoomed viewport should scale
 * `snap_radius` and `equal_radius_tol` by the inverse of the
 * current viewport zoom so the world-space catch radius stays
 * constant in screen pixels.
 */
export interface InferenceTolerance {
  angle_tol: number
  snap_radius: number
  equal_radius_tol: number
}

/** Body of `POST /api/csketch/{id}/infer-constraints`. */
export interface InferConstraintsRequest {
  draft: DraftEntity
  tolerance?: InferenceTolerance
}

// ── Wire-level entity-id response ───────────────────────────────────

/** Response shape for `POST /point` / `/line` / `/circle`. */
export interface EntityIdResponse {
  id: string
}

/** Response shape for `POST /constraint`. */
export interface ConstraintIdResponse {
  id: string
}

// ── Typed error ─────────────────────────────────────────────────────

/**
 * Thrown by `csketchApi.updateConstraintValue` when the server
 * returns a 409 `SketchConstraintConflict`. Carries the structured
 * details so callers can surface them in the UI without reparsing
 * the response body.
 *
 * Every other failure path throws a plain `Error` whose message is
 * the server's error message (or `HTTP <status>` if the body is
 * empty / non-JSON), matching the existing `sketch-api.ts` policy.
 */
export class CSketchConstraintConflictError extends Error {
  readonly status = 409
  readonly code = 'sketch_constraint_conflict'
  readonly details: ConstraintUpdateConflict

  constructor(message: string, details: ConstraintUpdateConflict) {
    super(message)
    this.name = 'CSketchConstraintConflictError'
    this.details = details
  }
}

// ── HTTP helpers ────────────────────────────────────────────────────

interface ServerErrorBody {
  code?: string
  message?: string
  error?: string
  details?: unknown
}

async function parseError(resp: Response): Promise<ServerErrorBody> {
  try {
    return (await resp.json()) as ServerErrorBody
  } catch {
    return {}
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const resp = await fetch(`${API_BASE}${path}`, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  if (!resp.ok) {
    const err = await parseError(resp)
    if (resp.status === 409 && err.code === 'sketch_constraint_conflict') {
      throw new CSketchConstraintConflictError(
        err.message || 'sketch constraint conflict',
        err.details as ConstraintUpdateConflict,
      )
    }
    throw new Error(err.message || err.error || `HTTP ${resp.status}`)
  }
  // 204 No Content has no body.
  if (resp.status === 204) {
    return undefined as T
  }
  return (await resp.json()) as T
}

// ── Public API ──────────────────────────────────────────────────────

export const csketchApi = {
  /** Create a fresh empty constrained sketch. Returns its server id. */
  create(): Promise<EntityIdResponse> {
    return request('POST', '/csketch')
  },
  /** List every live csketch id. */
  list(): Promise<string[]> {
    return request('GET', '/csketch')
  },
  /** Full entity snapshot for a sketch. */
  get(id: string): Promise<CSketchSummary> {
    return request('GET', `/csketch/${id}`)
  },
  /** Drop a sketch. 204 on success. */
  delete(id: string): Promise<void> {
    return request('DELETE', `/csketch/${id}`)
  },
  /** Append a point. `fixed: true` pins both DOFs immediately. */
  addPoint(id: string, body: AddPointRequest): Promise<EntityIdResponse> {
    return request('POST', `/csketch/${id}/point`, body)
  },
  /** Append a line segment between two existing points. */
  addLine(id: string, body: AddLineRequest): Promise<EntityIdResponse> {
    return request('POST', `/csketch/${id}/line`, body)
  },
  /** Append a circle by centre + radius. */
  addCircle(id: string, body: AddCircleRequest): Promise<EntityIdResponse> {
    return request('POST', `/csketch/${id}/circle`, body)
  },
  /**
   * Add a constraint. The caller supplies a fully-formed
   * `Constraint` record; the kernel honours the embedded id.
   */
  addConstraint(id: string, body: Constraint): Promise<ConstraintIdResponse> {
    return request('POST', `/csketch/${id}/constraint`, body)
  },
  /** Remove a constraint by id. 204 on success. */
  deleteConstraint(id: string, cid: string): Promise<void> {
    return request('DELETE', `/csketch/${id}/constraint/${cid}`)
  },
  /** List every constraint on a sketch. */
  listConstraints(id: string): Promise<Constraint[]> {
    return request('GET', `/csketch/${id}/constraints`)
  },
  /**
   * Edit the scalar target of a dimensional constraint and re-solve.
   * On a 409 the server has already reverted; this method rethrows a
   * typed `CSketchConstraintConflictError` carrying the conflict
   * payload so callers can surface it without parsing the response.
   */
  updateConstraintValue(
    id: string,
    cid: string,
    value: number,
  ): Promise<ConstraintUpdateResponse> {
    return request('PATCH', `/csketch/${id}/constraint/${cid}/value`, { value })
  },
  /** Run the solver. Pass `undefined` to use server defaults. */
  solve(id: string, options?: SolveOptions): Promise<SketchSolveReport> {
    const body: SolveRequest = options ? { options } : {}
    return request('POST', `/csketch/${id}/solve`, body)
  },
  /**
   * Drag a single entity toward `target` while honouring every
   * other constraint. Uses the framerate-tuned defaults from
   * `SolveOptions::for_drag` when `options` is omitted.
   */
  drag(id: string, body: DragRequest): Promise<SketchSolveReport> {
    return request('POST', `/csketch/${id}/drag`, body)
  },
  /**
   * Pure structural-DOF analysis — does not run Newton-Raphson, so
   * cheap enough to call on every constraint add/remove for a
   * reactive "DOF: 3" badge.
   */
  dof(id: string): Promise<DofReport> {
    return request('GET', `/csketch/${id}/dof`)
  },
  /**
   * Find snap candidates near `cursor` within `radius` sketch
   * units. Results are pre-sorted by (priority, distance) — the
   * first element, if any, is the best snap.
   *
   * Safe to call at viewport-pointermove cadence: the kernel walks
   * every entity store linearly but the work stays well under a
   * millisecond for sketches under ~1k entities.
   */
  snap(id: string, body: SnapRequest): Promise<SnapCandidate[]> {
    return request('POST', `/csketch/${id}/snap`, body)
  },
  /**
   * Propose `GeometricConstraint`s for an in-flight `DraftEntity`.
   * Used by the draw tools to render auto-constrain glyphs near the
   * cursor (⊥, ∥, ⊙, =) while the user drags out a line / circle /
   * point. Pass `tolerance` to override the kernel defaults
   * (3°, 5-unit snap radius, 0.5-unit equal-radius tol).
   *
   * Returns the proposal list in declaration order — caller is
   * expected to filter by `confidence` (≥ 0.6 is the
   * auto-constrain threshold) before promoting any to real
   * constraints on commit.
   */
  inferConstraints(
    id: string,
    body: InferConstraintsRequest,
  ): Promise<ProposedConstraint[]> {
    return request('POST', `/csketch/${id}/infer-constraints`, body)
  },
}

// ── Helpers — value extractors ──────────────────────────────────────

/**
 * Pull the single scalar carried by a `DimensionalConstraint`. Used
 * by editors that need the current value before calling
 * `updateConstraintValue`. Returns `null` for `CenterOfMass` (which
 * carries `{ x, y }` and is not editable via the single-scalar
 * surface).
 */
export function dimensionalScalar(d: DimensionalConstraint): number | null {
  if ('Distance' in d) return d.Distance
  if ('Angle' in d) return d.Angle
  if ('Radius' in d) return d.Radius
  if ('Diameter' in d) return d.Diameter
  if ('Length' in d) return d.Length
  if ('XCoordinate' in d) return d.XCoordinate
  if ('YCoordinate' in d) return d.YCoordinate
  if ('Area' in d) return d.Area
  if ('Perimeter' in d) return d.Perimeter
  if ('ArcLength' in d) return d.ArcLength
  if ('Curvature' in d) return d.Curvature
  if ('Slope' in d) return d.Slope
  if ('OffsetDistance' in d) return d.OffsetDistance
  if ('AspectRatio' in d) return d.AspectRatio
  if ('MinDistance' in d) return d.MinDistance
  if ('MaxDistance' in d) return d.MaxDistance
  if ('MomentOfInertia' in d) return d.MomentOfInertia
  return null
}

/**
 * Pull the scalar from a `Constraint` whose `constraint_type` is
 * dimensional, or `null` when it is geometric or two-valued.
 */
export function constraintScalar(c: Constraint): number | null {
  if ('Dimensional' in c.constraint_type) {
    return dimensionalScalar(c.constraint_type.Dimensional)
  }
  return null
}

/**
 * Discriminator tag for an `EntityRef`. Useful for switch-case
 * dispatch in renderers without rewriting the `in` ladder at every
 * call site.
 */
export type EntityKind =
  | 'Point'
  | 'Line'
  | 'Arc'
  | 'Circle'
  | 'Rectangle'
  | 'Ellipse'
  | 'Spline'
  | 'Polyline'

/** Return the kind tag and id of an `EntityRef`. */
export function entityRefParts(ref: EntityRef): { kind: EntityKind; id: string } {
  if ('Point' in ref) return { kind: 'Point', id: ref.Point }
  if ('Line' in ref) return { kind: 'Line', id: ref.Line }
  if ('Arc' in ref) return { kind: 'Arc', id: ref.Arc }
  if ('Circle' in ref) return { kind: 'Circle', id: ref.Circle }
  if ('Rectangle' in ref) return { kind: 'Rectangle', id: ref.Rectangle }
  if ('Ellipse' in ref) return { kind: 'Ellipse', id: ref.Ellipse }
  if ('Spline' in ref) return { kind: 'Spline', id: ref.Spline }
  return { kind: 'Polyline', id: ref.Polyline }
}

/** Build a `Point`-kinded `EntityRef` for the given point id. */
export function pointRef(id: string): EntityRef {
  return { Point: id }
}
/** Build a `Line`-kinded `EntityRef` for the given line id. */
export function lineRef(id: string): EntityRef {
  return { Line: id }
}
/** Build a `Circle`-kinded `EntityRef` for the given circle id. */
export function circleRef(id: string): EntityRef {
  return { Circle: id }
}
