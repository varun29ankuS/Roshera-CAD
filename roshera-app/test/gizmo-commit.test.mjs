// Run: node --experimental-strip-types --test test/gizmo-commit.test.mjs
//
// The transform gizmo must move a part only when the kernel did. These tests
// drive the commit helper the gizmo calls on mouse-up: the body it sends to
// `POST /api/geometry/transform`, and what happens to the viewport pose when
// the kernel refuses or the transport fails.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Matrix4, Quaternion, Vector3, Euler } from 'three'
import { planKernelTransform, commitGizmoDrag } from '../src/lib/gizmo-commit.ts'

function pose(position, quat = [0, 0, 0, 1], scale = [1, 1, 1]) {
  const e = new Euler().setFromQuaternion(new Quaternion(...quat), 'XYZ')
  return { position, quaternion: quat, rotation: [e.x, e.y, e.z], scale }
}

function quatAxisAngle(axis, angle) {
  const q = new Quaternion().setFromAxisAngle(new Vector3(...axis).normalize(), angle)
  return [q.x, q.y, q.z, q.w]
}

// The kernel's rotation (geometry-engine `Matrix4::rotation_axis`, Rodrigues
// about `center`) followed by the translation, exactly as the route applies
// them: rotation first, then translation.
function kernelApply(body, v) {
  let p = v
  if (body.rotation) {
    const [ax, ay, az] = body.rotation.axis
    const len = Math.hypot(ax, ay, az)
    const [x, y, z] = [ax / len, ay / len, az / len]
    const s = Math.sin(body.rotation.angle)
    const c = Math.cos(body.rotation.angle)
    const t = 1 - c
    const [cx, cy, cz] = body.rotation.center
    const [px, py, pz] = [p[0] - cx, p[1] - cy, p[2] - cz]
    p = [
      (x * x * t + c) * px + (x * y * t - z * s) * py + (x * z * t + y * s) * pz + cx,
      (x * y * t + z * s) * px + (y * y * t + c) * py + (y * z * t - x * s) * pz + cy,
      (x * z * t - y * s) * px + (y * z * t + x * s) * py + (z * z * t + c) * pz + cz,
    ]
  }
  if (body.translation) {
    p = [p[0] + body.translation[0], p[1] + body.translation[1], p[2] + body.translation[2]]
  }
  return p
}

// What three.js displays for kernel vertex `v` under a scene pose.
function displayed(p, v) {
  const m = new Matrix4().compose(
    new Vector3(...p.position),
    new Quaternion(...p.quaternion),
    new Vector3(...p.scale),
  )
  const out = new Vector3(...v).applyMatrix4(m)
  return [out.x, out.y, out.z]
}

function assertClose(a, b, eps = 1e-9) {
  for (let i = 0; i < a.length; i++) {
    assert.ok(Math.abs(a[i] - b[i]) <= eps, `component ${i}: ${a[i]} vs ${b[i]}`)
  }
}

const ID = '11111111-2222-4333-8444-555555555555'
const VERTS = [
  [0, 0, 0],
  [10, 0, 0],
  [0, 7, 0],
  [3, -4, 12],
]

test('a pure translate drag sends exactly the drag delta, no rotation', () => {
  const plan = planKernelTransform(ID, pose([0, 0, 0]), pose([5, -2, 3.5]))
  assert.equal(plan.kind, 'send')
  assert.deepEqual(plan.body, { object: ID, translation: [5, -2, 3.5] })
})

test('a pure rotate drag sends axis/angle about the gizmo pivot, no translation', () => {
  const q = quatAxisAngle([0, 0, 1], Math.PI / 2)
  const plan = planKernelTransform(ID, pose([0, 0, 0]), pose([0, 0, 0], q))
  assert.equal(plan.kind, 'send')
  assert.equal(plan.body.translation, undefined)
  assertClose(plan.body.rotation.axis, [0, 0, 1])
  assert.ok(Math.abs(plan.body.rotation.angle - Math.PI / 2) < 1e-12)
  assert.deepEqual(plan.body.rotation.center, [0, 0, 0])
})

test('the kernel motion lands every vertex where the gizmo displayed it (independent three.js check)', () => {
  const q = quatAxisAngle([1, 2, -0.5], 1.1)
  const end = pose([4, -9, 2], q)
  const plan = planKernelTransform(ID, pose([0, 0, 0]), end)
  assert.equal(plan.kind, 'send')
  for (const v of VERTS) {
    assertClose(kernelApply(plan.body, v), displayed(end, v))
  }
})

test('a rotation past pi is sent as the equivalent short rotation, still landing the part', () => {
  const q = quatAxisAngle([0, 1, 0], 1.9 * Math.PI)
  const end = pose([0, 0, 0], q)
  const plan = planKernelTransform(ID, pose([0, 0, 0]), end)
  assert.equal(plan.kind, 'send')
  assert.ok(plan.body.rotation.angle <= Math.PI + 1e-12)
  for (const v of VERTS) {
    assertClose(kernelApply(plan.body, v), displayed(end, v))
  }
})

test('a part that started at a display offset lands where it was dropped (server re-broadcasts at identity)', () => {
  // Some create paths carry `position` as a display transform over a mesh
  // built at the origin; the transform route re-broadcasts at identity, so
  // the kernel must take the whole displayed pose, not only the drag delta.
  const start = pose([20, 0, 0])
  const end = pose([25, 1, 0], quatAxisAngle([0, 0, 1], 0.3))
  const plan = planKernelTransform(ID, start, end)
  assert.equal(plan.kind, 'send')
  for (const v of VERTS) {
    assertClose(kernelApply(plan.body, v), displayed(end, v))
  }
})

test('a click with no motion sends nothing', () => {
  const q = quatAxisAngle([0, 0, 1], 0.4)
  assert.deepEqual(planKernelTransform(ID, pose([1, 2, 3], q), pose([1, 2, 3], q)), {
    kind: 'unchanged',
  })
})

test('a non-unit display scale is refused: the kernel route carries no scale', () => {
  const plan = planKernelTransform(ID, pose([0, 0, 0]), pose([1, 0, 0], [0, 0, 0, 1], [2, 1, 1]))
  assert.equal(plan.kind, 'refuse')
  assert.match(plan.reason, /scale/)
  assert.ok(!plan.reason.includes('  '))
})

function harness(fetchImpl) {
  const calls = { fetch: [], restored: [], reports: [] }
  const deps = {
    fetchFn: async (url, init) => {
      calls.fetch.push({ url, init })
      return fetchImpl(url, init)
    },
    apiBase: '/api',
    restore: (p) => calls.restored.push(p),
    report: (line) => calls.reports.push(line),
  }
  return { deps, calls }
}

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

test('a successful commit POSTs the relative body to /api/geometry/transform and leaves the pose to the server', async () => {
  const { deps, calls } = harness(() =>
    jsonResponse(200, { success: true, object: ID, solid_id: 3, perception: {} }),
  )
  const start = pose([0, 0, 0])
  const end = pose([5, 0, 0])
  const outcome = await commitGizmoDrag(deps, ID, start, end)
  assert.equal(outcome, 'committed')
  assert.equal(calls.fetch.length, 1)
  assert.equal(calls.fetch[0].url, '/api/geometry/transform')
  assert.equal(calls.fetch[0].init.method, 'POST')
  assert.deepEqual(JSON.parse(calls.fetch[0].init.body), { object: ID, translation: [5, 0, 0] })
  assert.equal(calls.restored.length, 0)
  assert.equal(calls.reports.length, 0)
})

test('a transport failure snaps the part back to its pre-drag pose and says so', async () => {
  const { deps, calls } = harness(() => {
    throw new TypeError('Failed to fetch')
  })
  const start = pose([1, 2, 3])
  const outcome = await commitGizmoDrag(deps, ID, start, pose([9, 2, 3]))
  assert.equal(outcome, 'refused')
  assert.deepEqual(calls.restored, [start])
  assert.equal(calls.reports.length, 1)
  assert.match(calls.reports[0], /unreachable/)
  assert.ok(!calls.reports[0].includes('  '))
})

test('a kernel refusal snaps the part back and shows the kernel message', async () => {
  const { deps, calls } = harness(() =>
    jsonResponse(409, {
      success: false,
      error_code: 'unsound_base',
      error: 'transform refused: the base solid is unsound',
    }),
  )
  const start = pose([0, 0, 0])
  const outcome = await commitGizmoDrag(deps, ID, start, pose([0, 4, 0]))
  assert.equal(outcome, 'refused')
  assert.deepEqual(calls.restored, [start])
  assert.match(calls.reports[0], /the base solid is unsound/)
})

test('a 200 that does not say success is not a success', async () => {
  const { deps, calls } = harness(() => jsonResponse(200, { success: false, message: 'no' }))
  const start = pose([0, 0, 0])
  assert.equal(await commitGizmoDrag(deps, ID, start, pose([0, 0, 1])), 'refused')
  assert.deepEqual(calls.restored, [start])
})

test('a scaled pose is refused locally and snapped back without a request', async () => {
  const { deps, calls } = harness(() => jsonResponse(200, { success: true }))
  const start = pose([0, 0, 0], [0, 0, 0, 1], [1, 1, 1])
  const outcome = await commitGizmoDrag(deps, ID, start, pose([0, 0, 0], [0, 0, 0, 1], [1.5, 1, 1]))
  assert.equal(outcome, 'refused')
  assert.equal(calls.fetch.length, 0)
  assert.deepEqual(calls.restored, [start])
})

test('a click with no motion makes no request and moves nothing', async () => {
  const { deps, calls } = harness(() => jsonResponse(200, { success: true }))
  const p = pose([1, 1, 1])
  assert.equal(await commitGizmoDrag(deps, ID, p, pose([1, 1, 1])), 'unchanged')
  assert.equal(calls.fetch.length, 0)
  assert.equal(calls.restored.length, 0)
})

// ─── Round 1: the absolute pose is only valid over the kernel's last mesh ───
//
// The body is the drag-end pose over the mesh the viewport holds. After a
// commit the kernel holds the moved geometry; until the server's rebroadcast
// of that geometry lands, a second drag would compose its pose on the stale
// mesh and the kernel would take the first move twice.
import { createGizmoSession } from '../src/lib/gizmo-commit.ts'

function sessionHarness({ meshAfterCommit, resyncMesh } = {}) {
  const meshes = new Map([[ID, { v: 0 }]])
  const calls = { fetch: [], restored: [], reports: [], resyncs: 0 }
  const session = createGizmoSession({
    fetchFn: async (url, init) => {
      calls.fetch.push(JSON.parse(init.body))
      if (meshAfterCommit) meshes.set(ID, meshAfterCommit())
      return jsonResponse(200, { success: true, object: ID })
    },
    apiBase: '/api',
    restore: (id, p) => calls.restored.push(p),
    report: (line) => calls.reports.push(line),
    readMesh: (id) => meshes.get(id),
    wait: async () => {},
    resync: async () => {
      calls.resyncs += 1
      if (resyncMesh) meshes.set(ID, resyncMesh())
    },
    settleTimeoutMs: 100,
    pollMs: 10,
  })
  return { session, calls, meshes }
}

test('a second drag before the rebroadcast lands does not send a pose composed on the stale mesh', async () => {
  // The rebroadcast never arrives and the resync cannot deliver it either.
  const { session, calls } = sessionHarness()
  assert.equal(await session.drag(ID, pose([0, 0, 0]), pose([5, 0, 0])), 'committed')
  assert.equal(calls.resyncs, 1)
  assert.equal(session.isStale(ID), true)
  const second = pose([5, 0, 0])
  assert.equal(await session.drag(ID, second, pose([8, 0, 0])), 'refused')
  assert.equal(calls.fetch.length, 1, 'the second pose must not reach the kernel')
  assert.deepEqual(calls.restored, [second])
  assert.ok(calls.reports.some((l) => /has not received/.test(l)))
  assert.ok(calls.reports.every((l) => !l.includes('  ')))
})

test('a rebroadcast that lands with the reply re-arms the gizmo at once, no resync', async () => {
  const { session, calls } = sessionHarness({ meshAfterCommit: () => ({ v: 1 }) })
  assert.equal(await session.drag(ID, pose([0, 0, 0]), pose([5, 0, 0])), 'committed')
  assert.equal(calls.resyncs, 0)
  assert.equal(session.isStale(ID), false)
  assert.equal(await session.drag(ID, pose([0, 0, 0]), pose([0, 3, 0])), 'committed')
  assert.equal(calls.fetch.length, 2)
})

test('a missed rebroadcast is recovered from the scene snapshot before the next drag', async () => {
  const { session, calls } = sessionHarness({ resyncMesh: () => ({ v: 2 }) })
  assert.equal(await session.drag(ID, pose([0, 0, 0]), pose([5, 0, 0])), 'committed')
  assert.equal(calls.resyncs, 1)
  assert.equal(session.isStale(ID), false)
  assert.equal(await session.drag(ID, pose([0, 0, 0]), pose([0, 3, 0])), 'committed')
  assert.equal(calls.fetch.length, 2)
})

test('a stale part unlocks once any later server mesh for it lands', async () => {
  const { session, meshes } = sessionHarness()
  await session.drag(ID, pose([0, 0, 0]), pose([5, 0, 0]))
  assert.equal(session.isStale(ID), true)
  meshes.set(ID, { v: 9 })
  assert.equal(session.isStale(ID), false)
})

test('a drag released while a commit is still settling is not sent', async () => {
  let release
  const gate = new Promise((r) => (release = r))
  const meshes = new Map([[ID, { v: 0 }]])
  const fetched = []
  const restored = []
  const session = createGizmoSession({
    fetchFn: async (url, init) => {
      fetched.push(init.body)
      await gate
      meshes.set(ID, { v: 1 })
      return jsonResponse(200, { success: true })
    },
    apiBase: '/api',
    restore: (id, p) => restored.push(p),
    report: () => {},
    readMesh: (id) => meshes.get(id),
    wait: async () => {},
    resync: async () => {},
    settleTimeoutMs: 100,
    pollMs: 10,
  })
  const first = session.drag(ID, pose([0, 0, 0]), pose([5, 0, 0]))
  assert.equal(session.isBusy(), true)
  const start2 = pose([5, 0, 0])
  assert.equal(await session.drag(ID, start2, pose([9, 0, 0])), 'refused')
  assert.deepEqual(restored, [start2])
  release()
  assert.equal(await first, 'committed')
  assert.equal(fetched.length, 1)
  assert.equal(session.isBusy(), false)
})
