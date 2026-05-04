/**
 * WebSocket → Scene Store bridge.
 * Connects on mount, routes incoming ServerMessages to the appropriate stores,
 * and converts backend CADObject format to the scene store format.
 */
import { wsClient } from './ws-client'
import { useWSStore } from '@/stores/ws-store'
import { useSceneStore, type CADObject, type CADMesh, type CADMaterial, type AnalyticalGeometry } from '@/stores/scene-store'
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

function convertMaterial(mat: MaterialProperties): CADMaterial {
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
    params: ag.parameters,
  }
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
      const obj = convertCADObject(msg.payload.object)
      const existing = scene.objects.get(obj.id)
      if (existing) {
        scene.updateObject(obj.id, obj)
      } else {
        scene.addObject(obj)
        // Same rationale as ObjectCreated — backend sometimes ships new
        // geometry as a Tessellated GeometryUpdate (e.g., REST creates
        // followed by tessellation). Frame it.
        scene.setPendingFrameObject(obj.id)
      }
      break
    }

    case 'ObjectCreated': {
      const obj = convertCADObject(msg.payload)
      // Diagnostic: surfaces whether the WS path is wired and whether
      // the per-triangle FaceId map reached the bridge. If `faceIds` is
      // 0 here, face-picking will silently fall back to raw triangle
      // indices and the kernel pick query will reject them.
      // eslint-disable-next-line no-console
      console.log(
        '[WS] ObjectCreated',
        obj.id.slice(0, 8),
        obj.objectType,
        `tris=${obj.mesh.indices.length / 3}`,
        `faceIds=${obj.mesh.faceIds?.length ?? 0}`,
      )
      scene.addObject(obj)
      // Auto-frame the viewport on the new object so the user always
      // sees what they just made — backend may place it off-screen
      // (e.g., booleans land at world origin, extrudes shift along the
      // face normal). CameraController consumes & clears this flag.
      scene.setPendingFrameObject(obj.id)
      break
    }

    case 'ObjectUpdated': {
      const obj = convertCADObject(msg.payload)
      scene.updateObject(obj.id, obj)
      break
    }

    case 'ObjectDeleted': {
      scene.removeObject(msg.payload.id)
      break
    }

    case 'SceneSync': {
      // Full scene sync — replace all objects.
      scene.clearScene()
      for (const proto of msg.payload.objects) {
        scene.addObject(convertCADObject(proto))
      }
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
    case 'SketchExtruded': {
      // The actual mesh arrives via `ObjectCreated` (broadcast just
      // before this frame). Nothing to wire into the scene store yet —
      // future timeline / chat hooks can attribute the new solid back
      // to its sketch via `payload.sketch_id`.
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

// ─── Lifecycle ──────────────────────────────────────────────────────

let initialized = false
let unsubscribe: (() => void) | null = null

export function initWebSocket() {
  if (initialized) return

  initialized = true
  unsubscribe = wsClient.onMessage(handleServerMessage)
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
}
