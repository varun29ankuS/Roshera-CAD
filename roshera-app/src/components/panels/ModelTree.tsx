import { useState, useEffect, useLayoutEffect, useCallback, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Eye, EyeOff, Trash2, Pencil, Plus } from 'lucide-react'
import { useSceneStore, isStandardPlane, type CADObject, type SketchPlane } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { sketchApi, type ServerSketchSession } from '@/lib/sketch-api'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { CreateDatumDialog } from '@/components/panels/CreateDatumDialog'

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
    case 'datumgroup': return '⌖'
    case 'datumorigin': return '⊕'
    case 'datumplane': return '▱'
    case 'datumaxis': return '↔'
    default: return '•'
  }
}

// ─── Datum DTO (GET /api/datums) ────────────────────────────────────

interface DatumDto {
  id: number
  name: string
  kind: 'origin' | 'plane' | 'axis'
  plane_orientation?: string
  axis_direction?: string
  origin: [number, number, number]
  visible: boolean
  is_default: boolean
}

interface DatumListResponse {
  datums: DatumDto[]
}

/**
 * Build a tree node for the Datums group from the backend snapshot.
 * Tree-node ids use the `datum:<numeric-id>` prefix so the visibility
 * toggle handler can route the click back to `PATCH /api/datums/:id/...`
 * instead of mutating the local scene-store.
 */
function datumsToNodes(datums: DatumDto[]): TreeNode[] {
  if (datums.length === 0) return []
  const children: TreeNode[] = datums.map((d) => {
    const subtype =
      d.kind === 'origin' ? 'datumorigin' :
      d.kind === 'plane' ? 'datumplane' :
      'datumaxis'
    return {
      id: `datum:${d.id}`,
      name: d.name,
      type: subtype,
      symbol: symbolForType(subtype),
      visible: d.visible,
      locked: d.is_default,
    }
  })
  return [
    {
      id: 'datum:group',
      name: 'datums',
      type: 'datumgroup',
      symbol: symbolForType('datumgroup'),
      visible: children.some((c) => c.visible),
      locked: true,
      children,
    },
  ]
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
  onAdd,
}: {
  node: TreeNode
  isLast: boolean
  ancestorIsLast: boolean[] // one entry per ancestor depth: true = ancestor was last sibling
  selectedIds: Set<string>
  onSelect: (id: string, additive: boolean) => void
  onToggleVisibility: (id: string) => void
  onToggleLock: (id: string) => void
  onContextMenu: (e: React.MouseEvent, node: TreeNode) => void
  /**
   * If supplied, render a "+" affordance on the row (hover-revealed).
   * Currently used only for the datum group row to open the
   * CreateDatumDialog. Returning `null` from the parent keeps the
   * affordance off for every other row without adding props.
   */
  onAdd?: (node: TreeNode) => void
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
          {onAdd && (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onAdd(node)
              }}
              className="text-foreground/60 hover:text-foreground transition-colors w-3 flex items-center justify-center"
              aria-label="Add"
              title="New datum"
            >
              <Plus size={11} />
            </button>
          )}
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
              onAdd={onAdd}
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

export function ModelTree({
  expanded = true,
  onToggle,
}: {
  /**
   * When `false`, the header chip is rendered but the tree content
   * below is hidden. The chip itself stays as a permanent anchor so
   * the user can re-expand without hunting for a separate launcher.
   */
  expanded?: boolean
  /**
   * Called when the header chip is activated. The parent owns the
   * `expanded` state so the collapsed/expanded preference can be
   * persisted alongside other layout state.
   */
  onToggle?: () => void
} = {}) {
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const selectObject = useSceneStore((s) => s.selectObject)
  const updateObject = useSceneStore((s) => s.updateObject)
  const serverSketches = useSceneStore((s) => s.serverSketches)
  const wsStatus = useWSStore((s) => s.status)
  const sessionId = useWSStore((s) => s.sessionId)

  const [backendNodes, setBackendNodes] = useState<TreeNode[] | null>(null)
  const [datums, setDatums] = useState<DatumDto[]>([])
  const [menu, setMenu] = useState<TreeContextMenuState | null>(null)
  const [createDatumOpen, setCreateDatumOpen] = useState(false)

  const handleNodeContextMenu = useCallback(
    (e: React.MouseEvent, node: TreeNode) => {
      e.preventDefault()
      e.stopPropagation()
      // The datum group row has no rename / delete semantics — its
      // only mutable affordance is the hover-revealed "+" button.
      // Open the context menu would surface all-disabled entries,
      // which is just noise. Suppress it here.
      if (node.id === 'datum:group') return
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

  // Fetch the canonical datum list (Origin + reference planes + axes,
  // plus user-authored datums in Slice 3). Same poll cadence as the
  // hierarchy fetch — datums are tens-of-bytes per row, so polling is
  // cheap and we get visibility-flag sync for free.
  const fetchDatums = useCallback(async () => {
    try {
      const resp = await fetch(`${API_BASE}/api/datums`)
      if (resp.ok) {
        const data: DatumListResponse = await resp.json()
        if (Array.isArray(data.datums)) {
          setDatums(data.datums)
          return
        }
      }
    } catch {
      // Backend not running — leave datums as-is rather than clearing,
      // so the tree doesn't flicker on transient network errors.
    }
  }, [])

  useEffect(() => {
    if (wsStatus === 'connected') {
      // Fetch on connect — async to avoid synchronous setState in effect
      void Promise.resolve().then(fetchHierarchy)
      void Promise.resolve().then(fetchDatums)
    }
  }, [wsStatus, fetchHierarchy, fetchDatums])

  // Poll for hierarchy updates (pause when tab is hidden)
  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | null = null

    function startPolling() {
      stopPolling()
      timer = setInterval(() => {
        fetchHierarchy()
        fetchDatums()
      }, 5000)
    }

    function stopPolling() {
      if (timer) { clearInterval(timer); timer = null }
    }

    function handleVisibility() {
      if (document.visibilityState === 'visible') {
        fetchHierarchy()
        fetchDatums()
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
  }, [fetchHierarchy, fetchDatums])

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
  // Datums sit at the very top of the tree — they're the canonical
  // reference frame everything else is positioned against, so listing
  // them first matches the mental model "place primitives relative to
  // these references".
  const datumNodes = datumsToNodes(datums)
  const treeNodes = [...datumNodes, ...sketchNodes, ...objectNodes]

  const handleToggleVisibility = useCallback(
    (id: string) => {
      // Datum ids carry the `datum:` prefix; route them back to the
      // kernel via PATCH so the visibility flag persists across reloads
      // and is shared across collaborators.
      if (id.startsWith('datum:')) {
        const tail = id.slice('datum:'.length)
        if (tail === 'group') {
          // Toggling the group flips every default datum to the
          // inverse of the group's current "any visible" indicator.
          const anyVisible = datums.some((d) => d.visible)
          const next = !anyVisible
          for (const d of datums) {
            void fetch(`${API_BASE}/api/datums/${d.id}/visibility`, {
              method: 'PATCH',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ visible: next }),
            })
          }
          // Optimistic local update; the next poll will reconcile.
          setDatums((prev) => prev.map((d) => ({ ...d, visible: next })))
          return
        }
        const numericId = Number(tail)
        if (!Number.isFinite(numericId)) return
        const target = datums.find((d) => d.id === numericId)
        if (!target) return
        const next = !target.visible
        void fetch(`${API_BASE}/api/datums/${numericId}/visibility`, {
          method: 'PATCH',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ visible: next }),
        })
        setDatums((prev) =>
          prev.map((d) => (d.id === numericId ? { ...d, visible: next } : d)),
        )
        return
      }
      const obj = objects.get(id)
      if (obj) updateObject(id, { visible: !obj.visible })
    },
    [datums, objects, updateObject],
  )

  const handleToggleLock = useCallback(
    (id: string) => {
      const obj = objects.get(id)
      if (obj) updateObject(id, { locked: !obj.locked })
    },
    [objects, updateObject],
  )

  // Hover-revealed "+" affordance routes to the create-datum dialog when
  // it's the datum group; no-op otherwise (currently only the datum
  // group passes `onAdd`, but the type accepts any node so future
  // group rows can wire their own creation flows).
  const handleAdd = useCallback((node: TreeNode) => {
    if (node.id === 'datum:group') {
      setCreateDatumOpen(true)
    }
  }, [])

  return (
    <div className="flex flex-col h-full">
      {/* Header is the only element in the browser that carries panel
          chrome — border, rounded corners, drop shadow, and the
          background fill. The whole header is a button that toggles
          the tree's expanded state. When collapsed, only this chip
          remains visible. */}
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        aria-controls="browser-tree"
        title={expanded ? 'Collapse browser' : 'Expand browser'}
        className={cn(
          'cad-panel-header flex items-center gap-1.5 font-mono border border-border rounded shadow-md backdrop-blur-sm w-full text-left cursor-pointer',
          expanded
            ? 'bg-card/95 hover:bg-card'
            // Slightly darker shade of the panel background when
            // collapsed — uses the design-system `muted` token so the
            // chip stays visually grouped with the rest of the UI
            // while still standing out against the viewport.
            : 'bg-muted/95 hover:bg-muted',
        )}
      >
        <span className="flex-1">browser</span>
        <span aria-hidden="true" className="text-muted-foreground/70">
          {expanded ? '«' : '»'}
        </span>
      </button>
      {expanded && (
        <ScrollArea id="browser-tree" className="flex-1">
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
                  onAdd={node.id === 'datum:group' ? handleAdd : undefined}
                />
              ))}
            </div>
          )}
        </ScrollArea>
      )}
      {menu && (
        <TreeContextMenu
          menu={menu}
          datums={datums}
          onClose={closeMenu}
          onDatumsChanged={fetchDatums}
        />
      )}
      <CreateDatumDialog
        open={createDatumOpen}
        onOpenChange={setCreateDatumOpen}
        onCreated={fetchDatums}
      />
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
  datums,
  onClose,
  onDatumsChanged,
}: {
  menu: TreeContextMenuState
  /**
   * Snapshot of the datum list at click time. Used by the datum-node
   * branch so the menu knows whether the target is a default (locked)
   * datum and can resolve its current name for the rename prompt
   * default value.
   */
  datums: DatumDto[]
  onClose: () => void
  /**
   * Called after a successful PATCH/DELETE so the parent re-fetches
   * the datum list and the tree updates immediately.
   */
  onDatumsChanged: () => void
}) {
  const ref = useRef<HTMLDivElement>(null)
  const updateObject = useSceneStore((s) => s.updateObject)
  const editServerSketch = useSceneStore((s) => s.editServerSketch)
  const clearServerSketchId = useSceneStore((s) => s.clearServerSketchId)
  const isSketchNode = menu.node.type === 'sketch'
  const localObj = useSceneStore.getState().objects.get(menu.node.id)
  const isVisible = localObj?.visible ?? menu.node.visible ?? true

  // ─── Datum branch ───────────────────────────────────────────────────
  // Datum tree nodes use the `datum:<id>` prefix. The group row
  // (`datum:group`) has its own "+" affordance and isn't a meaningful
  // rename/delete target, so the menu is hidden for it. Individual
  // default datums (Origin / FrontPlane / TopPlane / RightPlane / X /
  // Y / Z axes) are locked at the kernel layer — `is_default = true`
  // returns 409 from PATCH/DELETE — so we surface them as disabled
  // entries rather than letting the user fire a request the backend
  // will refuse anyway.
  const isDatumNode = menu.node.id.startsWith('datum:')
  const isDatumGroupNode = menu.node.id === 'datum:group'
  const datumId = isDatumNode && !isDatumGroupNode
    ? Number(menu.node.id.slice('datum:'.length))
    : null
  const datumRecord =
    datumId !== null && Number.isFinite(datumId)
      ? datums.find((d) => d.id === datumId)
      : undefined
  const isDefaultDatum = !!datumRecord?.is_default

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

  const handleRenameDatum = useCallback(async () => {
    onClose()
    if (datumId === null || !datumRecord) return
    const next = window.prompt('Rename datum', datumRecord.name)?.trim()
    if (!next || next === datumRecord.name) return
    try {
      const resp = await fetch(`${API_BASE}/api/datums/${datumId}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: next }),
      })
      if (!resp.ok) {
        const text = await resp.text().catch(() => '')
        console.error('[browser] datum rename failed:', resp.status, text)
        return
      }
      onDatumsChanged()
    } catch (err) {
      console.error('[browser] datum rename error:', err)
    }
  }, [datumId, datumRecord, onClose, onDatumsChanged])

  const handleDeleteDatum = useCallback(async () => {
    onClose()
    if (datumId === null || !datumRecord) return

    // Two-pass delete:
    //   1. Try DELETE without cascade — server returns 409 if there
    //      are anchored solids (or if this datum is a default, but
    //      we already gate that via `deleteEnabled` above).
    //   2. On 409, ask the user to confirm a cascade detach. The
    //      response of pass 2 carries `detached_solids` so we can
    //      log the count for inspection.
    // Probing first keeps the no-dependents path single-confirm and
    // single-network, while still surfacing the cascade choice
    // explicitly when it actually matters.
    if (!window.confirm(`Delete datum "${datumRecord.name}"?`)) return

    const baseUrl = `${API_BASE}/api/datums/${datumId}`
    try {
      const probe = await fetch(baseUrl, { method: 'DELETE' })
      if (probe.ok) {
        onDatumsChanged()
        return
      }
      if (probe.status !== 409) {
        const text = await probe.text().catch(() => '')
        console.error('[browser] datum delete failed:', probe.status, text)
        return
      }
      // 409 → either default (impossible here, gated) or has dependents.
      const proceed = window.confirm(
        `"${datumRecord.name}" has anchored solids. Re-anchor them to Origin and delete?`,
      )
      if (!proceed) return
      const cascade = await fetch(`${baseUrl}?cascade=detach`, {
        method: 'DELETE',
      })
      if (!cascade.ok) {
        const text = await cascade.text().catch(() => '')
        console.error('[browser] datum cascade delete failed:', cascade.status, text)
        return
      }
      const result = await cascade.json().catch(() => null) as
        | { datum_id: number; detached_solids: number[] }
        | null
      if (result && result.detached_solids.length > 0) {
        console.info(
          `[browser] detached ${result.detached_solids.length} solid(s) from datum ${datumId}`,
        )
      }
      onDatumsChanged()
    } catch (err) {
      console.error('[browser] datum delete error:', err)
    }
  }, [datumId, datumRecord, onClose, onDatumsChanged])

  const handleDelete = isDatumNode
    ? handleDeleteDatum
    : isSketchNode
      ? handleDeleteSketch
      : handleDeleteObject
  const handleEdit = isDatumNode
    ? handleRenameDatum
    : isSketchNode
      ? handleEditSketch
      : handleRename
  // Datum nodes always have a server-side record so editing is
  // enabled iff the datum exists and isn't a default. Default datums
  // and the group row see disabled entries with a tooltip-style hint.
  const editEnabled = isDatumNode
    ? !!datumRecord && !isDefaultDatum
    : isSketchNode || !!localObj
  const deleteEnabled = isDatumNode
    ? !!datumRecord && !isDefaultDatum
    : true
  const visibilityEnabled = !isDatumNode && !isSketchNode && !!localObj
  const editLabel = isDatumNode
    ? 'Rename'
    : isSketchNode
      ? 'Edit sketch'
      : 'Rename'

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
      <TreeMenuItem onClick={handleDelete} danger disabled={!deleteEnabled}>
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
