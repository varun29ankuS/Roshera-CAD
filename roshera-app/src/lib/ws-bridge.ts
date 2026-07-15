/**
 * WebSocket → Scene Store bridge.
 * Connects on mount, routes incoming ServerMessages to the appropriate stores,
 * and converts backend CADObject format to the scene store format.
 */
import { wsClient, setResyncHook } from './ws-client'
import { useWSStore } from '@/stores/ws-store'
import { useSceneStore, type CADObject, type CADMesh, type CADMaterial, type AnalyticalGeometry } from '@/stores/scene-store'
import { useChatStore } from '@/stores/chat-store'
import { sketchApi } from './sketch-api'
import type {
  CADObject as ProtocolCADObject,
  MaterialProperties,
  Transform3D,
  MeshData,
  AnalyticalGeometry as ProtocolAnalyticalGeometry,
} from './protocol'
import type { ServerMessage } from './ws-schemas'
import { Quaternion, Euler } from 'three'

// ─── Type conversion: backend → frontend ────────────────────────────

function convertMesh(mesh: MeshData): CADMesh {
  return {
    vertices: new Float32Array(mesh.vertices),
    indices: new Uint32Array(mesh.indices),
    normals: new Float32Array(mesh.normals),
    faceIds: mesh.face_ids ? new Uint32Array(mesh.face_ids) : undefined,
  }
}

function rgbaToHex(rgba: [number, number, number, number]): string {
  const r = Math.round(rgba[0] * 255)
  const g = Math.round(rgba[1] * 255)
  const b = Math.round(rgba[2] * 255)
  return `#${r.toString(16).padStart(2, '0')}${g.toString(16).padStart(2, '0')}${b.toString(16).padStart(2, '0')}`
}

// Default neutral material for objects that ship without one — the
// agent/MCP build path (revolve, create_*) and the /api/scene/snapshot
// serialization don't always attach a material, and a missing one must
// NOT crash the whole scene convert (it used to throw on `mat.diffuse_color`
// and, paired with a clear-then-build resync, blanked the viewport — the
// "appears then disappears on reconnect" bug). Mirrors the scene-eye's grey.
const DEFAULT_MATERIAL: CADMaterial = {
  color: '#9a9a9a',
  metalness: 0.1,
  roughness: 0.6,
  opacity: 1,
}

function convertMaterial(mat: MaterialProperties | null | undefined): CADMaterial {
  if (!mat || !mat.diffuse_color) return { ...DEFAULT_MATERIAL }
  return {
    color: rgbaToHex(mat.diffuse_color),
    metalness: mat.metallic,
    roughness: mat.roughness,
    opacity: mat.diffuse_color[3],
  }
}

function quaternionToEuler(q: [number, number, number, number]): [number, number, number] {
  const quat = new Quaternion(q[0], q[1], q[2], q[3])
  const euler = new Euler().setFromQuaternion(quat, 'XYZ')
  return [euler.x, euler.y, euler.z]
}

function convertTransform(t: Transform3D): {
  position: [number, number, number]
  rotation: [number, number, number]
  scale: [number, number, number]
} {
  return {
    position: t.translation,
    rotation: quaternionToEuler(t.rotation),
    scale: t.scale,
  }
}

function convertAnalyticalGeometry(
  ag: ProtocolAnalyticalGeometry | undefined,
): AnalyticalGeometry | undefined {
  if (!ag) return undefined
  return {
    type: ag.primitive_type,
    // NURBS / mesh / imported parts carry no analytic parameters (null);
    // the scene store's params is a plain map, so default to empty.
    params: ag.parameters ?? {},
    solidId: ag.solid_id,
  }
}

// ─── Dimension echo formatter ───────────────────────────────────────
//
// Format an `Object created` chat message from a freshly-broadcast
// proto payload. Backend ships authoritative bbox + volume in
// `analytical_geometry.properties` for every primitive / op result;
// when absent (raw mesh imports, etc.) we fall back to a vertex scan.
// Volume is omitted in the fallback path because there's no robust way
// to recover it from a tessellated mesh without reconstructing topology.
function bboxFromVertices(verts: number[]): {
  min: [number, number, number]
  max: [number, number, number]
} | null {
  if (verts.length < 3 || verts.length % 3 !== 0) return null
  let minX = Infinity, minY = Infinity, minZ = Infinity
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity
  for (let i = 0; i < verts.length; i += 3) {
    const x = verts[i], y = verts[i + 1], z = verts[i + 2]
    if (x < minX) minX = x; if (x > maxX) maxX = x
    if (y < minY) minY = y; if (y > maxY) maxY = y
    if (z < minZ) minZ = z; if (z > maxZ) maxZ = z
  }
  return { min: [minX, minY, minZ], max: [maxX, maxY, maxZ] }
}

function fmtNum(n: number): string {
  // 0–999.99 → up to 2 decimals; ≥1000 → no decimals (compact for chat)
  if (Math.abs(n) >= 1000) return n.toFixed(0)
  return n.toFixed(2).replace(/\.?0+$/, '')
}

function dimensionEchoMessage(proto: ProtocolCADObject): string | null {
  const props = proto.analytical_geometry?.properties
  const bbox = props?.bounding_box ?? bboxFromVertices(proto.mesh.vertices)
  if (!bbox) return null
  const dx = bbox.max[0] - bbox.min[0]
  const dy = bbox.max[1] - bbox.min[1]
  const dz = bbox.max[2] - bbox.min[2]
  const dims = `${fmtNum(dx)} × ${fmtNum(dy)} × ${fmtNum(dz)} mm`
  const vol = props?.volume
  const triCount = Math.floor(proto.mesh.indices.length / 3)
  const parts = [`Created **${proto.name}** — ${dims}`]
  if (typeof vol === 'number' && Number.isFinite(vol) && vol > 0) {
    parts.push(`volume ${fmtNum(vol)} mm³`)
  }
  parts.push(`${triCount} tris`)
  return parts.join(' · ')
}

function convertCADObject(proto: ProtocolCADObject): CADObject {
  const { position, rotation, scale } = convertTransform(proto.transform)
  return {
    id: proto.id,
    name: proto.name,
    objectType: proto.analytical_geometry?.primitive_type || 'mesh',
    mesh: convertMesh(proto.mesh),
    analyticalGeometry: convertAnalyticalGeometry(proto.analytical_geometry),
    material: convertMaterial(proto.material),
    position,
    rotation,
    scale,
    visible: proto.visible,
    locked: proto.locked,
    parentId: proto.parent,
  }
}

// ─── WS churn batching ──────────────────────────────────────────────
//
// A part-churn flood (rapid ObjectCreated/ObjectUpdated/ObjectDeleted/
// GeometryUpdate frames — e.g. an eval suite creating/tearing down ~60
// heavy-mesh parts back to back) used to call `scene.addObject` /
// `updateObject` / `removeObject` once PER MESSAGE. Each call clones the
// whole `objects` Map (O(n)) and forces a React commit that re-uploads
// every touched mesh's GPU buffers — serialized into one synchronous
// pile of work per animation frame, which froze the tab.
//
// Instead, geometry-lifecycle messages are folded into `pendingOps`
// keyed by object id. A `Map.set` for an id that's already pending
// simply overwrites the previous entry — this alone implements the two
// ordering rules: "an update after a create is merged" (the later
// upsert record replaces the earlier one) and "a delete after a create
// in the same window is a net no-op" (the id ends up mapped to a
// delete; if it was never in the real store, `applyObjectBatch` removes
// nothing). The batch is applied via one `scene.applyObjectBatch` call
// per flush. Heavy upserts are further capped per flush
// (`MAX_UPSERTS_PER_FLUSH`) so a 60-part burst spreads its GPU uploads
// over several frames instead of stalling one; deletes are cheap (no
// GPU cost) and always drain in full, which is also what collapses a
// `clear_parts` burst of N `ObjectDeleted` frames into one scene sweep.
interface PendingUpsert {
  kind: 'upsert'
  obj: CADObject
  /** Mirrors the old per-message side effects: ObjectCreated always
   *  frames + echoes; GeometryUpdate only when the object is genuinely
   *  new; ObjectUpdated never. Computed once at receive time. */
  announceAsNew: boolean
  echoMessage: string | null
}
interface PendingDelete {
  kind: 'delete'
}
type PendingOp = PendingUpsert | PendingDelete

const pendingOps = new Map<string, PendingOp>()
const MAX_UPSERTS_PER_FLUSH = 4
const FLUSH_BACKSTOP_MS = 50

let flushRafHandle: number | null = null
let flushTimerHandle: ReturnType<typeof setTimeout> | null = null

/** The object this id would resolve to right now if the batch were
 *  flushed — checks still-pending churn before falling back to the
 *  committed store, so name-preservation logic sees same-window
 *  create-then-update chains correctly instead of stale committed state. */
function getEffectiveExisting(id: string): CADObject | undefined {
  const pending = pendingOps.get(id)
  if (pending) return pending.kind === 'upsert' ? pending.obj : undefined
  return useSceneStore.getState().objects.get(id)
}

function queueUpsert(obj: CADObject, announceAsNew: boolean, echoMessage: string | null) {
  pendingOps.set(obj.id, { kind: 'upsert', obj, announceAsNew, echoMessage })
  scheduleFlush()
}

function queueDelete(id: string) {
  pendingOps.set(id, { kind: 'delete' })
  scheduleFlush()
}

function scheduleFlush() {
  if (flushRafHandle !== null || flushTimerHandle !== null) return
  if (typeof requestAnimationFrame === 'function') {
    flushRafHandle = requestAnimationFrame(() => {
      flushRafHandle = null
      if (flushTimerHandle !== null) {
        clearTimeout(flushTimerHandle)
        flushTimerHandle = null
      }
      flushPendingOps()
    })
  }
  // Backstop: guarantees a flush even when rAF is throttled (a
  // backgrounded tab — plausible during a headless/unfocused eval run)
  // or unavailable. Whichever trigger fires first flushes and clears
  // the other's handle; the loser is simply a stale id, safe to ignore.
  flushTimerHandle = setTimeout(() => {
    flushTimerHandle = null
    flushRafHandle = null
    flushPendingOps()
  }, FLUSH_BACKSTOP_MS)
}

function flushPendingOps() {
  if (pendingOps.size === 0) return
  const scene = useSceneStore.getState()

  const deletes: string[] = []
  const upsertEntries: Array<[string, PendingUpsert]> = []
  for (const [id, op] of pendingOps) {
    if (op.kind === 'delete') deletes.push(id)
    else upsertEntries.push([id, op])
  }

  // Cap how many heavy upserts land in this commit; the rest stay queued
  // and ride the next flush. Deletes are cheap (no mesh/GPU cost) and
  // always apply in full — this is what makes a `clear_parts` burst of
  // N deletions collapse into one scene sweep.
  const toApplyNow = upsertEntries.slice(0, MAX_UPSERTS_PER_FLUSH)

  for (const [id] of toApplyNow) pendingOps.delete(id)
  for (const id of deletes) pendingOps.delete(id)

  if (deletes.length > 0 || toApplyNow.length > 0) {
    scene.applyObjectBatch({
      upserts: toApplyNow.map(([, op]) => op.obj),
      deletes,
    })

    for (const [, op] of toApplyNow) {
      if (!op.announceAsNew) continue
      scene.setPendingFrameObject(op.obj.id)
      if (op.echoMessage) {
        useChatStore.getState().addMessage({
          role: 'system',
          content: op.echoMessage,
          objectsAffected: [op.obj.id],
        })
      }
    }
  }

  // More upserts queued (throttled by MAX_UPSERTS_PER_FLUSH) or new
  // messages arrived while this flush ran — keep draining.
  if (pendingOps.size > 0) {
    scheduleFlush()
  }
}

// ─── Message router ─────────────────────────────────────────────────

function handleServerMessage(msg: ServerMessage) {
  const scene = useSceneStore.getState()
  const ws = useWSStore.getState()

  // Each branch's `msg.payload` is correctly narrowed by the
  // discriminated union — no runtime guards or `as` casts are
  // necessary because `ws-schemas.ts` already validated the shape.
  switch (msg.type) {
    case 'Welcome': {
      // The backend `ServerMessage` enum is `#[serde(tag = "type",
      // content = "data")]`, so the live wire shape is
      //   { "type": "Welcome", "data": { "connection_id": "..." } }
      // The zod schema also tolerates top-level / `payload`-wrapped
      // shapes for older emitters; check all three and use whichever
      // carries the UUID. The backend's timeline handler lazily seeds
      // `session_positions` for any UUID it sees, so we don't need a
      // separate `JoinSession` round-trip — without this set, the
      // Ctrl+Z handler in `shortcuts.ts` bails at the null guard
      // and the keypress silently does nothing.
      const connectionId =
        msg.data?.connection_id ??
        msg.connection_id ??
        msg.payload?.connection_id
      if (connectionId) {
        ws.setSessionId(connectionId)
        // First time we know the session is live, hydrate sketches —
        // mirrors the SessionUpdate path so the model tree fills in
        // even on the connection-id-only protocol path.
        hydrateSketchesOnce()
      }
      break
    }

    case 'GeometryUpdate': {
      const proto = msg.payload.object
      const obj = convertCADObject(proto)
      const existing = getEffectiveExisting(obj.id)
      const isNew = existing === undefined
      // Same rationale as ObjectCreated — backend sometimes ships new
      // geometry as a Tessellated GeometryUpdate (e.g., REST creates
      // followed by tessellation). Frame it, but only when it's
      // genuinely new (mirrors the old addObject-vs-updateObject split).
      queueUpsert(obj, isNew, isNew ? dimensionEchoMessage(proto) : null)
      break
    }

    case 'ObjectCreated': {
      const proto = msg.payload
      const obj = convertCADObject(proto)
      // Diagnostic: surfaces whether the WS path is wired and whether
      // the per-triangle FaceId map reached the bridge. If `faceIds` is
      // 0 here, face-picking will silently fall back to raw triangle
      // indices and the kernel pick query will reject them.

      console.log(
        '[WS] ObjectCreated',
        obj.id.slice(0, 8),
        obj.objectType,
        `tris=${obj.mesh.indices.length / 3}`,
        `faceIds=${obj.mesh.faceIds?.length ?? 0}`,
      )
      // In-place upsert (e.g., face-extrude that mutates the host solid
      // and rebroadcasts on the same UUID): preserve the user-visible
      // name. Backend handlers that mutate-in-place currently emit a
      // generated name like "FaceExtrude {solid_id}", which would
      // otherwise clobber whatever the user named the host ("Box 0",
      // "Bracket A", …). Object identity is the UUID, not the name —
      // when we already know the object, only the geometry is new.
      const existing = getEffectiveExisting(obj.id)
      if (existing) {
        obj.name = existing.name
      }
      // Auto-frame the viewport on the new object so the user always
      // sees what they just made — backend may place it off-screen
      // (e.g., booleans land at world origin, extrudes shift along the
      // face normal). CameraController consumes & clears this flag.
      queueUpsert(obj, true, dimensionEchoMessage(proto))
      break
    }

    case 'ObjectUpdated': {
      // Modifying ops (shell, mirror, fillet, chamfer, face-extrude)
      // ship `ObjectUpdated` with the new tessellation but a backend-
      // generated `name` field (the kernel does not track user-visible
      // names). Mirror the `ObjectCreated` upsert convention here:
      // identity is the UUID, the user owns the name. Strip the name
      // and objectType from the patch so the kernel cannot rename a
      // box to "Fillet 7" just because an edge was rounded.
      const obj = convertCADObject(msg.payload)
      const existing = getEffectiveExisting(obj.id)
      // Matches the old `updateObject`'s no-op-if-missing guard: an
      // ObjectUpdated for an id the client has never heard of has
      // nothing to patch.
      if (!existing) break
      obj.name = existing.name
      obj.objectType = existing.objectType
      queueUpsert(obj, false, null)
      break
    }

    case 'ObjectDeleted': {
      queueDelete(msg.payload.id)
      break
    }

    case 'ObjectColor': {
      // Backend `set_part_color` recoloured a part; apply it to the live
      // mesh. `color` is [r,g,b] in 0..=255 — scene-store swaps in a new
      // material so CADMesh's color-keyed memo recomputes.
      scene.setObjectColor(msg.payload.object_id, msg.payload.color)
      break
    }

    case 'SceneSync': {
      // Full scene sync — replace all objects. This is an authoritative
      // snapshot, so any still-pending churn from the batching queue
      // above is superseded; drop it rather than let a stale queued
      // delete/upsert race the sync on the next flush.
      pendingOps.clear()
      scene.clearScene()
      scene.applyObjectBatch({
        upserts: msg.payload.objects.map(convertCADObject),
        deletes: [],
      })
      break
    }

    case 'SessionUpdate': {
      ws.setSessionId(msg.payload.session_id)
      // First time we know the session is live, hydrate the sketch
      // collection from REST. The model tree needs every existing
      // sketch to render — Sketch{Created,Updated,Deleted} frames
      // only fire on changes after this point, so without the seed
      // a page reload would leave the tree blind to live sessions.
      hydrateSketchesOnce()
      break
    }

    case 'Pong':
      // Heartbeat response — RTT is timed at the client in
      // `ws-client.ts::startHeartbeat`, payload is not consumed here.
      break

    case 'SubElementResult': {
      // Backend resolved a sub-element pick to authoritative topology
      // indices. Replace the optimistic local selection with the
      // backend's answer.
      scene.clearSubElementSelections()
      for (const el of msg.payload.elements) {
        scene.addSubElementSelection({
          objectId: msg.payload.object_id,
          type: el.type,
          index: el.index,
          polyline: el.polyline,
        })
      }
      break
    }

    case 'Error': {
      console.error('[WS] Server error:', msg.payload.message)
      break
    }

    // Sketch lifecycle frames. Backend is the source of truth for
    // sketch sessions; the local store mirrors the snapshots.
    case 'SketchCreated':
    case 'SketchUpdated': {
      scene.applyServerSketchSnapshot(msg.payload)
      break
    }
    case 'SketchDeleted': {
      scene.clearServerSketchId(msg.payload.id)
      break
    }
    case 'SketchRegionsUpdated': {
      // Per-sketch region decomposition. The local store only mirrors
      // regions for the user's currently-active session; peer-edited
      // sketches don't need a regions cache because the overlay only
      // paints the active session anyway.
      scene.applySketchRegions(msg.payload)
      break
    }
    case 'SketchExtruded': {
      // The actual mesh arrives via `ObjectCreated` (broadcast just
      // before this frame). Future timeline / chat hooks can attribute
      // the new solid back to its sketch via `payload.sketch_id`.
      //
      // If the *local* user's active sketch is the one that just
      // extruded, tear the overlay down: the profile has become a solid,
      // so leaving the 2D loop on screen (and capturing pointer events
      // behind the new body) is exactly the "sketch lingers after the
      // 3D is consumed" confusion. The backend already owns the session
      // lifecycle here, so don't re-delete it (`deleteBackend: false`).
      // This frame precedes `SketchDeleted` in the broadcast order, so
      // `serverId` still matches at this point.
      if (
        scene.sketch.active &&
        scene.sketch.serverId === msg.payload.sketch_id
      ) {
        scene.exitSketch({ deleteBackend: false })
      }
      break
    }
  }
}

// ─── Sketch hydration ───────────────────────────────────────────────

// Latch so we only call `sketchApi.list()` once per WebSocket lifecycle.
// `SessionUpdate` can fire multiple times (initial Welcome + later
// reconciliations), but the bootstrapping fetch is idempotent and
// expensive enough to be worth deduplicating.
let sketchesHydrated = false

function hydrateSketchesOnce() {
  if (sketchesHydrated) return
  sketchesHydrated = true
  sketchApi
    .list()
    .then((sessions) => {
      useSceneStore.getState().setServerSketches(sessions)
    })
    .catch((err) => {
      // Sketch listing is best-effort; if the endpoint is unreachable
      // (older backend, network blip), the model tree falls back to
      // showing only sketches that arrive via WS frames.
      console.warn('[ws-bridge] sketch hydration failed:', err)
      sketchesHydrated = false
    })
}

// ─── Reconnect resync ───────────────────────────────────────────────

/// Full scene refetch after a WS RE-connect. The server may be a brand
/// new process (dev rebuild/restart), in which case every object the
/// client holds is stale — wrong UUIDs, ghost geometry, deletes that
/// silently no-op. Replace the scene wholesale from
/// `GET /api/scene/snapshot` (same payload shape as `ObjectCreated`,
/// so `convertCADObject` is reused verbatim). Sketch hydration is also
/// reset so the model tree refills against the new server.
const API_BASE = `${import.meta.env.VITE_API_URL || ''}/api`

/// Force a full scene refetch from the server.
///
/// Identical to the reconnect resync, exposed for mutations whose result the
/// client must reflect immediately without waiting on (or in case of a missed)
/// an `ObjectCreated` WS frame — e.g. a drag-and-drop STEP import that splices
/// several solids into the live model server-side. Cheap (one
/// `GET /api/scene/snapshot`) and idempotent (clear + rebuild).
export async function refreshSceneFromServer(): Promise<void> {
  return resyncSceneFromServer()
}

async function resyncSceneFromServer(): Promise<void> {
  try {
    const res = await fetch(`${API_BASE}/scene/snapshot`)
    if (!res.ok) return
    const snap = (await res.json()) as { objects?: ProtocolCADObject[] }
    // Convert FIRST, then clear+swap. If a malformed object throws mid-convert,
    // it throws BEFORE we touch the live scene — so a bad snapshot can never
    // blank the viewport. (The old clear-then-build order, paired with an
    // object missing its material, was the "appears then disappears" bug.)
    const converted = (snap.objects ?? []).map(convertCADObject)
    const scene = useSceneStore.getState()
    // Authoritative snapshot — drop any still-pending churn so a queued
    // delete/upsert from before the reconnect can't race this rebuild.
    pendingOps.clear()
    scene.clearScene()
    scene.applyObjectBatch({ upserts: converted, deletes: [] })
    sketchesHydrated = false
    hydrateSketchesOnce()
    console.info(
      `[ws-bridge] scene resynced after reconnect: ${converted.length} object(s)`,
    )
  } catch (err) {
    console.warn('[ws-bridge] scene resync failed (scene preserved):', err)
  }
}

// ─── Lifecycle ──────────────────────────────────────────────────────

let initialized = false
let unsubscribe: (() => void) | null = null

export function initWebSocket() {
  if (initialized) return

  initialized = true
  unsubscribe = wsClient.onMessage(handleServerMessage)
  setResyncHook(() => void resyncSceneFromServer())
  wsClient.connect()

  // The backend emits `Welcome` on connect carrying the per-connection
  // UUID; `handleServerMessage` lifts that into `useWSStore.sessionId`
  // so REST endpoints (timeline undo/redo, etc.) have a key to thread
  // through. No client-initiated handshake is required — sessions are
  // seeded lazily server-side keyed on whichever UUID the client uses.
  // Keep the store subscription as a no-op so the cleanup tuple
  // signature stays stable for future use.
  const unsub = useWSStore.subscribe(() => {})

  // Return cleanup for potential future use
  return () => {
    unsub()
    unsubscribe?.()
    wsClient.disconnect()
    initialized = false
  }
}

export function teardownWebSocket() {
  unsubscribe?.()
  wsClient.disconnect()
  initialized = false
  sketchesHydrated = false
  // Cancel any in-flight batch flush and drop queued churn — nothing
  // left to apply it to once the bridge is torn down, and a stray timer
  // firing after teardown would resurrect stale objects on the next init.
  pendingOps.clear()
  if (flushRafHandle !== null && typeof cancelAnimationFrame === 'function') {
    cancelAnimationFrame(flushRafHandle)
  }
  flushRafHandle = null
  if (flushTimerHandle !== null) {
    clearTimeout(flushTimerHandle)
    flushTimerHandle = null
  }
}
