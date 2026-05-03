import { useEffect, useRef, useCallback } from 'react'
import { Eye, EyeOff, PenTool, Trash2 } from 'lucide-react'
import { useSceneStore } from '@/stores/scene-store'
import { sketchApi } from '@/lib/sketch-api'

const API_BASE = import.meta.env.VITE_API_URL || ''

/**
 * Right-click context menu for the 3D viewport.
 *
 * Opens at the cursor whenever a CADMesh fires onContextMenu. The menu
 * acts on `contextMenu.objectId` (the right-clicked object). Closes on
 * outside click, Escape, or any item click. The Delete action calls the
 * authoritative DELETE /api/geometry/{uuid} endpoint — the resulting
 * `ObjectDeleted` broadcast is what actually removes the object from
 * the local store, keeping the kernel as the single source of truth.
 * Hide/Show toggles the local `visible` flag (display-only state, no
 * backend roundtrip needed).
 */
export function ViewportContextMenu() {
  const menu = useSceneStore((s) => s.contextMenu)
  const close = useSceneStore((s) => s.closeContextMenu)
  const updateObject = useSceneStore((s) => s.updateObject)
  const ref = useRef<HTMLDivElement>(null)

  // Outside-click + Escape dismiss. Wired only while the menu is open
  // so the global listeners don't bleed across renders.
  useEffect(() => {
    if (!menu) return
    const onMouseDown = (e: MouseEvent) => {
      const el = ref.current
      if (el && e.target instanceof Node && el.contains(e.target)) return
      close()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [menu, close])

  const handleDelete = useCallback(async () => {
    if (!menu) return
    close()
    try {
      const resp = await fetch(`${API_BASE}/api/geometry/${menu.objectId}`, {
        method: 'DELETE',
      })
      if (!resp.ok) {
        const text = await resp.text().catch(() => '')
        console.error('[viewport] delete failed:', resp.status, text)
      }
      // Local removal happens via the ObjectDeleted broadcast — see
      // ws-bridge.ts. Avoid optimistic local removal so a server-side
      // failure doesn't desync the scene from the kernel.
    } catch (err) {
      console.error('[viewport] delete error:', err)
    }
  }, [menu, close])

  const handleToggleVisibility = useCallback(() => {
    if (!menu) return
    const obj = useSceneStore.getState().objects.get(menu.objectId)
    if (obj) updateObject(menu.objectId, { visible: !obj.visible })
    close()
  }, [menu, updateObject, close])

  const handleSketchOnFace = useCallback(async () => {
    if (!menu || menu.faceId === undefined) return
    close()
    try {
      // Backend resolves the face → planar surface → SketchPlane::Custom
      // {origin, u_axis, v_axis} in the same right-handed frame the
      // kernel uses, so the in-canvas overlay and any subsequent
      // extrude all see consistent (u, v) coordinates.
      const plane = await sketchApi.planeFromFace(menu.objectId, menu.faceId)
      useSceneStore.getState().enterSketch(plane, 'polyline')
    } catch (err) {
      // Most common failure here is a non-planar face (face/surface
      // wasn't a Plane). Surface to the dev console rather than a
      // toast so we don't silently drop the rejection — the picker
      // already prevents non-face right-clicks from reaching this
      // path, so any failure is a kernel surprise worth logging.
      console.error('[viewport] sketch-on-face failed:', err)
    }
  }, [menu, close])

  if (!menu) return null

  const obj = useSceneStore.getState().objects.get(menu.objectId)
  const isVisible = obj?.visible ?? true

  return (
    <div
      ref={ref}
      className="fixed z-50 cad-panel min-w-[160px] py-1 text-[12px] shadow-lg select-none"
      style={{ left: menu.x, top: menu.y }}
      role="menu"
    >
      <MenuItem onClick={handleToggleVisibility}>
        {isVisible ? <EyeOff size={13} /> : <Eye size={13} />}
        {isVisible ? 'Hide' : 'Show'}
      </MenuItem>
      {menu.faceId !== undefined && (
        <>
          <div className="my-1 border-t border-border/50" />
          <MenuItem onClick={handleSketchOnFace}>
            <PenTool size={13} />
            Sketch on this face
          </MenuItem>
        </>
      )}
      <div className="my-1 border-t border-border/50" />
      <MenuItem onClick={handleDelete} danger>
        <Trash2 size={13} />
        Delete
      </MenuItem>
    </div>
  )
}

interface MenuItemProps {
  children: React.ReactNode
  onClick: () => void
  danger?: boolean
}

function MenuItem({ children, onClick, danger }: MenuItemProps) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      className={[
        'w-full flex items-center gap-2 px-3 py-1.5 text-left',
        'hover:bg-accent/40 transition-colors',
        danger ? 'text-destructive' : 'text-foreground',
      ].join(' ')}
    >
      {children}
    </button>
  )
}
