import { useState, useEffect, useCallback } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  ChevronRight,
  ChevronDown,
  Box,
  Circle,
  Cylinder,
  Triangle,
  Torus,
  Layers,
  Eye,
  EyeOff,
  Lock,
  Unlock,
  Component,
  FolderOpen,
  Grip,
  PenTool,
  ArrowUpFromLine,
  RefreshCcw,
  Disc,
  Hexagon,
  Grid3x3,
  CircleDot,
  type LucideIcon,
} from 'lucide-react'
import { cn } from '@/lib/utils'

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
  icon: LucideIcon
  children?: TreeNode[]
  visible?: boolean
  locked?: boolean
}

// ─── Icon map ───────────────────────────────────────────────────────

function iconForType(type: string): LucideIcon {
  switch (type.toLowerCase()) {
    case 'box': return Box
    case 'sphere': return Circle
    case 'cylinder': return Cylinder
    case 'cone': return Triangle
    case 'torus': return Torus
    case 'assembly': return Component
    case 'group': return Layers
    case 'sketch': return PenTool
    case 'extrude': return ArrowUpFromLine
    case 'revolve': return RefreshCcw
    case 'fillet': return Disc
    case 'chamfer': return Hexagon
    case 'pattern': return Grid3x3
    case 'hole': return CircleDot
    default: return Grip
  }
}

// ─── Tree node component ────────────────────────────────────────────

function TreeItem({
  node,
  depth,
  selectedIds,
  onSelect,
  onToggleVisibility,
  onToggleLock,
}: {
  node: TreeNode
  depth: number
  selectedIds: Set<string>
  onSelect: (id: string, additive: boolean) => void
  onToggleVisibility: (id: string) => void
  onToggleLock: (id: string) => void
}) {
  const [expanded, setExpanded] = useState(true)
  const isSelected = selectedIds.has(node.id)
  const hasChildren = node.children && node.children.length > 0
  const Icon = node.icon

  return (
    <div>
      <div
        className={cn(
          'flex items-center gap-1 px-1 py-0.5 cursor-pointer transition-colors group',
          isSelected
            ? 'bg-primary/15 text-primary'
            : 'text-foreground/70 hover:bg-accent/50 hover:text-foreground',
        )}
        style={{ paddingLeft: `${depth * 14 + 4}px` }}
        onClick={(e) => onSelect(node.id, e.shiftKey || e.ctrlKey || e.metaKey)}
      >
        {hasChildren ? (
          <button
            onClick={(e) => {
              e.stopPropagation()
              setExpanded(!expanded)
            }}
            className="w-3 h-3 flex items-center justify-center shrink-0"
          >
            {expanded ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}

        <Icon size={12} strokeWidth={1.5} className="shrink-0 text-muted-foreground" />

        <span className="text-[11px] truncate flex-1">{node.name}</span>

        <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <button
            onClick={(e) => {
              e.stopPropagation()
              onToggleVisibility(node.id)
            }}
            className="p-0.5 rounded hover:bg-accent"
          >
            {node.visible !== false ? (
              <Eye size={10} className="text-muted-foreground" />
            ) : (
              <EyeOff size={10} className="text-muted-foreground/40" />
            )}
          </button>
          <button
            onClick={(e) => {
              e.stopPropagation()
              onToggleLock(node.id)
            }}
            className="p-0.5 rounded hover:bg-accent"
          >
            {node.locked ? (
              <Lock size={10} className="text-muted-foreground" />
            ) : (
              <Unlock size={10} className="text-muted-foreground/40" />
            )}
          </button>
        </div>
      </div>

      {hasChildren && expanded && (
        <div>
          {node.children!.map((child) => (
            <TreeItem
              key={child.id}
              node={child}
              depth={depth + 1}
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

function hierarchyToNodes(
  hierarchy: ProjectHierarchy,
): TreeNode[] {
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
        icon: iconForType(f.feature_type),
      }))
      return {
        id: inst.instance_id,
        name,
        type: 'part',
        icon: Grip,
        children: children && children.length > 0 ? children : undefined,
      }
    } else {
      const asm = node.SubAssembly
      return {
        id: asm.id,
        name: asm.name,
        type: 'assembly',
        icon: Component,
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
    icon: iconForType(obj.objectType),
    visible: obj.visible,
    locked: obj.locked,
    children: children.length > 0 ? children : undefined,
  }
}

// ─── Main panel ─────────────────────────────────────────────────────

export function ModelTree() {
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

  // Use backend hierarchy if available, otherwise fall back to local scene
  const treeNodes = backendNodes ?? sceneToNodes(objects, objectOrder)

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
      <div className="px-3 py-1.5 text-xs font-medium uppercase tracking-wider text-muted-foreground border-b border-white/5 flex items-center gap-1.5">
        <FolderOpen size={11} className="text-primary" />
        Model Tree
      </div>
      <ScrollArea className="flex-1">
        {treeNodes.length === 0 ? (
          <div className="p-3 text-[11px] text-muted-foreground/60 text-center">
            No objects in scene
          </div>
        ) : (
          <div className="py-1">
            <div className="flex items-center gap-1 px-2 py-1 text-[11px] text-muted-foreground/60">
              <Component size={11} />
              <span className="uppercase tracking-wider text-[9px] font-medium">Assembly</span>
            </div>
            {treeNodes.map((node) => (
              <TreeItem
                key={node.id}
                node={node}
                depth={1}
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
