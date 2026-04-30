import { useState, useEffect, useCallback } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'

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
}: {
  node: TreeNode
  isLast: boolean
  ancestorIsLast: boolean[] // one entry per ancestor depth: true = ancestor was last sibling
  selectedIds: Set<string>
  onSelect: (id: string, additive: boolean) => void
  onToggleVisibility: (id: string) => void
  onToggleLock: (id: string) => void
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
            className="text-foreground/60 hover:text-foreground transition-colors w-3 text-center"
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

// ─── Build tree from local scene store (fallback) ───────────────────

function sceneToNodes(
  objects: Map<string, ReturnType<typeof useSceneStore.getState>['objects'] extends Map<string, infer V> ? V : never>,
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

function buildLocalNode(
  obj: { id: string; name: string; objectType: string; visible: boolean; locked: boolean; parentId?: string },
  allObjects: Map<string, typeof obj>,
): TreeNode {
  const children: TreeNode[] = []
  for (const [, child] of allObjects) {
    if (child.parentId === obj.id) {
      children.push(buildLocalNode(child, allObjects))
    }
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

export function ModelTree({ onCollapse }: { onCollapse?: () => void } = {}) {
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const selectObject = useSceneStore((s) => s.selectObject)
  const updateObject = useSceneStore((s) => s.updateObject)
  const wsStatus = useWSStore((s) => s.status)
  const sessionId = useWSStore((s) => s.sessionId)

  const [backendNodes, setBackendNodes] = useState<TreeNode[] | null>(null)

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
  // if available, falling back to the local scene store.
  const treeNodes = isPreviewMode()
    ? MOCK_TREE_NODES
    : (backendNodes ?? sceneToNodes(objects, objectOrder))

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
              />
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  )
}
