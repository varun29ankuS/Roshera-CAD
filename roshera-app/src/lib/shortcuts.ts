import { useEffect } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { wsClient } from './ws-client'
import * as THREE from 'three'

const API_BASE = import.meta.env.VITE_API_URL || ''

async function timelineAction(action: 'undo' | 'redo') {
  try {
    await fetch(`${API_BASE}/api/timeline/${action}`, { method: 'POST' })
  } catch {
    // backend not running
  }
}

function focusSelected() {
  const state = useSceneStore.getState()
  const { cameraRef, sceneRef } = state
  if (!cameraRef || !sceneRef || state.selectedIds.size === 0) return

  const box = new THREE.Box3()
  for (const id of state.selectedIds) {
    sceneRef.traverse((child) => {
      if (child.userData?.cadObjectId === id && child instanceof THREE.Mesh) {
        const childBox = new THREE.Box3().setFromObject(child)
        box.union(childBox)
      }
    })
  }

  if (box.isEmpty()) return

  const center = new THREE.Vector3()
  box.getCenter(center)
  const size = new THREE.Vector3()
  box.getSize(size)
  const maxDim = Math.max(size.x, size.y, size.z, 1)
  const camera = cameraRef as THREE.PerspectiveCamera

  const fov = camera.fov * (Math.PI / 180)
  const dist = maxDim / (2 * Math.tan(fov / 2)) * 1.5
  const direction = new THREE.Vector3()
  camera.getWorldDirection(direction)

  camera.position.copy(center).sub(direction.multiplyScalar(dist))
  camera.lookAt(center)
  camera.updateProjectionMatrix()
}

export function useKeyboardShortcuts() {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement
      if (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable
      ) {
        return
      }

      const ctrl = e.ctrlKey || e.metaKey
      const state = useSceneStore.getState()

      switch (e.key.toLowerCase()) {
        // Undo / Redo
        case 'z':
          if (ctrl) {
            e.preventDefault()
            if (e.shiftKey) {
              timelineAction('redo')
            } else {
              timelineAction('undo')
            }
          }
          break

        // Select all
        case 'a':
          if (ctrl) {
            e.preventDefault()
            for (const id of state.objectOrder) {
              state.selectObject(id, true)
            }
          }
          break

        // Transform tools
        case 'v':
          if (!ctrl) {
            e.preventDefault()
            state.setActiveTool('select')
          }
          break
        case 'g':
          if (!ctrl) {
            e.preventDefault()
            state.setActiveTool('translate')
          }
          break
        case 'r':
          if (!ctrl) {
            e.preventDefault()
            state.setActiveTool('rotate')
          }
          break
        case 's':
          if (!ctrl) {
            e.preventDefault()
            state.setActiveTool('scale')
          }
          break

        // Selection modes
        case '1':
          if (!ctrl && !e.altKey) {
            e.preventDefault()
            state.setSelectionMode('object')
          }
          break
        case '2':
          if (!ctrl && !e.altKey) {
            e.preventDefault()
            state.setSelectionMode('face')
          }
          break
        case '3':
          if (!ctrl && !e.altKey) {
            e.preventDefault()
            state.setSelectionMode('edge')
          }
          break
        case '4':
          if (!ctrl && !e.altKey) {
            e.preventDefault()
            state.setSelectionMode('vertex')
          }
          break

        // Camera presets (numpad)
        case 'numpad1':
          e.preventDefault()
          state.setCameraPreset(e.ctrlKey ? 'back' : 'front')
          break
        case 'numpad3':
          e.preventDefault()
          state.setCameraPreset(e.ctrlKey ? 'left' : 'right')
          break
        case 'numpad7':
          e.preventDefault()
          state.setCameraPreset(e.ctrlKey ? 'bottom' : 'top')
          break
        case 'numpad0':
          e.preventDefault()
          state.setCameraPreset('isometric')
          break

        // Delete selected
        case 'delete':
        case 'backspace':
          if (!ctrl) {
            e.preventDefault()
            for (const id of state.selectedIds) {
              wsClient.send({ type: 'Command', payload: { cmd: 'DeleteObject', object_id: id } })
              state.removeObject(id)
            }
          }
          break

        // Escape — deselect and reset tool
        case 'escape':
          e.preventDefault()
          state.deselectAll()
          state.setActiveTool('select')
          break

        // Toggle transform space
        case 'x':
          if (!ctrl) {
            e.preventDefault()
            state.setTransformSpace(
              state.transformSpace === 'world' ? 'local' : 'world',
            )
          }
          break

        // Focus selected (zoom to fit)
        case 'f':
          if (!ctrl) {
            e.preventDefault()
            focusSelected()
          }
          break
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [])
}
