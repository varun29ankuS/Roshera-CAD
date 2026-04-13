import { useSceneStore } from '@/stores/scene-store'
import { useThree } from '@react-three/fiber'
import { useEffect, useRef } from 'react'

/**
 * Manages selection-mode visual state and cursor changes.
 * Actual face/edge/vertex picking will be added in PR 3.
 * This component sets up the groundwork: cursor changes based on mode,
 * and placeholder for sub-element highlight overlays.
 */
export function SelectionOverlay() {
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const gl = useThree((s) => s.gl)
  const canvasRef = useRef(gl.domElement)

  useEffect(() => {
    canvasRef.current = gl.domElement
  }, [gl])

  useEffect(() => {
    const canvas = canvasRef.current
    const cursor = selectionMode === 'face' ? 'cell'
      : selectionMode === 'edge' || selectionMode === 'vertex' ? 'crosshair'
      : 'default'
    canvas.style.cursor = cursor
    return () => { canvas.style.cursor = 'default' }
  }, [selectionMode])

  return null
}
