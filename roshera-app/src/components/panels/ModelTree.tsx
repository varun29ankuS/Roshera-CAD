import { useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Eye, EyeOff, Trash2, Pencil } from 'lucide-react'
import { useSceneStore, isStandardPlane, type CADObject, type SketchPlane } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { sketchApi, type ServerSketchSession } from '@/lib/sketch-api'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'

const API_BASE = import.meta.env.VITE_API_URL || ''

// ─── Preview mode (opt-in via ?preview URL param) ───────────────────

function isPreviewMode(): boolean {
  if (typeof window === 'undefined') return false
  return new URLSearchParams(window.location.search).has('preview')
}

// ─── Backend hierarchy types (GET /api/hierarchy/{session_id}) ─────

interface HierarchyResponse {
  success: boolean
  data: {
    hierarchy: ProjectHierarchy
    workflow_state: WorkflowState
  }
}

interface ProjectHierarchy {
  root_assembly: Assembly
  part_library: Record<string, PartDefinition>
}

interface Assembly {
  id: string
  name: string
  children: HierarchyNode[]
}

type HierarchyNode =
  | { PartInstance: PartInstance }
  | { SubAssembly: Assembly }

interface PartInstance {
  instance_id: string
  definition_id: string
  instance_number: number
  transform: { position: number[]; rotation: number[]; scale: number[] }
  is_unique: boolean
}

interface PartDefinition {
  id: string
  name: string
  geometry_id: string
  features: Feature[]
  version: number
}

interface Feature {
  id: string
  feature_type: string
  parameters: Record<string, number>
}

interface WorkflowState {
  current_stage: string
  current_context: string
  available_tools: string[]
}

// ─── Tree node types ────────────────────────────────────────────────

interface TreeNode {
  id: string
  name: string
  type: string
  symbol: string
  children?: TreeNode[]
  visible?: boolean
  locked?: boolean
}

// ─── Unicode symbol map (terminal aesthetic) ────────────────────────

function symbolForType(type: string): string {
  switch (type.toLowerCase()) {
    case 'box': return '▣'
    case 'sphere': return '◯'
    case 'cylinder': return '⊟'
    case 'cone': return '△'
    case 'torus': return '◎'
    case 'assembly': return '▦'
    case 'group': return '▤'
    case 'sketch': return '✎'
    case 'extrude': return '↑'
    case 'revolve': return '↻'
    case 'fillet': return '◜'
    case 'chamfer': return '⬡'
    case 'pattern': return '▦'
    case 'hole': return '⊙'
    case 'part': return '◆'
    default: return '•'
  }
}

// ─── Tree row (terminal lineage style) ──────────────────────────────

function TreeItem({
  node,
  isLast,
  ancestorIsLast,
  selectedIds,
  onSelect,
  onToggleVisibility,
  onToggleLock,
  onContextMenu,
}: {
  node: TreeNode
  isLast: boolean
  ancestorIsLast: boolean[] // one entry per ancestor depth: true = ancestor was last sibling
  selectedIds: Set<string>
  onSelect: (id: string, additive: boolean) => void
  onToggleVisibility: (id: string) => void
  onToggleLock: (id: string) => void
  onContextMenu: (e: React.MouseEvent, node: TreeNode) => void
}) {
  const [expanded, setExpanded] = useState(true)
  const isSelected = selectedIds.has(node.id)
  const hasChildren = !!node.children && node.children.length > 0

  // Build lineage prefix: │ for ancestors with more siblings, spaces otherwise.
  const lineagePrefix = ancestorIsLast.map((last) => (last ? '   ' : '│  ')).join('')

  // Branch char + arm char (the latter doubles as the expand/collapse affordance).
  const branchChar = isLast ? '└' : '├'
  let armChar: string
  if (hasChildren) {
    armChar = expanded ? '▾' : '▸'
  } else {
    armChar = '─'
  }

  const visible = node.visible !== false
  const locked = !!node.locked

  return (
    <div>
      <div
        className={cn(
          'flex items-center cursor-pointer select-none transition-colors group font-mono text-[13px] leading-snug',
          isSelected
            ? 'bg-primary/15 text-primary'
            : 'text-foreground/70 hover:bg-accent/50 hover:text-foreground',
        )}
        onClick={(e) => onSelect(node.id, e.shiftKey || e.ctrlKey || e.metaKey)}
        onContextMenu={(e) => onContextMenu(e, node)}
      >
        {/* Lineage (ancestor connectors) — non-interactive */}
        {lineagePrefix.length > 0 && (
          <span className="whitespace-pre text-muted-foreground/50 shrink-0">
            {lineagePrefix}
          </span>
        )}

        {/* Branch + arm — arm is the expand/collapse click target */}
        <span className="whitespace-pre shrink-0">
          <span className="text-muted-foreground/50">{branchChar}</span>
          {hasChildren ? (
            <button
              onClick={(e) => {
                e.stopPropagation()
                setExpanded(!expanded)
              }}
              className="text-foreground/70 hover:text-foreground transition-colors"
              aria-label={expanded ? 'Collapse' : 'Expand'}
              aria-expanded={expanded}
            >
              {armChar}
            </button>
          ) : (
            <span className="text-muted-foreground/50">{armChar}</span>
          )}
          <span className="text-muted-foreground/50"> </span>
        </span>

        {/* Type symbol */}
        <span className="shrink-0 text-muted-foreground/80 mr-1">{node.symbol}</span>

        {/* Name */}
        <span className="truncate flex-1">{node.name}</span>

        {/* Visibility / lock — hover-revealed unicode */}
        <div className="flex items-center gap-1 px-1 opacity-0 group-hover:opacity-100 focus-within:opacity-100 transition-opacity shrink-0">
          <button
            onClick={(e) => {
              e.stopPropagation()
              onToggleVisibility(node.id)
            }}
            // Hidden rows render the indicator in orange so the user
            // can scan the tree and immediately spot what's been
            // toggled off, instead of relying on the dim ○/● shape
            // alone (which is easy to miss against muted text).
            className={cn(
              'transition-colors w-3 text-center',
              visible
                ? 'text-foreground/60 hover:text-foreground'
                : 'text-orange-500 hover:text-orange-400',
            )}
            aria-label={visible ? 'Hide' : 'Show'}
            title={visible ? 'Hide' : 'Show'}
          >
            {visible ? '●' : '○'}
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation()
              onToggleLock(node.id)
            }}
            className="text-foreground/60 hover:text-foreground transition-colors w-3 text-center"
            aria-label={locked ? 'Unlock' : 'Lock'}
            title={locked ? 'Unlock' : 'Lock'}
          >
            {locked ? '■' : '□'}
          </button>
        </div>
      </div>

      {hasChildren && expanded && (
        <div>
          {node.children!.map((child, idx) => (
            <TreeItem
              key={child.id}
              node={child}
              isLast={idx === node.children!.length - 1}
              ancestorIsLast={[...ancestorIsLast, isLast]}
              selectedIds={selectedIds}
              onSelect={onSelect}
              onToggleVisibility={onToggleVisibility}
              onToggleLock={onToggleLock}
              onContextMenu={onContextMenu}
            />
          ))}
        </div>
      )}
    </div>
  )
}

// ─── Convert backend hierarchy to tree nodes ────────────────────────

function hierarchyToNodes(hierarchy: ProjectHierarchy): TreeNode[] {
  const { root_assembly, part_library } = hierarchy

  function convertNode(node: HierarchyNode): TreeNode {
    if ('PartInstance' in node) {
      const inst = node.PartInstance
      const def = part_library[inst.definition_id]
      const name = def ? `${def.name} #${inst.instance_number}` : `Part #${inst.instance_number}`
      const children = def?.features.map((f) => ({
        id: f.id,
        name: `${f.feature_type}`,
        type: f.feature_type.toLowerCase(),
        symbol: symbolForType(f.feature_type),
      }))
      return {
        id: inst.instance_id,
        name,
        type: 'part',
        symbol: symbolForType('part'),
        children: children && children.length > 0 ? children : undefined,
      }
    } else {
      const asm = node.SubAssembly
      return {
        id: asm.id,
        name: asm.name,
        type: 'assembly',
        symbol: symbolForType('assembly'),
        children: asm.children.map(convertNode),
      }
    }
  }

  return root_assembly.children.map(convertNode)
}

// ─── Build sketch nodes from server-tracked sessions ────────────────

/**
 * One tree node per known sketch session. Children describe the
 * sketch's plane and tool — purely informational, the actual edit
 * action is on the parent node's context menu ("Edit sketch").
 *
 * Sessions are sorted by `created_at` so the order in the tree is
 * stable across re-renders — backend `Map` iteration order is
 * insertion-order today but we don't want to rely on that.
 */
function sketchPlaneLabel(plane: SketchPlane): string {
  // Standard planes serialise as bare strings ('xy' | 'xz' | 'yz');
  // face-anchored sketches arrive as { origin, u_axis, v_axis } objects
  // which have no `.toUpperCase()`. Calling it unconditionally on a
  // discriminated union throws at runtime and tears down the model
  // tree render — that's how Task #38's plane refactor leaked into the
  // browser as a "nothing renders, can't right-click anything" state.
  return isStandardPlane(plane) ? plane.toUpperCase() : 'FACE'
}

function sketchesToNodes(sketches: Map<string, ServerSketchSession>): TreeNode[] {
  return Array.from(sketches.values())
    .sort((a, b) => a.created_at - b.created_at)
    .map((s, idx) => {
      const planeLabel = sketchPlaneLabel(s.plane)
      const ptSuffix = s.points.length === 1 ? 'pt' : 'pts'
      return {
        id: s.id,
        name: `Sketch ${idx + 1} (${planeLabel} · ${s.points.length} ${ptSuffix})`,
        type: 'sketch',
        symbol: symbolForType('sketch'),
        visible: true,
        locked: false,
      }
    })
}

// ─── Build tree from local scene store (fallback) ───────────────────

function sceneToNodes(
  objects: Map<string, CADObject>,
  objectOrder: string[],
): TreeNode[] {
  return objectOrder
    .map((id) => {
      const obj = objects.get(id)
      if (!obj || obj.parentId) return null
      return buildLocalNode(obj, objects)
    })
    .filter(Boolean) as TreeNode[]
}

/**
 * Pull the source-sketch id off an object's analytical-geometry params,
 * if any. Extrude broadcasts (api-server `extrude_sketch`) embed
 * `sketch_id` + `plane` in `parameters`; the linkage survives even when
 * the sketch session itself is consumed (default), so the model tree
 * can still surface the sketch as a child feature post-extrude.
 */
function ownedSketchInfo(obj: CADObject): { id: string; plane?: string } | null {
  const params = obj.analyticalGeometry?.params
  if (!params) return null
  const id = params['sketch_id']
  if (typeof id !== 'string' || id.length === 0) return null
  const planeRaw = params['plane']
  const plane = typeof planeRaw === 'string' ? planeRaw : undefined
  return { id, plane }
}

function buildLocalNode(
  obj: CADObject,
  allObjects: Map<string, CADObject>,
): TreeNode {
  const children: TreeNode[] = []
  for (const [, child] of allObjects) {
    if (child.parentId === obj.id) {
      children.push(buildLocalNode(child, allObjects))
    }
  }

  // Synthesize a "Sketch" child for any object that was produced from a
  // sketch (extrude today; revolve/sweep/loft when those land). The
  // params blob is authoritative — the sketch session itself may have
  // been consumed by the operation, but the id + plane are preserved
  // on the resulting feature.
  const owned = ownedSketchInfo(obj)
  if (owned) {
    const planeLabel = owned.plane ? owned.plane.toUpperCase() : '?'
    children.push({
      id: owned.id,
      name: `Sketch (${planeLabel})`,
      type: 'sketch',
      symbol: symbolForType('sketch'),
      visible: true,
      locked: false,
    })
  }

  return {
    id: obj.id,
    name: obj.name,
    type: obj.objectType,
    symbol: symbolForType(obj.objectType),
    visible: obj.visible,
    locked: obj.locked,
    children: children.length > 0 ? children : undefined,
  }
}

// ─── Mock data for preview mode ─────────────────────────────────────

const MOCK_TREE_NODES: TreeNode[] = [
  {
    id: 'mock-box-1',
    name: 'Box #1',
    type: 'box',
    symbol: symbolForType('box'),
    visible: true,
    locked: false,
    children: [
      { id: 'mock-extrude-1', name: 'Extrude', type: 'extrude', symbol: symbolForType('extrude'), visible: true },
      { id: 'mock-fillet-1', name: 'Fillet', type: 'fillet', symbol: symbolForType('fillet'), visible: true },
    ],
  },
  {
    id: 'mock-sphere-1',
    name: 'Sphere',
    type: 'sphere',
    symbol: symbolForType('sphere'),
    visible: true,
    locked: false,
  },
  {
    id: 'mock-union-1',
    name: 'Union #1',
    type: 'assembly',
    symbol: symbolForType('assembly'),
    visible: true,
    locked: false,
    children: [
      { id: 'mock-box-2', name: 'Box #2', type: 'box', symbol: symbolForType('box'), visible: true },
      {
        id: 'mock-cyl-1',
        name: 'Cylinder',
        type: 'cylinder',
        symbol: symbolForType('cylinder'),
        visible: true,
        children: [
          { id: 'mock-chamfer-1', name: 'Chamfer', type: 'chamfer', symbol: symbolForType('chamfer'), visible: true },
        ],
      },
    ],
  },
  {
    id: 'mock-sketch-1',
    name: 'Sketch',
    type: 'sketch',
    symbol: symbolForType('sketch'),
    visible: false,
    locked: true,
  },
]

// ─── Main panel ─────────────────────────────────────────────────────

interface TreeContextMenuState {
  x: number
  y: number
  node: TreeNode
}

export function ModelTree({ onCollapse }: { onCollapse?: () => void } = {}) {
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const selectObject = useSceneStore((s) => s.selectObject)
  const updateObject = useSceneStore((s) => s.updateObject)
  const serverSketches = useSceneStore((s) => s.serverSketches)
  const wsStatus = useWSStore((s) => s.status)
  const sessionId = useWSStore((s) => s.sessionId)

  const [backendNodes, setBackendNodes] = useState<TreeNode[] | null>(null)
  const [menu, setMenu] = useState<TreeContextMenuState | null>(null)

  const handleNodeContextMenu = useCallback(
    (e: React.MouseEvent, node: TreeNode) => {
      e.preventDefault()
      e.stopPropagation()
      setMenu({ x: e.clientX, y: e.clientY, node })
    },
    [],
  )

  const closeMenu = useCallback(() => setMenu(null), [])

  // Try to fetch hierarchy from backend
  const fetchHierarchy = useCallback(async () => {
    const sid = sessionId || 'default-session'
    try {
      const resp = await fetch(`/api/hierarchy/${sid}`)
      if (resp.ok) {
        const data: HierarchyResponse = await resp.json()
        if (data.success && data.data?.hierarchy) {
          setBackendNodes(hierarchyToNodes(data.data.hierarchy))
          return
        }
      }
    } catch {
      // Backend not running — fall back to local
    }
    setBackendNodes(null)
  }, [sessionId])

  useEffect(() => {
    if (wsStatus === 'connected') {
      // Fetch on connect — async to avoid synchronous setState in effect
      void Promise.resolve().then(fetchHierarchy)
    }
  }, [wsStatus, fetchHierarchy])

  // Poll for hierarchy updates (pause when tab is hidden)
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | null = null

    function startPolling() {
      stopPolling()
      timer = setInterval(fetchHierarchy, 5000)
    }

    function stopPolling() {
      if (timer) { clearInterval(timer); timer = null }
    }

    function handleVisibility() {
      if (document.visibilityState === 'visible') {
        fetchHierarchy()
        startPolling()
      } else {
        stopPolling()
      }
    }

    startPolling()
    document.addEventListener('visibilitychange', handleVisibility)
    return () => {
      stopPolling()
      document.removeEventListener('visibilitychange', handleVisibility)
    }
  }, [fetchHierarchy])

  // Preview mode short-circuits to mock data; otherwise use backend hierarchy
  // if available *and non-empty*, falling back to the local scene store.
  // The hierarchy endpoint returns `success: true` with empty children when
  // the project has no Parts yet — but the scene store is already mirroring
  // ObjectCreated broadcasts, so falling through gives the user something
  // to right-click instead of a permanently empty Browser.
  const localNodes = sceneToNodes(objects, objectOrder)
  // Sketches that are already represented as a child of an extrude
  // (or any other operation that records `sketch_id` in its params)
  // must not also appear at the top level — that would render the
  // same sketch twice for the same feature.
  const ownedSketchIds = new Set<string>()
  for (const obj of objects.values()) {
    const owned = ownedSketchInfo(obj)
    if (owned) ownedSketchIds.add(owned.id)
  }
  const sketchNodes = sketchesToNodes(serverSketches).filter(
    (n) => !ownedSketchIds.has(n.id),
  )
  // Sketches appear above solids — they're the source the user
  // sketches *into*, and grouping them visually at the top mirrors
  // every CAD package's feature-tree convention.
  const objectNodes = isPreviewMode()
    ? MOCK_TREE_NODES
    : (backendNodes && backendNodes.length > 0 ? backendNodes : localNodes)
  const treeNodes = [...sketchNodes, ...objectNodes]

  const handleToggleVisibility = useCallback(
    (id: string) => {
      const obj = objects.get(id)
      if (obj) updateObject(id, { visible: !obj.visible })
    },
    [objects, updateObject],
  )

  const handleToggleLock = useCallback(
    (id: string) => {
      const obj = objects.get(id)
      if (obj) updateObject(id, { locked: !obj.locked })
    },
    [objects, updateObject],
  )

  return (
    <div className="flex flex-col h-full">
      <div className="cad-panel-header flex items-center gap-1.5 font-mono">
        <span className="flex-1">browser</span>
        {onCollapse && (
          <button
            onClick={onCollapse}
            className="cad-icon-btn h-5 w-5"
            title="Collapse browser"
            aria-label="Collapse browser"
          >
            «
          </button>
        )}
      </div>
      <ScrollArea className="flex-1">
        {treeNodes.length === 0 ? (
          <div className="p-3 text-[13px] text-muted-foreground/60 text-center font-mono">
            ∅ no objects in scene
          </div>
        ) : (
          <div className="py-1 px-1">
            {treeNodes.map((node, idx) => (
              <TreeItem
                key={node.id}
                node={node}
                isLast={idx === treeNodes.length - 1}
                ancestorIsLast={[]}
                selectedIds={selectedIds}
                onSelect={selectObject}
                onToggleVisibility={handleToggleVisibility}
                onToggleLock={handleToggleLock}
                onContextMenu={handleNodeContextMenu}
              />
            ))}
          </div>
        )}
      </ScrollArea>
      {menu && <TreeContextMenu menu={menu} onClose={closeMenu} />}
    </div>
  )
}

// ─── Per-node context menu (right-click → rename / hide / delete) ───
//
// Positioned at the cursor with edge-aware flipping so it never falls
// off-screen. Acts on both backend-only nodes (visible from API) and
// local fallback nodes from the scene store. Delete routes through the
// authoritative DELETE /api/geometry/{uuid}; the resulting ObjectDeleted
// broadcast is what removes the object locally — backend stays the
// single source of truth (mirrors `ViewportContextMenu`).

function TreeContextMenu({
  menu,
  onClose,
}: {
  menu: TreeContextMenuState
  onClose: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const updateObject = useSceneStore((s) => s.updateObject)
  const editServerSketch = useSceneStore((s) => s.editServerSketch)
  const clearServerSketchId = useSceneStore((s) => s.clearServerSketchId)
  const isSketchNode = menu.node.type === 'sketch'
  const localObj = useSceneStore.getState().objects.get(menu.node.id)
  const isVisible = localObj?.visible ?? menu.node.visible ?? true

  // Edge-aware positioning — flip the menu inward whenever the click
  // landed close enough to a viewport edge that the natural downward /
  // rightward layout would clip.
  const [pos, setPos] = useState<{ x: number; y: number; ready: boolean }>({
    x: menu.x,
    y: menu.y,
    ready: false,
  })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight
    const margin = 8
    let x = menu.x
    let y = menu.y
    if (x + rect.width > vw - margin) {
      x = Math.max(margin, menu.x - rect.width)
    }
    if (y + rect.height > vh - margin) {
      y = Math.max(margin, menu.y - rect.height)
    }
    setPos({ x, y, ready: true })
  }, [menu.x, menu.y])

  useEffect(() => {
    const onMouseDown = (e: MouseEvent) => {
      const el = ref.current
      if (el && e.target instanceof Node && el.contains(e.target)) return
      onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onMouseDown)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onMouseDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [onClose])

  const handleRename = useCallback(() => {
    onClose()
    if (!localObj) return
    const next = window.prompt('Rename', localObj.name)?.trim()
    if (!next || next === localObj.name) return
    updateObject(localObj.id, { name: next })
  }, [localObj, updateObject, onClose])

  const handleEditSketch = useCallback(() => {
    onClose()
    editServerSketch(menu.node.id)
  }, [editServerSketch, menu.node.id, onClose])

  const handleToggleVisibility = useCallback(() => {
    onClose()
    if (!localObj) return
    updateObject(localObj.id, { visible: !localObj.visible })
  }, [localObj, updateObject, onClose])

  const handleDeleteSketch = useCallback(async () => {
    onClose()
    try {
      // Optimistically drop locally so the tree updates immediately;
      // the backend `SketchDeleted` broadcast (if/when wired) would
      // also clear, but the REST DELETE is the authoritative call.
      await sketchApi.delete(menu.node.id)
      clearServerSketchId(menu.node.id)
    } catch (err) {
      console.error('[browser] sketch delete failed:', err)
    }
  }, [menu.node.id, clearServerSketchId, onClose])

  const handleDeleteObject = useCallback(async () => {
    onClose()
    try {
      const resp = await fetch(`${API_BASE}/api/geometry/${menu.node.id}`, {
        method: 'DELETE',
      })
      if (!resp.ok) {
        const text = await resp.text().catch(() => '')
        console.error('[browser] delete failed:', resp.status, text)
      }
      // Local removal happens via the ObjectDeleted broadcast (ws-bridge.ts)
      // so a server-side failure leaves the scene in sync with the kernel.
    } catch (err) {
      console.error('[browser] delete error:', err)
    }
  }, [menu.node.id, onClose])

  const handleDelete = isSketchNode ? handleDeleteSketch : handleDeleteObject
  const handleEdit = isSketchNode ? handleEditSketch : handleRename
  const editEnabled = isSketchNode || !!localObj
  const visibilityEnabled = !isSketchNode && !!localObj
  const editLabel = isSketchNode ? 'Edit sketch' : 'Rename'

  // Render via portal so the menu escapes the model-tree panel's
  // containing block and clip region. The browser panel uses
  // `backdrop-blur-sm` (which creates a containing block for fixed
  // descendants) AND `overflow-hidden` — together they cause a
  // `position: fixed` menu rendered inline to be positioned relative
  // to the panel and clipped to its 224px width, making it invisible
  // for any click that lands more than ~180px from the panel's
  // top-left. Portaling to `document.body` sidesteps both effects.
  return createPortal(
    <div
      ref={ref}
      className="fixed z-[1000] cad-panel min-w-[180px] py-1 text-[12px] shadow-lg select-none"
      style={{ left: pos.x, top: pos.y, visibility: pos.ready ? 'visible' : 'hidden' }}
      role="menu"
    >
      <div className="px-3 py-1 text-[10px] uppercase tracking-wider text-muted-foreground/70 truncate">
        {menu.node.name}
      </div>
      <div className="my-1 border-t border-border/50" />
      <TreeMenuItem onClick={handleEdit} disabled={!editEnabled}>
        <Pencil size={13} />
        {editLabel}
      </TreeMenuItem>
      <TreeMenuItem onClick={handleToggleVisibility} disabled={!visibilityEnabled}>
        {isVisible ? <EyeOff size={13} /> : <Eye size={13} />}
        {isVisible ? 'Hide' : 'Show'}
      </TreeMenuItem>
      <div className="my-1 border-t border-border/50" />
      <TreeMenuItem onClick={handleDelete} danger>
        <Trash2 size={13} />
        Delete
      </TreeMenuItem>
    </div>,
    document.body,
  )
}

interface TreeMenuItemProps {
  children: React.ReactNode
  onClick: () => void
  danger?: boolean
  disabled?: boolean
}

function TreeMenuItem({ children, onClick, danger, disabled }: TreeMenuItemProps) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      disabled={disabled}
      className={cn(
        'w-full flex items-center gap-2 px-3 py-1.5 text-left transition-colors',
        disabled
          ? 'text-muted-foreground/40 cursor-not-allowed'
          : danger
            ? 'text-destructive hover:bg-accent/40'
            : 'text-foreground hover:bg-accent/40',
      )}
    >
      {children}
    </button>
  )
}
