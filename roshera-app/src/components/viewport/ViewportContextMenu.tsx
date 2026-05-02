import { useEffect, useRef, useCallback } from 'react'
import { Eye, EyeOff, Trash2 } from 'lucide-react'
import { useSceneStore } from '@/stores/scene-store'

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
