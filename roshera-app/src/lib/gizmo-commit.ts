/**
 * Transform-gizmo commit: turn a finished drag into a kernel transform.
 *
 * The gizmo moves the three.js object while the user drags; that pose is a
 * proposal, not a fact. On release the proposal goes to the same route the
 * agent uses — `POST /api/geometry/transform` (certified, gated, recorded) —
 * and the part stays where it was dropped only if the kernel moved it. The
 * route mutates the solid and re-broadcasts its mesh under the same UUID at
 * an identity display transform, so on success the viewport pose comes from
 * that `ObjectCreated` frame, never from the local drag. On a refusal or a
 * transport failure the part snaps back to its pre-drag pose and the reason
 * is reported.
 *
 * Geometry. A kernel solid's vertices `v` are drawn at `M·v` where `M` is the
 * scene pose (position `p`, quaternion `q`, unit scale). Most solids are drawn
 * at identity; some create paths carry a display offset over a mesh built at
 * the origin. Because the route re-broadcasts at identity, the kernel must
 * take the whole drag-end pose `M1` (rotation `q1` about the origin, then
 * translation `p1`) for the part to land where it was dropped. For a part
 * that started at identity that is exactly the drag delta. The route applies
 * rotation first, then translation — the same order as `M1 = T(p1)·R(q1)`.
 *
 * The route has no scale, so a pose with a non-unit scale cannot be committed
 * and is refused here rather than sent without its scale.
 *
 * Pure module (no `@/` imports, no `import.meta.env`) so it runs under
 * `node --test` as well as in the app.
 */

import { refusalMessage, tryReadJson } from './backend-refusal.ts'

export type Vec3 = [number, number, number]
/** Quaternion as `[x, y, z, w]` (three.js component order). */
export type Quat = [number, number, number, number]

/** The scene pose of an object, as the gizmo and the scene store see it. */
export interface GizmoPose {
  position: Vec3
  quaternion: Quat
  /** Euler XYZ, radians — the scene store's rotation representation. */
  rotation: Vec3
  scale: Vec3
}

/** Request body for `POST /api/geometry/transform`. */
export interface KernelTransformBody {
  object: string
  translation?: Vec3
  rotation?: { axis: Vec3; angle: number; center: Vec3 }
}

export type TransformPlan =
  | { kind: 'unchanged' }
  | { kind: 'refuse'; reason: string }
  | { kind: 'send'; body: KernelTransformBody }

/** Below this, a length or a rotation's sine of half-angle is zero motion. */
const MOTION_EPS = 1e-12
/** Tolerance for "scale is exactly one" on a pose the backend always ships as 1. */
const UNIT_SCALE_EPS = 1e-9

function isUnitScale(s: Vec3): boolean {
  return s.every((c) => Math.abs(c - 1) <= UNIT_SCALE_EPS)
}

function samePose(a: GizmoPose, b: GizmoPose): boolean {
  const dp = Math.hypot(
    a.position[0] - b.position[0],
    a.position[1] - b.position[1],
    a.position[2] - b.position[2],
  )
  if (dp > MOTION_EPS) return false
  const na = Math.hypot(...a.quaternion)
  const nb = Math.hypot(...b.quaternion)
  const dot =
    (a.quaternion[0] * b.quaternion[0] +
      a.quaternion[1] * b.quaternion[1] +
      a.quaternion[2] * b.quaternion[2] +
      a.quaternion[3] * b.quaternion[3]) /
    (na * nb)
  // q and -q are the same rotation.
  return Math.abs(dot) >= 1 - MOTION_EPS
}

/**
 * Decide what, if anything, to send for a drag from `start` to `end`.
 * Pure: no I/O.
 */
export function planKernelTransform(
  objectId: string,
  start: GizmoPose,
  end: GizmoPose,
): TransformPlan {
  if (!isUnitScale(start.scale) || !isUnitScale(end.scale)) {
    return {
      kind: 'refuse',
      reason:
        'the kernel transform has no scale, so a scaled pose cannot be committed',
    }
  }
  if (![...end.position, ...end.quaternion].every(Number.isFinite)) {
    return { kind: 'refuse', reason: 'the gizmo produced a non-finite pose' }
  }
  if (samePose(start, end)) return { kind: 'unchanged' }

  const body: KernelTransformBody = { object: objectId }

  let [x, y, z, w] = end.quaternion
  const n = Math.hypot(x, y, z, w)
  if (!Number.isFinite(n) || n === 0) {
    return { kind: 'refuse', reason: 'the gizmo produced a degenerate rotation' }
  }
  x /= n
  y /= n
  z /= n
  w /= n
  // Take the short way round: q and -q are the same rotation, and w >= 0
  // keeps the angle in [0, pi].
  if (w < 0) {
    x = -x
    y = -y
    z = -z
    w = -w
  }
  const s = Math.hypot(x, y, z)
  if (s > MOTION_EPS) {
    body.rotation = {
      axis: [x / s, y / s, z / s],
      angle: 2 * Math.atan2(s, w),
      center: [0, 0, 0],
    }
  }

  const [px, py, pz] = end.position
  if (Math.hypot(px, py, pz) > MOTION_EPS) {
    body.translation = [px, py, pz]
  }

  // The drag ended on the kernel's own placement (a display-offset part
  // dragged back onto its kernel geometry): the kernel already holds this
  // pose, so there is nothing to ask it to do.
  if (!body.rotation && !body.translation) return { kind: 'unchanged' }
  return { kind: 'send', body }
}

export interface GizmoCommitDeps {
  /** Resolved at call time so the app's authenticated fetch applies. */
  fetchFn: (input: string, init: RequestInit) => Promise<Response>
  /** API root, e.g. `/api`. */
  apiBase: string
  /** Put the object back at `pose` in the viewport. */
  restore: (pose: GizmoPose) => void
  /** Show one line to the user. */
  report: (line: string) => void
}

export type GizmoCommitOutcome = 'unchanged' | 'committed' | 'refused'

/**
 * Commit a finished drag. Resolves `committed` only when the backend
 * answered success; on anything else the object is restored to `start` and
 * the reason reported.
 */
export async function commitGizmoDrag(
  deps: GizmoCommitDeps,
  objectId: string,
  start: GizmoPose,
  end: GizmoPose,
): Promise<GizmoCommitOutcome> {
  const plan = planKernelTransform(objectId, start, end)
  if (plan.kind === 'unchanged') return 'unchanged'
  if (plan.kind === 'refuse') {
    deps.restore(start)
    deps.report(`Transform refused: ${plan.reason}. The part is back where it was.`)
    return 'refused'
  }

  let resp: Response
  try {
    resp = await deps.fetchFn(`${deps.apiBase}/geometry/transform`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(plan.body),
    })
  } catch {
    deps.restore(start)
    deps.report('Transform failed: backend unreachable. The part is back where it was.')
    return 'refused'
  }

  const body = await tryReadJson(resp)
  if (!resp.ok || !body || body.success !== true) {
    deps.restore(start)
    const reason = refusalMessage(body, resp.status).replace(/\.+$/, '')
    deps.report(`Transform refused: ${reason}. The part is back where it was.`)
    return 'refused'
  }
  return 'committed'
}

export interface GizmoSessionDeps {
  fetchFn: GizmoCommitDeps['fetchFn']
  apiBase: string
  /** Put `objectId` back at `pose` in the viewport. */
  restore: (objectId: string, pose: GizmoPose) => void
  /** Show one line to the user. */
  report: (line: string) => void
  /**
   * The mesh buffer the viewport currently holds for `objectId`, compared by
   * identity: every server upsert of the part installs a new buffer.
   */
  readMesh: (objectId: string) => unknown
  /** Resolve after `ms` milliseconds. */
  wait: (ms: number) => Promise<void>
  /** Reload the scene from the server snapshot (`/api/scene/snapshot`). */
  resync: () => Promise<void>
  /** How long to wait for the rebroadcast before resyncing. */
  settleTimeoutMs: number
  /** How often to look for it meanwhile. */
  pollMs: number
}

export type GizmoDragOutcome = GizmoCommitOutcome

export interface GizmoSession {
  /**
   * Commit a finished drag, then hold until the viewport holds the kernel's
   * new geometry for the part. Refuses (and restores `start`) while another
   * commit is settling or while the part's last move has not reached the
   * viewport.
   */
  drag: (objectId: string, start: GizmoPose, end: GizmoPose) => Promise<GizmoDragOutcome>
  /** A commit is in flight or settling. */
  isBusy: () => boolean
  /**
   * The kernel moved this part and the viewport has not received the result:
   * its pose is not a basis for another commit. Clears itself once any later
   * server mesh for the part lands.
   */
  isStale: (objectId: string) => boolean
}

/**
 * The body `planKernelTransform` builds is the drag-end pose over the mesh
 * the viewport holds, which is exact only while that mesh is the kernel's
 * current geometry. After a commit the kernel holds the moved geometry and
 * the viewport shows the old mesh at the dropped pose until the server's
 * rebroadcast lands; a drag composed in that window would send the first
 * move a second time. The session closes the window: after a commit it waits
 * (bounded) for a new mesh, falls back to a snapshot resync, and if even
 * that does not deliver, marks the part stale and refuses to commit it.
 */
export function createGizmoSession(deps: GizmoSessionDeps): GizmoSession {
  let busy = false
  // objectId -> the mesh buffer that is known NOT to reflect the kernel.
  const staleMesh = new Map<string, unknown>()

  const isStale = (objectId: string): boolean => {
    if (!staleMesh.has(objectId)) return false
    if (deps.readMesh(objectId) !== staleMesh.get(objectId)) {
      staleMesh.delete(objectId)
      return false
    }
    return true
  }

  const settle = async (objectId: string, meshBefore: unknown): Promise<void> => {
    for (let waited = 0; ; waited += deps.pollMs) {
      if (deps.readMesh(objectId) !== meshBefore) return
      if (waited >= deps.settleTimeoutMs) break
      await deps.wait(deps.pollMs)
    }
    await deps.resync()
    if (deps.readMesh(objectId) !== meshBefore) return
    staleMesh.set(objectId, meshBefore)
    deps.report(
      'The kernel moved the part, but the viewport has not received the result. ' +
        'The part is locked until it arrives; reload the page if it does not.',
    )
  }

  const drag = async (
    objectId: string,
    start: GizmoPose,
    end: GizmoPose,
  ): Promise<GizmoDragOutcome> => {
    if (busy) {
      deps.restore(objectId, start)
      deps.report('Transform not sent: the previous move is still being applied.')
      return 'refused'
    }
    if (isStale(objectId)) {
      deps.restore(objectId, start)
      deps.report(
        'Transform not sent: the viewport has not received the kernel’s last move of this part.',
      )
      return 'refused'
    }
    busy = true
    try {
      // Captured at release, before the request: the rebroadcast of THIS
      // commit cannot land before it is sent.
      const meshBefore = deps.readMesh(objectId)
      const outcome = await commitGizmoDrag(
        {
          fetchFn: deps.fetchFn,
          apiBase: deps.apiBase,
          restore: (pose) => deps.restore(objectId, pose),
          report: deps.report,
        },
        objectId,
        start,
        end,
      )
      if (outcome === 'committed') await settle(objectId, meshBefore)
      return outcome
    } finally {
      busy = false
    }
  }

  return { drag, isBusy: () => busy, isStale }
}
