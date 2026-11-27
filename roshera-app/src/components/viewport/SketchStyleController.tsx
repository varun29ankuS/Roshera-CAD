import { useEffect } from 'react'
import * as THREE from 'three'
import { useThree } from '@react-three/fiber'
import { useSceneStore } from '@/stores/scene-store'
import { useThemeStore } from '@/stores/theme-store'
import { viewportSketchUniforms } from '@/lib/sketch-material'
import { nextReveal, prefersReducedMotion, revealStart, REVEAL_STEP_MS, sketchColors } from '@/lib/sketch-style'

/**
 * Keeps the viewport's Sketch display mode's ONE shared uniform set current
 * (lib/sketch-material.ts): the theme's paper and ink, the drawing buffer's
 * size, and the stepped top-to-bottom print when the mode is switched on
 * (instant under reduced motion). The canvas renders on demand, so every
 * change asks for a frame. Renders nothing.
 */
export function SketchStyleController() {
  const on = useSceneStore((s) => s.sketchStyle)
  const theme = useThemeStore((s) => s.theme)
  const gl = useThree((s) => s.gl)
  const size = useThree((s) => s.size)
  const dpr = useThree((s) => s.viewport.dpr)
  const invalidate = useThree((s) => s.invalidate)

  // Colours: read after the theme's class has reached the document.
  useEffect(() => {
    if (!on) return
    const frame = window.requestAnimationFrame(() => {
      const c = sketchColors()
      viewportSketchUniforms.uSketchInk.value.set(...c.ink)
      viewportSketchUniforms.uSketchFace.value.set(...c.face)
      invalidate()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [on, theme, invalidate])

  useEffect(() => {
    const v = new THREE.Vector2()
    gl.getDrawingBufferSize(v)
    viewportSketchUniforms.uSketchRes.value.copy(v)
    invalidate()
  }, [gl, size, dpr, invalidate])

  // The print: four fixed steps when the mode comes on.
  useEffect(() => {
    if (!on) return
    let reveal = revealStart(prefersReducedMotion())
    viewportSketchUniforms.uSketchReveal.value = reveal
    invalidate()
    let timer: number | null = null
    const step = () => {
      if (reveal >= 1) return
      timer = window.setTimeout(() => {
        reveal = nextReveal(reveal)
        viewportSketchUniforms.uSketchReveal.value = reveal
        invalidate()
        step()
      }, REVEAL_STEP_MS)
    }
    step()
    return () => {
      if (timer !== null) window.clearTimeout(timer)
      viewportSketchUniforms.uSketchReveal.value = 1
    }
  }, [on, invalidate])

  return null
}
