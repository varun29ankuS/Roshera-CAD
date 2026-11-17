import { useEffect } from 'react'
import * as THREE from 'three'
import { useSceneStore } from '@/stores/scene-store'
import { useCursorStore } from '@/stores/cursor-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useHistoryView } from '@/stores/history-view-store'
import { useSketchBench } from '@/stores/sketch-bench-store'
import { useUserContextStore } from '@/stores/user-context-store'
import {
  ContextPublisher,
  buildReport,
  type CameraView,
  type PublishAck,
  type UserContextReport,
} from '@/lib/user-context'

const API_HOST: string = import.meta.env.VITE_API_URL || ''

/** How often the context is looked at for a heartbeat. */
const BEAT_MS = 1000

async function post(report: UserContextReport): Promise<PublishAck | null> {
  try {
    const r = await fetch(`${API_HOST}/api/agent/user-context`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(report),
    })
    return r.ok ? ((await r.json()) as PublishAck) : null
  } catch {
    // Unreachable server: the publisher tries again on its next beat.
    return null
  }
}

const direction = new THREE.Vector3()

function cameraNow(): CameraView | null {
  const scene = useSceneStore.getState()
  const camera = scene.cameraRef
  if (!camera) return null
  camera.getWorldDirection(direction)
  return {
    position: camera.position.toArray() as [number, number, number],
    direction: direction.toArray() as [number, number, number],
    up: camera.up.toArray() as [number, number, number],
    projection: scene.cameraProjection,
    ...(scene.cameraPreset ? { preset: scene.cameraPreset } : {}),
  }
}

/** The report for what the stores hold now. */
function reportNow(): UserContextReport {
  const scene = useSceneStore.getState()
  const history = useHistoryView.getState()
  return buildReport(
    {
      sharing: useUserContextStore.getState().sharing,
      workspace: useDocModeStore.getState().mode ?? 'part',
      tool: useSketchBench.getState().sketchId !== null ? 'sketch' : scene.activeTool,
      selectionMode: scene.selectionMode,
      selectedIds: scene.selectedIds,
      subElements: scene.subElementSelections,
      hoveredId: scene.hoveredId,
      cursor: useCursorStore.getState().hit,
      faceIdsOf: (id) => scene.objects.get(id)?.mesh.faceIds,
      feature: history.selectedChapter,
      historyStep: history.step,
      camera: cameraNow(),
    },
    Date.now(),
  )
}

/**
 * Report the person's context to the server while the workspace is open:
 * on every change once it settles, and on a heartbeat while the tab is in
 * front (a tab in the background stops, and the server then calls the
 * context stale). Mounted once, by `App`.
 */
export function useUserContextPublisher(active: boolean): void {
  useEffect(() => {
    if (!active) return
    const store = useUserContextStore.getState()
    const publisher = new ContextPublisher({
      send: post,
      now: () => Date.now(),
      schedule: (fn, ms) => window.setTimeout(fn, ms),
      cancel: (handle) => window.clearTimeout(handle as number),
      onAck: (ack, report) => {
        store.setAcknowledged(report)
        if (ack.agent_look) {
          store.agentLooked({
            agent: ack.agent_look.agent,
            atMs: Date.now() - ack.agent_look.age_ms,
            seq: ack.agent_look.seq,
          })
        }
      },
    })
    const offer = () => publisher.offer(reportNow())
    const unsubscribe = [
      useSceneStore.subscribe((s, prev) => {
        if (
          s.selectedIds !== prev.selectedIds ||
          s.subElementSelections !== prev.subElementSelections ||
          s.hoveredId !== prev.hoveredId ||
          s.selectionMode !== prev.selectionMode ||
          s.activeTool !== prev.activeTool
        )
          offer()
      }),
      useCursorStore.subscribe(offer),
      useDocModeStore.subscribe(offer),
      useHistoryView.subscribe((s, prev) => {
        if (s.selectedChapter !== prev.selectedChapter || s.step !== prev.step) offer()
      }),
      useSketchBench.subscribe((s, prev) => {
        if ((s.sketchId === null) !== (prev.sketchId === null)) offer()
      }),
      useUserContextStore.subscribe((s, prev) => {
        if (s.sharing !== prev.sharing) offer()
      }),
    ]
    offer()
    const beat = window.setInterval(() => {
      if (document.visibilityState === 'visible') publisher.heartbeat(reportNow())
    }, BEAT_MS)
    return () => {
      window.clearInterval(beat)
      for (const u of unsubscribe) u()
      publisher.stop()
    }
  }, [active])
}
