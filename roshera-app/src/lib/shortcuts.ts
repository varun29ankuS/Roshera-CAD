import { useEffect } from 'react'
import { useSketchBench } from '@/stores/sketch-bench-store'
import { invalidate } from '@react-three/fiber'
import { useSceneStore } from '@/stores/scene-store'
import { useCommandPaletteStore } from '@/stores/command-palette-store'
import { timelineAction } from '@/lib/history'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { sceneShortcutApplies } from '@/lib/workspaces'
import { viewPresetForKey } from '@/lib/view-shortcuts'
import { deleteParts } from '@/lib/delete-parts'
import { useBlackboardStore } from '@/stores/blackboard-store'
import * as THREE from 'three'

const API_BASE = import.meta.env.VITE_API_URL || ''


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
  // The canvas renders on demand; a camera moved outside React must ask
  // for the frame that shows it.
  invalidate()
}

/**
 * Delete the selected parts (the Delete key and Edit > Delete). Routed
 * through the canonical REST endpoint -- the same one ModelTree's context
 * menu uses -- so the kernel, timeline and viewport stay in sync: a part
 * leaves the screen only on the kernel's `ObjectDeleted` broadcast
 * (ws-bridge.ts), never before. A delete that does not happen is reported on
 * the Blackboard by the part's name (lib/delete-parts.ts).
 */
export function deleteSelectedParts(): Promise<number> {
  const state = useSceneStore.getState()
  const parts = Array.from(state.selectedIds).map((id) => ({
    id,
    name: state.objects.get(id)?.name ?? 'the selected part',
  }))
  return deleteParts(
    parts,
    (id) => fetch(`${API_BASE}/api/geometry/${id}`, { method: 'DELETE' }),
    (line) => useBlackboardStore.getState().addLine(line, 'system'),
  )
}

export function useKeyboardShortcuts() {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement
      const ctrl = e.ctrlKey || e.metaKey

      // Ctrl/Cmd-K opens the command palette. This is the one shortcut
      // that must work *even when the user is typing in an input*, so
      // we handle it before the input-typing bail-out below — same
      // behaviour as VS Code, Linear, Figma, Notion, etc.
      if (ctrl && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        useCommandPaletteStore.getState().toggle()
        return
      }

      if (
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable
      ) {
        return
      }

      // These are the part scene's keys. From a workspace where the scene
      // is parked out of sight they must not act on it (lib/workspaces).
      if (!sceneShortcutApplies(useDocModeStore.getState().mode ?? 'part', e.key, ctrl)) return

      // While the sketch workbench is open it owns the keyboard: its own
      // tools, Delete, Esc, Enter and Ctrl+Z act on the sketch, never on
      // the part or the timeline.
      if (useSketchBench.getState().sketchId !== null) return

      const state = useSceneStore.getState()

      // Inside sketch mode the SketchPanel owns Enter/Esc/Backspace
      // and Ctrl+Z so they affect points, not the timeline. Bail out
      // here for the keys it claims and let its window listener run.
      if (state.sketch.active) {
        const k = e.key.toLowerCase()
        if ((k === 'z' || k === 'y') && ctrl) return
      }

      // Standard views: Shift+1..7 and the numpad (lib/view-shortcuts.ts).
      // Read from the physical key, so it is checked before the switch on
      // `e.key` below -- the old numpad cases compared `e.key` to 'numpad1',
      // which is a `code`, never a `key`, so no view key ever fired.
      const view = viewPresetForKey(e)
      if (view) {
        e.preventDefault()
        state.setCameraPreset(view)
        return
      }

      switch (e.key.toLowerCase()) {
        // K: start a sketch (on XY; right-click a face to sketch on it).
        case 'k':
          if (!ctrl && !e.altKey && !state.sketch.active) {
            e.preventDefault()
            void useSketchBench.getState().open('xy')
          }
          break

        // Undo / Redo
        case 'z':
          if (ctrl) {
            e.preventDefault()
            if (e.shiftKey) {
              void timelineAction('redo')
            } else {
              void timelineAction('undo')
            }
          }
          break

        // Redo (the Windows convention, beside Ctrl+Shift+Z)
        case 'y':
          if (ctrl && !e.shiftKey) {
            e.preventDefault()
            void timelineAction('redo')
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

        // Delete selected, through the kernel (see `deleteSelectedParts`).
        case 'delete':
        case 'backspace':
          if (!ctrl) {
            e.preventDefault()
            void deleteSelectedParts()
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
