/**
 * ViewportBridge — gives Claude (or any dev tool) eyes into the live
 * Three.js viewport.
 *
 * Mounts a WebSocket to the api-server's `/ws/viewport-bridge` endpoint
 * (only present when the server is started with `ROSHERA_DEV_BRIDGE=1`).
 * Receives commands from the server and replies with screenshots, acks,
 * or error messages.
 *
 * Commands:
 *  - `snapshot`     — returns base64 PNG of the current canvas
 *  - `set_camera`   — moves camera to `position` looking at `target`
 *  - `load_stl`     — decodes base64 STL, adds as a CADObject
 *  - `set_shading`  — swaps material mode (lit | normals | wireframe)
 *  - `clear_scene`  — drops every CADObject
 *
 * Enable from the frontend by setting `VITE_ENABLE_VIEWPORT_BRIDGE=1`
 * (the component renders nothing — purely a side-effecting hook host).
 */

import { useEffect, useRef, type MutableRefObject } from 'react'
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js'
import { useSceneStore, type CADObject } from '@/stores/scene-store'

type SnapshotCmd = {
  cmd: 'snapshot'
  request_id: string
  width?: number | null
  height?: number | null
}
type SetCameraCmd = {
  cmd: 'set_camera'
  request_id: string
  position: [number, number, number]
  target: [number, number, number]
  up: [number, number, number]
}
type LoadStlCmd = {
  cmd: 'load_stl'
  request_id: string
  path: string // base64-encoded STL bytes
  name: string
  replace_scene: boolean
}
type SetShadingCmd = {
  cmd: 'set_shading'
  request_id: string
  mode: string
}
type ClearSceneCmd = {
  cmd: 'clear_scene'
  request_id: string
}

type BridgeCommand =
  | SnapshotCmd
  | SetCameraCmd
  | LoadStlCmd
  | SetShadingCmd
  | ClearSceneCmd

/** Resolve the bridge WebSocket URL. */
function bridgeUrl(): string {
  const explicit = import.meta.env.VITE_VIEWPORT_BRIDGE_URL as string | undefined
  if (explicit && explicit.length > 0) return explicit

  const wsBase = import.meta.env.VITE_WS_URL as string | undefined
  if (wsBase) {
    return wsBase.replace(/\/ws\/?$/, '') + '/ws/viewport-bridge'
  }
  // Default: same host as the page, ws scheme matched to http(s).
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  // The api-server defaults to port 8081 in development. Falling back to
  // window.location.host would point at Vite (5173) and 404. Prefer
  // explicit override via VITE_VIEWPORT_BRIDGE_URL when running on a
  // non-default port.
  return `${proto}//${window.location.hostname}:8081/ws/viewport-bridge`
}

/** Convert base64 (with or without data: prefix) to a Uint8Array. */
function decodeBase64(input: string): Uint8Array {
  const stripped = input.replace(/^data:[^;]+;base64,/, '').trim()
  const binary = atob(stripped)
  const out = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) {
    out[i] = binary.charCodeAt(i)
  }
  return out
}

const stlLoader = new STLLoader()

/**
 * React component that, when mounted, opens a debug WebSocket to the
 * api-server and translates incoming commands into Three.js / scene-store
 * operations. Renders nothing.
 */
export function ViewportBridge() {
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectTimerRef = useRef<number | null>(null)
  const closingRef = useRef(false)
  const autoSnapTimerRef = useRef<number | null>(null)

  useEffect(() => {
    const enabled = import.meta.env.VITE_ENABLE_VIEWPORT_BRIDGE === '1'
    if (!enabled) return

    const url = bridgeUrl()

    const connect = () => {
      if (closingRef.current) return
      // eslint-disable-next-line no-console
      console.info('[viewport-bridge] connecting to', url)
      const ws = new WebSocket(url)
      wsRef.current = ws

      ws.onopen = () => {
        // eslint-disable-next-line no-console
        console.info('[viewport-bridge] connected')
        // Push an immediate snapshot so `latest.png` is fresh on connect.
        scheduleAutoSnapshot(wsRef, autoSnapTimerRef, 100)
      }

      ws.onmessage = (event) => {
        let cmd: BridgeCommand
        try {
          cmd = JSON.parse(event.data) as BridgeCommand
        } catch (err) {
          // eslint-disable-next-line no-console
          console.warn('[viewport-bridge] bad json', err, event.data)
          return
        }
        handleCommand(ws, cmd).catch((err: unknown) => {
          const message =
            err instanceof Error ? err.message : String(err)
          // eslint-disable-next-line no-console
          console.error('[viewport-bridge] handler failed', err)
          sendError(ws, cmd.request_id, message)
        })
      }

      ws.onclose = () => {
        wsRef.current = null
        if (closingRef.current) return
        // Backoff reconnect — 2s is plenty for dev workflow.
        reconnectTimerRef.current = window.setTimeout(connect, 2000)
      }

      ws.onerror = (event) => {
        // eslint-disable-next-line no-console
        console.warn('[viewport-bridge] ws error', event)
        // onclose will follow.
      }
    }

    connect()

    // Auto-snapshot on any scene-store mutation. We rely on the
    // 350 ms debounce in `scheduleAutoSnapshot` to coalesce bursts
    // (object add → camera refit → material settle) into one capture.
    const unsubscribe = useSceneStore.subscribe(() => {
      scheduleAutoSnapshot(wsRef, autoSnapTimerRef, 350)
    })

    return () => {
      closingRef.current = true
      unsubscribe()
      if (reconnectTimerRef.current) {
        clearTimeout(reconnectTimerRef.current)
        reconnectTimerRef.current = null
      }
      if (autoSnapTimerRef.current) {
        clearTimeout(autoSnapTimerRef.current)
        autoSnapTimerRef.current = null
      }
      const ws = wsRef.current
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.close()
      }
      wsRef.current = null
    }
  }, [])

  return null
}

/**
 * Schedule a debounced auto-snapshot push. Re-arming the timer
 * coalesces bursts of scene mutations into a single capture.
 */
function scheduleAutoSnapshot(
  wsRef: MutableRefObject<WebSocket | null>,
  timerRef: MutableRefObject<number | null>,
  delayMs: number,
): void {
  if (timerRef.current) {
    clearTimeout(timerRef.current)
  }
  timerRef.current = window.setTimeout(() => {
    timerRef.current = null
    const ws = wsRef.current
    if (!ws || ws.readyState !== WebSocket.OPEN) return

    const gl = useSceneStore.getState().glRef
    const scene = useSceneStore.getState().sceneRef
    const camera = useSceneStore.getState().cameraRef
    if (!gl || !scene || !camera) return

    try {
      gl.render(scene, camera)
      const canvas = gl.domElement
      const dataUrl = canvas.toDataURL('image/png')
      send(ws, {
        kind: 'auto_snapshot',
        data_base64: dataUrl,
        width: canvas.width,
        height: canvas.height,
      })
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('[viewport-bridge] auto-snapshot failed', err)
    }
  }, delayMs)
}

// ─── command dispatch ───────────────────────────────────────────────────

async function handleCommand(ws: WebSocket, cmd: BridgeCommand): Promise<void> {
  switch (cmd.cmd) {
    case 'snapshot':
      await handleSnapshot(ws, cmd)
      return
    case 'set_camera':
      await handleSetCamera(ws, cmd)
      return
    case 'load_stl':
      await handleLoadStl(ws, cmd)
      return
    case 'set_shading':
      await handleSetShading(ws, cmd)
      return
    case 'clear_scene':
      await handleClearScene(ws, cmd)
      return
  }
}

async function handleSnapshot(ws: WebSocket, cmd: SnapshotCmd): Promise<void> {
  const gl = useSceneStore.getState().glRef
  if (!gl) throw new Error('renderer not ready')

  // Force a render so the canvas reflects any pending state changes
  // (e.g. material swap that just happened on the previous command).
  const scene = useSceneStore.getState().sceneRef
  const camera = useSceneStore.getState().cameraRef
  if (scene && camera) {
    gl.render(scene, camera)
  }

  const canvas = gl.domElement
  const dataUrl = canvas.toDataURL('image/png')
  send(ws, {
    kind: 'snapshot_result',
    request_id: cmd.request_id,
    data_base64: dataUrl,
    width: canvas.width,
    height: canvas.height,
  })
}

async function handleSetCamera(
  ws: WebSocket,
  cmd: SetCameraCmd,
): Promise<void> {
  // Reuse the existing CameraController animation pipeline by writing a
  // synthetic preset into the store. The controller picks it up on its
  // next render tick, animates over ~600ms, then clears it.
  useSceneStore.setState({
    cameraPreset: 'bridge',
    pendingCameraPreset: {
      name: 'Bridge',
      position: cmd.position,
      target: cmd.target,
      up: cmd.up,
    },
  })

  // Wait for the animation to settle before acking. The controller uses a
  // 0.6s ease-out cubic; 700ms is a safe ceiling.
  await new Promise<void>((resolve) => {
    window.setTimeout(resolve, 700)
  })

  sendAck(ws, cmd.request_id)
}

async function handleLoadStl(ws: WebSocket, cmd: LoadStlCmd): Promise<void> {
  const bytes = decodeBase64(cmd.path)
  const buffer = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer
  const geometry = stlLoader.parse(buffer)

  const positionAttr = geometry.getAttribute('position')
  if (!positionAttr) throw new Error('STL produced no position attribute')

  const vertices = new Float32Array(positionAttr.array.length)
  vertices.set(positionAttr.array as ArrayLike<number>)

  let normals: Float32Array
  const normalAttr = geometry.getAttribute('normal')
  if (normalAttr) {
    normals = new Float32Array(normalAttr.array.length)
    normals.set(normalAttr.array as ArrayLike<number>)
  } else {
    normals = new Float32Array(0)
  }

  const indices = new Uint32Array(0)

  const obj: CADObject = {
    id: `bridge-${cmd.request_id}`,
    name: cmd.name,
    objectType: 'bridge-stl',
    mesh: { vertices, indices, normals },
    material: {
      color: '#9ca8c4',
      metalness: 0.1,
      roughness: 0.45,
      opacity: 1,
    },
    position: [0, 0, 0],
    rotation: [0, 0, 0],
    scale: [1, 1, 1],
    visible: true,
    locked: false,
  }

  if (cmd.replace_scene) {
    useSceneStore.getState().clearScene()
  }
  useSceneStore.getState().addObject(obj)

  sendAck(ws, cmd.request_id)
}

async function handleSetShading(
  ws: WebSocket,
  cmd: SetShadingCmd,
): Promise<void> {
  // Shading modes are surfaced as edge-settings + per-object material tweaks.
  // For v1, we toggle wireframe on the existing edge-rendering layer.
  const mode = cmd.mode.toLowerCase()
  const setEdge = useSceneStore.getState().setEdgeSettings

  switch (mode) {
    case 'lit':
      setEdge({ visible: true })
      break
    case 'wireframe':
      setEdge({ visible: true, threshold: 1, lineWidth: 1 })
      break
    case 'normals':
    case 'edges':
      // Future: requires extending CADMesh material logic. Acked so the
      // caller can probe whether the bridge is alive without erroring.
      setEdge({ visible: true })
      break
    default:
      throw new Error(`unknown shading mode: ${cmd.mode}`)
  }

  sendAck(ws, cmd.request_id)
}

async function handleClearScene(
  ws: WebSocket,
  cmd: ClearSceneCmd,
): Promise<void> {
  useSceneStore.getState().clearScene()
  sendAck(ws, cmd.request_id)
}

// ─── reply helpers ──────────────────────────────────────────────────────

function send(ws: WebSocket, payload: unknown): void {
  if (ws.readyState !== WebSocket.OPEN) return
  ws.send(JSON.stringify(payload))
}

function sendAck(ws: WebSocket, requestId: string): void {
  send(ws, { kind: 'ack', request_id: requestId })
}

function sendError(ws: WebSocket, requestId: string, message: string): void {
  send(ws, { kind: 'error', request_id: requestId, message })
}
