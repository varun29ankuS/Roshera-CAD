import { useSceneStore } from '@/stores/scene-store'
import { useThree } from '@react-three/fiber'
import { useEffect } from 'react'

/**
 * Manages selection-mode visual state and cursor changes.
 * Actual face/edge/vertex picking will be added in PR 3.
 * This component sets up the groundwork: cursor changes based on mode,
 * and placeholder for sub-element highlight overlays.
 */
export function SelectionOverlay() {
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const { gl } = useThree()

  useEffect(() => {
    const canvas = gl.domElement
    switch (selectionMode) {
      case 'face':
        canvas.style.cursor = 'cell'
        break
      case 'edge':
        canvas.style.cursor = 'crosshair'
        break
      case 'vertex':
        canvas.style.cursor = 'crosshair'
        break
      default:
        canvas.style.cursor = 'default'
    }
    return () => {
      canvas.style.cursor = 'default'
    }
  }, [selectionMode, gl])

  return null
}
