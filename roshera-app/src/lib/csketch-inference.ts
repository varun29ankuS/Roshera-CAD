/**
 * D-2-d frontend driver for the kernel's auto-constrain pipeline.
 *
 * After every entity commit (point / line / circle) the overlay
 * calls `applyInferredConstraints` with:
 *
 *   - the kernel csketch id,
 *   - the freshly-committed entity rendered as a `DraftEntity`,
 *   - the entity-id mapping for the slots that draft can speak
 *     about (`point_self`, `line_start`, `line_end`, `line_self`,
 *     `circle_self`),
 *   - the store-bound `addCSketchConstraint` action.
 *
 * The helper asks `POST /csketch/:id/infer-constraints` for
 * proposals, filters by confidence, and walks each surviving
 * proposal through `addCSketchConstraint`. The store refreshes the
 * active summary + active constraints after every add, so the
 * existing `CSketchGeometricBadges` view picks the new constraints
 * up automatically — there is no extra wiring on the render side.
 *
 * `circle_center` proposals are skipped for now: the centre of a
 * `ParametricCircle2d` is not exposed as a separate `Point` entity
 * in the csketch summary, so the frontend can't manufacture an
 * `EntityRef` for it without a kernel-side lookup. That lookup
 * lives in a follow-up slice — listing it here so future readers
 * know it's intentional, not an oversight.
 */

import {
  circleRef,
  csketchApi,
  lineRef,
  pointRef,
  type Constraint,
  type DraftEntity,
  type EntityRef,
  type ProposedConstraint,
} from './csketch-api'

/**
 * Minimum proposal confidence we will auto-apply. The kernel
 * reports `1.0` for snap-driven (exact-coincidence) proposals and
 * `1 - misalignment / angle_tol` for direction-driven proposals
 * (Horizontal, Vertical, Parallel, …). A threshold of `0.5` keeps
 * the strong half — borderline direction proposals at >50% of the
 * angle tolerance get dropped to avoid surprising the user.
 *
 * Mirrors the Fusion / SolidWorks default of "apply confident
 * proposals silently, never the marginal ones".
 */
const AUTO_APPLY_CONFIDENCE_THRESHOLD = 0.5

/**
 * Entity-id mapping for the slots a `DraftEntity` may carry.
 *
 *   - `point`: `point_self` only.
 *   - `line`: `line_self`, `line_start`, `line_end`.
 *   - `circle`: `circle_self` (and one day `circle_center` — see
 *     module doc).
 *
 * The caller fills in whichever slots are relevant; missing slots
 * cause matching proposals to be skipped (logged at debug level).
 */
export interface InferenceRefs {
  point_self?: string
  line_self?: string
  line_start?: string
  line_end?: string
  circle_self?: string
}

/**
 * Translate one `ProposedConstraint` to a kernel `Constraint`,
 * looking up the draft slot's entity id from `refs`. Returns
 * `null` if the slot is unmapped (e.g. `circle_center`).
 *
 * For unary constraints (`Horizontal`, `Vertical`) the kernel
 * accepts a single-entity body; for binary constraints
 * (`Coincident`, `PointOnCurve`, …) the body is `[draft, target]`
 * in the order the inference engine reports them. `IntersectionAngle`
 * carries a scalar inside the geometric variant, so the
 * `constraint_type` wrapping handles it transparently.
 */
function proposalToConstraint(
  proposal: ProposedConstraint,
  refs: InferenceRefs,
): Constraint | null {
  const draftRef = draftSlotToRef(proposal.draft_slot, refs)
  if (draftRef === null) return null
  const entities: EntityRef[] =
    proposal.target === null ? [draftRef] : [draftRef, proposal.target]
  return {
    id: crypto.randomUUID(),
    constraint_type: { Geometric: proposal.constraint },
    entities,
    // Medium priority matches the kernel's default for manually
    // added constraints — inferred ones should not outrank
    // user-pinned `Required` fixes nor get relaxed before low-prio
    // drag pulls.
    priority: 'Medium',
    // The solver re-evaluates status on the next solve cycle
    // triggered by the store's refresh after addConstraint; the
    // initial value is a placeholder.
    status: 'Satisfied',
    name: null,
  }
}

function draftSlotToRef(
  slot: ProposedConstraint['draft_slot'],
  refs: InferenceRefs,
): EntityRef | null {
  switch (slot) {
    case 'point_self':
      return refs.point_self ? pointRef(refs.point_self) : null
    case 'line_self':
      return refs.line_self ? lineRef(refs.line_self) : null
    case 'line_start':
      return refs.line_start ? pointRef(refs.line_start) : null
    case 'line_end':
      return refs.line_end ? pointRef(refs.line_end) : null
    case 'circle_self':
      return refs.circle_self ? circleRef(refs.circle_self) : null
    case 'circle_center':
      // Centre point of a ParametricCircle2d is not surfaced as a
      // Point entity in the csketch summary — needs a kernel-side
      // lookup we haven't wired yet. See module doc.
      return null
  }
}

/**
 * Run inference for `draft`, filter by confidence, apply each
 * surviving proposal via `addConstraint`. Returns the number of
 * constraints actually added so callers can surface it in a toast
 * if they want — the overlay currently doesn't.
 *
 * Failures are logged and swallowed: the entity commit that
 * triggered this call has already succeeded, so an inference
 * failure must not surface to the user as a draw-error.
 */
export async function applyInferredConstraints(
  id: string,
  draft: DraftEntity,
  refs: InferenceRefs,
  addConstraint: (sketchId: string, constraint: Constraint) => Promise<string>,
): Promise<number> {
  let proposals: ProposedConstraint[]
  try {
    proposals = await csketchApi.inferConstraints(id, { draft })
  } catch (err) {
    console.error('[csketch-inference] infer-constraints failed:', err)
    return 0
  }
  let applied = 0
  for (const p of proposals) {
    if (p.confidence < AUTO_APPLY_CONFIDENCE_THRESHOLD) continue
    const constraint = proposalToConstraint(p, refs)
    if (constraint === null) continue
    try {
      // Serial await — the solver re-runs after each constraint
      // and a conflicting proposal from a later iteration would
      // otherwise race against the previous re-solve. Sequential
      // adds also let the kernel reject downstream proposals that
      // an earlier one made redundant (DOF tracking handles this).
      await addConstraint(id, constraint)
      applied += 1
    } catch (err) {
      console.error(
        '[csketch-inference] auto-apply',
        p.reason,
        'failed:',
        err,
      )
    }
  }
  return applied
}
