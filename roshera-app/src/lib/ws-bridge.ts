/**
 * WebSocket → Scene Store bridge.
 * Connects on mount, routes incoming ServerMessages to the appropriate stores,
 * and converts backend CADObject format to the scene store format.
 */
import { wsClient } from './ws-client'
import { useWSStore } from '@/stores/ws-store'
import { useSceneStore, type CADObject, type CADMesh, type CADMaterial, type AnalyticalGeometry } from '@/stores/scene-store'
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
    case 'GeometryUpdate': {
      const obj = convertCADObject(msg.payload.object)
      const existing = scene.objects.get(obj.id)
      if (existing) {
        scene.updateObject(obj.id, obj)
      } else {
        scene.addObject(obj)
      }
      break
    }

    case 'ObjectCreated': {
      scene.addObject(convertCADObject(msg.payload))
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
  }
}

// ─── Lifecycle ──────────────────────────────────────────────────────

let initialized = false
let unsubscribe: (() => void) | null = null

export function initWebSocket() {
  if (initialized) return

  initialized = true
  unsubscribe = wsClient.onMessage(handleServerMessage)
  wsClient.connect()

  // Backend has no `JoinSession` ClientMessage variant — sessions are
  // established automatically and the server emits `Welcome` on connect.
  // The session id is delivered later via SessionUpdate / Welcome and
  // routed into useWSStore from `handleServerMessage`. Nothing to send
  // on connect; keep the subscription as a no-op so the cleanup tuple
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
}
