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
 * Mirrors the mainstream parametric-CAD default of "apply confident
 * proposals silently, never the marginal ones".
 *
 * This threshold exists ONLY on this client. An agent driving the same
 * sketch through MCP/REST calls `infer-constraints` directly and sees
 * every proposal the kernel returns, unfiltered — same kernel, same
 * sketch, two different constraint sets depending on who is driving.
 * Whether the backend should own this number (so both clients agree)
 * or each client should keep its own is a product decision that has
 * not been made; moving it would need backend changes this module
 * cannot make unilaterally. Until that is decided, `applyInferredConstraints`
 * reports what it drops (see `FilteredProposal`) so the gap is visible
 * in the UI instead of silent — never applied without the human able
 * to see the kernel proposed more.
 *
 * Exported (not module-private) so `stores/scene-store.ts` — which
 * stamps this number onto every `InferenceFilterSummary` it records —
 * reads the real constant instead of a second hard-coded `0.5` the two
 * files could quietly drift apart on.
 */
export const AUTO_APPLY_CONFIDENCE_THRESHOLD = 0.5

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
 * A kernel-proposed constraint this client did NOT apply because its
 * confidence fell below {@link AUTO_APPLY_CONFIDENCE_THRESHOLD}. `reason`
 * is the kernel's own short tag (`ProposedConstraint.reason`); `kind`
 * names which `GeometricConstraint` variant was proposed (`Horizontal`,
 * `Parallel`, …) so the disclosure UI can say WHAT was dropped, not just
 * how many. This is the record of the divergence described on
 * {@link AUTO_APPLY_CONFIDENCE_THRESHOLD} — a proposal the kernel made
 * and an agent driving the same sketch would have kept.
 */
export interface FilteredProposal {
  kind: string
  reason: string
  confidence: number
}

/**
 * What one `applyInferredConstraints` call did: how many proposals were
 * applied, and — the disclosure this function exists to make possible —
 * exactly which ones were dropped on confidence and why.
 */
export interface InferenceApplyResult {
  applied: number
  filtered: FilteredProposal[]
}

/**
 * Run inference for `draft`, filter by confidence, apply each
 * surviving proposal via `addConstraint`. Returns both the count
 * applied and the full list of proposals dropped on confidence, so
 * the caller can render the disclosure instead of letting a
 * kernel-proposed constraint vanish without a trace.
 *
 * Failures are logged and swallowed: the entity commit that
 * triggered this call has already succeeded, so an inference
 * failure must not surface to the user as a draw-error. A dropped-on-
 * confidence proposal is not a failure — it is reported in `filtered`,
 * not the console.
 */
export async function applyInferredConstraints(
  id: string,
  draft: DraftEntity,
  refs: InferenceRefs,
  addConstraint: (sketchId: string, constraint: Constraint) => Promise<string>,
): Promise<InferenceApplyResult> {
  let proposals: ProposedConstraint[]
  try {
    proposals = await csketchApi.inferConstraints(id, { draft })
  } catch (err) {
    console.error('[csketch-inference] infer-constraints failed:', err)
    return { applied: 0, filtered: [] }
  }
  let applied = 0
  const filtered: FilteredProposal[] = []
  for (const p of proposals) {
    if (p.confidence < AUTO_APPLY_CONFIDENCE_THRESHOLD) {
      filtered.push({
        kind: proposalKind(p),
        reason: p.reason,
        confidence: p.confidence,
      })
      continue
    }
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
  return { applied, filtered }
}

/** Human-readable constraint variant name for the disclosure UI.
 *  `GeometricConstraint` is a string for every variant except
 *  `IntersectionAngle`, which wraps a scalar in an object — the only
 *  case that needs unwrapping here. */
function proposalKind(p: ProposedConstraint): string {
  return typeof p.constraint === 'string'
    ? p.constraint
    : Object.keys(p.constraint)[0]
}
