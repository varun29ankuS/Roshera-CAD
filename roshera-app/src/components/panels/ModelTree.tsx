import { useState, useEffect, useCallback, useMemo } from 'react'
import { Eye, EyeOff, Trash2, Pencil, Plus, FileText } from 'lucide-react'
import {
  ContextMenu,
  ContextMenuHeader,
  ContextMenuItem,
  ContextMenuSeparator,
} from '@/components/ui/context-menu'
import { useSceneStore, isStandardPlane, type CADObject, type SketchPlane } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { useWSStore } from '@/stores/ws-store'
import { useDocumentStore } from '@/stores/document-store'
import { sketchApi, type ServerSketchSession } from '@/lib/sketch-api'
import { createPartDrawing } from '@/lib/drawings-api'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { CreateDatumDialog } from '@/components/panels/CreateDatumDialog'
import { FeatureTreeBody } from '@/components/panels/FeatureTree'

const API_BASE = import.meta.env.VITE_API_URL || ''

/**
 * Browser viewing modes.
 *
 * - `browser` → assembly hierarchy + datums + sketches, sourced from
 *   `GET /api/hierarchy/{session_id}` and `GET /api/datums`.
 * - `features` → timeline-derived operation graph, rendered by
 *   `FeatureTreeBody` over `GET /api/feature-tree/{branch_id}`.
 *
 * Both modes are pure renderers over backend responses — the kernel
 * is the authoritative source of every node, parent edge, and
 * visibility flag. Mode is a UI-local toggle only.
 */
type BrowserMode = 'browser' | 'features'

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

// ─── Name rendering: protect the characters that actually identify a row ───

/**
 * Longest prefix `name` shares with any sibling, backed off to the last `_`
 * boundary so the remainder is a whole token.
 *
 * The backoff is the load-bearing part. A raw longest-shared-prefix returns
 * the ENTIRE name whenever one name is a proper prefix of a sibling —
 * `valve_pocket_cutter_L` against `valve_pocket_cutter_L2` shares all 21
 * characters — which would leave nothing to protect and render the row
 * exactly as before, still indistinguishable from `..._R`. Backing off to the
 * boundary yields tails of `L` / `L2` / `R` / `R2`, which is the whole point.
 *
 * Returns '' when the name shares nothing worth compressing.
 */
function sharedPrefix(name: string, siblings: readonly string[]): string {
  let longest = 0
  for (const other of siblings) {
    if (other === name) continue
    const stop = Math.min(name.length, other.length)
    let i = 0
    while (i < stop && name[i] === other[i]) i++
    if (i > longest) longest = i
  }
  if (longest === 0) return ''
  // Back off to the last separator at or before the shared run.
  const cut = name.lastIndexOf('_', longest - 1)
  return cut <= 0 ? '' : name.slice(0, cut + 1)
}

// Under this many characters a split costs more attention than it saves.
const MIN_COMPRESSIBLE_PREFIX = 4

/**
 * A tree row's name, with the shared head dimmed and clipped and the
 * discriminating tail protected from truncation.
 *
 * `text-overflow: ellipsis` is tail-biased, but in generated CAD names the
 * entropy lives in the tail — six parts whose names differ only in `_v2` /
 * `_L2` / `_R2` collapsed into two visible strings, because the renderer spent
 * its 145px preserving the redundant head and deleting the variant marker.
 * Flex does the budgeting: the head is `truncate min-w-0` and absorbs the cut,
 * the tail is `shrink-0` and never clips. No measurement, no font metrics, no
 * knowledge of the rail's width.
 */
function NameCell({ name, siblings }: { name: string; siblings: readonly string[] }) {
  const prefix = sharedPrefix(name, siblings)
  const rest = name.slice(prefix.length)

  if (prefix.length < MIN_COMPRESSIBLE_PREFIX) {
    return (
      <span className="truncate flex-1" title={name}>
        {name}
      </span>
    )
  }

  return (
    <span className="flex min-w-0 flex-1 items-baseline overflow-hidden" title={name}>
      <span className="truncate min-w-0 text-muted-foreground/60">{prefix}</span>
      <span className="shrink-0 whitespace-nowrap">{rest}</span>
    </span>
  )
}

// ─── Tree row (terminal lineage style) ──────────────────────────────

function TreeItem({
  node,
  siblings,
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
  // Names of every node at this level, this one included — the comparison set
  // that decides which characters are redundant.
  siblings: readonly string[]
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
  // Datums are furniture: seven rows of Origin/planes/axes ahead of any
  // content the reader came for. They collapse by default and expand on
  // click; every other row keeps its previous behaviour.
  const [expanded, setExpanded] = useState(node.id !== 'datum:group')
  const isSelected = selectedIds.has(node.id)
  const hasChildren = !!node.children && node.children.length > 0

  // Build lineage prefix: │ for ancestors with more siblings, spaces otherwise.
  const lineagePrefix = ancestorIsLast.map((last) => (last ? ' ' : '│  ')).join('')

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

        {/* Name — shared head dimmed and clipped, identifying tail protected */}
        <NameCell name={node.name} siblings={siblings} />

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
              siblings={node.children!.map((c) => c.name)}
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

function sketchesToNodes(
  sketches: Map<string, ServerSketchSession>,
  hiddenSketchIds: Set<string>,
): TreeNode[] {
  return Array.from(sketches.values())
    .sort((a, b) => a.created_at - b.created_at)
    .map((s, idx) => {
      const planeLabel = sketchPlaneLabel(s.plane)
      // Defensive: a session deserialised from an older format (or a
      // partial WS frame the bridge let through) may be missing
      // `shapes` entirely. Reading `.reduce` on `undefined` here used
      // to throw and tear down the entire ModelTree render — same
      // failure mode the `sketchPlaneLabel` defense above guards
      // against. Treat a missing/invalid shapes array as zero shapes.
      const shapes = Array.isArray(s.shapes) ? s.shapes : []
      const totalPoints = shapes.reduce(
        (sum, shape) =>
          sum + (Array.isArray(shape?.points) ? shape.points.length : 0),
        0,
      )
      const ptSuffix = totalPoints === 1 ? 'pt' : 'pts'
      const shapeSuffix =
        shapes.length > 1 ? ` · ${shapes.length} shapes` : ''
      return {
        id: s.id,
        name: `Sketch ${idx + 1} (${planeLabel} · ${totalPoints} ${ptSuffix}${shapeSuffix})`,
        type: 'sketch',
        symbol: symbolForType('sketch'),
        visible: !hiddenSketchIds.has(s.id),
        locked: false,
      }
    })
}

/**
 * Index objects by the sketch id they were produced from. Each
 * extrude / extrude_cut result carries
 * `analyticalGeometry.params.sketch_id` (set by `extrude_sketch` in
 * api-server/src/sketch.rs); we read it back so the tree can nest the
 * source sketch *under* the resulting solid — the convention every
 * mainstream CAD package uses and the explicit design intent recorded
 * on `ExtrudeSketchBody.consume` (default `false` precisely so the
 * sketch persists for re-edit).
 *
 * Returns a `sketchId -> ownerObjectId` map. Sketches that don't
 * appear in this map are still in the "drawing" phase and render at
 * root.
 */
function indexSketchOwners(objects: Map<string, CADObject>): Map<string, string> {
  const owners = new Map<string, string>()
  for (const obj of objects.values()) {
    const params = obj.analyticalGeometry?.params as
      | Record<string, unknown>
      | undefined
    const sketchId = params?.['sketch_id']
    if (typeof sketchId === 'string' && sketchId.length > 0) {
      // First writer wins — if a sketch was extruded, then later
      // (re-)extruded into a second body, the original producer keeps
      // the lineage badge so the tree shape stays stable across edits.
      if (!owners.has(sketchId)) {
        owners.set(sketchId, obj.id)
      }
    }
  }
  return owners
}

/**
 * Walk a forest and graft owned sketches into the matching solid's
 * children list. Pure function — returns a new forest, leaves the
 * input nodes untouched so React's referential-equality reconciliation
 * does the right thing on the unaffected branches.
 */
function nestSketchesUnderOwners(
  nodes: TreeNode[],
  sketchesByOwnerId: Map<string, TreeNode[]>,
): TreeNode[] {
  return nodes.map((node) => {
    const nestedChildren = node.children
      ? nestSketchesUnderOwners(node.children, sketchesByOwnerId)
      : []
    const owned = sketchesByOwnerId.get(node.id) ?? []
    const merged = [...nestedChildren, ...owned]
    if (merged.length === 0) {
      // Preserve the original `children: undefined` shape so the
      // expand arm renders as a leaf (`─`) instead of an empty
      // collapsible.
      return node
    }
    return { ...node, children: merged }
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
 * Map a single backend `CADObject` to a renderable tree node.
 *
 * Frontend does no lineage synthesis here — `parentId` (sourced from
 * the backend `parent` field via the WS bridge) is the only signal
 * that wires children to parents. Cross-feature lineage (which
 * sketch produced which extrude, which body each modifier touches)
 * is the Features mode's job; it queries the kernel feature tree
 * directly so the two modes never diverge.
 */
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


// ─── Solid classification (what KIND of thing is this row?) ─────────
//
// The rail listed every solid with an identical bullet, so a part, a
// cutter body and the result of a boolean were indistinguishable at a
// glance — the founder's complaint, twice: "i cannot make out with one
// look which is the part, which is an assembly, which is an operation
// within a part".
//
// Two facts the kernel now reports per solid answer it without any
// hierarchy: `named` (was the name CHOSEN, or generated) and `role`
// (`derived` means it came out of a boolean, which consumes its
// operands — so a live `primitive` was combined into nothing).

export type SolidClass = 'part' | 'result' | 'unnamed'

interface AgentPart {
  id: number
  name: string
  named: boolean
  role: 'primitive' | 'derived'
  anchor_datum_id: number
  anchor_datum_name: string
  location_oneliner: string
}

interface Section {
  key: SolidClass
  label: string
  nodes: TreeNode[]
  defaultOpen: boolean
}

/** `GET /api/agent/parts`, keyed by kernel solid id.
 *
 *  Starts empty and STAYS empty on failure: a dead endpoint must degrade
 *  every row to `unnamed`, never blank the tree. */
function useAgentParts(): Map<number, AgentPart> {
  const [parts, setParts] = useState<Map<number, AgentPart>>(() => new Map())
  useEffect(() => {
    const controller = new AbortController()
    fetch(`${API_BASE}/api/agent/parts`, { signal: controller.signal })
      .then((res) => {
        if (!res.ok) throw new Error(`GET /api/agent/parts -> ${res.status}`)
        return res.json() as Promise<AgentPart[]>
      })
      .then((rows) => {
        const next = new Map<number, AgentPart>()
        for (const row of rows) next.set(row.id, row)
        setParts(next)
      })
      .catch(() => {
        /* aborted or failed — keep the empty map and keep rendering */
      })
    return () => controller.abort()
  }, [])
  return parts
}

/** Names the SERVER invented before it stopped doing so.
 *
 *  `create_cylinder`/`box`/`cone` used to persist `format!("Cylinder {id}")`
 *  when the caller supplied no name, which made an invented name
 *  indistinguishable from a chosen one. That is fixed at the source, but the
 *  fix is NOT retroactive: solids created before it still carry the invented
 *  string and report `named: true`, so they file under PARTS and crowd out the
 *  bodies somebody actually named.
 *
 *  DELETE THIS once those rows are migrated. It is a display-level stopgap for
 *  a known, finite, shrinking set — not a naming convention, and nothing else
 *  should ever depend on the shape of a name. */
const SERVER_INVENTED_NAME = /^(Cylinder|Box|Sphere|Cone) \d+$/

/** An UNJOINED solid classifies as `unnamed`, never as `part`. A row we
 *  could not measure must not be promoted to the section that means
 * "this is a finished thing". */
function classifySolid(part: AgentPart | undefined): SolidClass {
  if (part === undefined) return 'unnamed'
  const chosen = part.named && !SERVER_INVENTED_NAME.test(part.name)
  if (chosen && part.role === 'primitive') return 'part'
  if (part.role === 'derived') return 'result'
  return 'unnamed'
}

/** Shape carries ROLE, fill carries PROVENANCE, so the glyph column is
 *  the answer column when scanning straight down the rail. */
function symbolForClass(cls: SolidClass, named: boolean): string {
  if (cls === 'part') return '◆'
  if (cls === 'result') return named ? '◈' : '◇'
  return '◌'
}

function groupIntoSections(
  nodes: TreeNode[],
  parts: Map<number, AgentPart>,
  solidIdOf: (n: TreeNode) => number | undefined,
): Section[] {
  const buckets: Record<SolidClass, TreeNode[]> = { part: [], result: [], unnamed: [] }
  for (const node of nodes) {
    const solidId = solidIdOf(node)
    const part = solidId === undefined ? undefined : parts.get(solidId)
    const cls = classifySolid(part)
    const chosen = part !== undefined && part.named && !SERVER_INVENTED_NAME.test(part.name)
    buckets[cls].push({ ...node, symbol: symbolForClass(cls, chosen) })
  }

  // A missing id sinks to the END of a descending sort: "newest" is
  // unknowable for a solid that was never measured, so it must not
  // outrank one that was.
  const rankId = (n: TreeNode): number => solidIdOf(n) ?? Number.NEGATIVE_INFINITY
  const byName = (a: TreeNode, b: TreeNode): number =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase())
  const isNamed = (n: TreeNode): boolean => {
    const id = solidIdOf(n)
    const p = id === undefined ? undefined : parts.get(id)
    return p !== undefined && p.named && !SERVER_INVENTED_NAME.test(p.name)
  }

  const sections: Section[] = []
  if (buckets.part.length > 0) {
    // Alphabetical: you are looking for a body you already know the name of.
    buckets.part.sort(byName)
    sections.push({ key: 'part', label: 'PARTS', nodes: buckets.part, defaultOpen: true })
  }
  if (buckets.result.length > 0) {
    buckets.result.sort((a, b) => {
      const na = isNamed(a)
      const nb = isNamed(b)
      if (na !== nb) return na ? -1 : 1
      if (na && nb) return byName(a, b)
      return rankId(b) - rankId(a)
    })
    sections.push({ key: 'result', label: 'RESULTS', nodes: buckets.result, defaultOpen: true })
  }
  if (buckets.unnamed.length > 0) {
    // Newest first: the question this section answers is "what did I just
    // make that never got a name?"
    buckets.unnamed.sort((a, b) => rankId(b) - rankId(a))
    sections.push({ key: 'unnamed', label: 'UNNAMED', nodes: buckets.unnamed, defaultOpen: false })
  }
  return sections
}

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
  const hiddenSketchIds = useSceneStore((s) => s.hiddenSketchIds)
  const toggleSketchVisibility = useSceneStore((s) => s.toggleSketchVisibility)
  const wsStatus = useWSStore((s) => s.status)
  const sessionId = useWSStore((s) => s.sessionId)

  // `mode` flips the body between the assembly tree and the
  // timeline-derived operation graph. Header chip remains a single
  // "browser" anchor; the segmented control inside it swaps modes.
  const [mode, setMode] = useState<BrowserMode>('browser')
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
      } else {
        console.error(`[ModelTree] GET /api/hierarchy/${sid} failed: ${resp.status}`)
      }
    } catch (err) {
      // Backend not running — fall back to local
      console.error('[ModelTree] hierarchy fetch threw:', err)
    }
    setBackendNodes(null)
  }, [sessionId])

  // Fetch the canonical datum list (Origin + reference planes + axes,
  // plus user-authored datums in Slice 3). Same poll cadence as the
  // hierarchy fetch — datums are tens-of-bytes per row, so polling is
  // cheap and we get visibility-flag sync for free.
  //
  // Logged, not surfaced in the UI: the viewport's `Datums.tsx` polls the
  // same endpoint and owns the user-facing failure report (a Blackboard
  // system line), so this tree doesn't double up on the same root cause.
  const fetchDatums = useCallback(async () => {
    try {
      const resp = await fetch(`${API_BASE}/api/datums`)
      if (resp.ok) {
        const data: DatumListResponse = await resp.json()
        if (Array.isArray(data.datums)) {
          setDatums(data.datums)
          return
        }
      } else {
        console.error(`[ModelTree] GET /api/datums failed: ${resp.status}`)
      }
    } catch (err) {
      // Backend not running — leave datums as-is rather than clearing,
      // so the tree doesn't flicker on transient network errors.
      console.error('[ModelTree] datums fetch threw:', err)
    }
  }, [])

  // `documentEpoch` bumps AFTER an in-place document switch is confirmed
  // (`stores/document-store.ts`) — treated exactly like a fresh WS connect:
  // the hierarchy/datums this panel shows belong to whichever document is
  // now active, and waiting out the 5s poll below would leave the previous
  // document's parts on screen for up to 5 seconds after switching.
  const documentEpoch = useDocumentStore((s) => s.epoch)
  useEffect(() => {
    if (wsStatus === 'connected') {
      // Fetch on connect — async to avoid synchronous setState in effect
      void Promise.resolve().then(fetchHierarchy)
      void Promise.resolve().then(fetchDatums)
    }
  }, [wsStatus, fetchHierarchy, fetchDatums, documentEpoch])

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

  // Tree composition order (top → bottom):
  //   1. Datums  — canonical reference frame, listed first so users
  //      see "the world's axes" before any feature placed in it.
  //   2. Bodies — backend assembly hierarchy when populated;
  //      otherwise a flat scene-store mirror of `ObjectCreated`
  //      broadcasts. Each body that was produced from a sketch
  //      (link via `analyticalGeometry.params.sketch_id`) has that
  //      source sketch grafted as a child — mainstream CAD
  //      convention, and the explicit reason
  //      `ExtrudeSketchBody.consume` defaults to `false` server-side.
  //   3. Standalone sketches — sessions that haven't been extruded
  //      yet. Once extrusion runs they migrate under the resulting
  //      body automatically on the next render.
  const localNodes = sceneToNodes(objects, objectOrder)
  const allSketchNodes = sketchesToNodes(serverSketches, hiddenSketchIds)
  const baseObjectNodes =
    backendNodes && backendNodes.length > 0 ? backendNodes : localNodes
  const datumNodes = datumsToNodes(datums)

  // Split sketches into "owned" (parent body exists in the scene) and
  // "standalone" (still in the drawing / pre-extrude phase). The
  // ownership index is built from the canonical scene-store object
  // map regardless of which object source feeds the tree, because the
  // `analyticalGeometry.params.sketch_id` link lives on every object
  // broadcast and is the single source of lineage truth.
  const sketchOwners = indexSketchOwners(objects)
  const sketchesByOwnerId = new Map<string, TreeNode[]>()
  const standaloneSketchNodes: TreeNode[] = []
  for (const sketchNode of allSketchNodes) {
    const ownerId = sketchOwners.get(sketchNode.id)
    if (ownerId !== undefined) {
      const existing = sketchesByOwnerId.get(ownerId)
      if (existing) {
        existing.push(sketchNode)
      } else {
        sketchesByOwnerId.set(ownerId, [sketchNode])
      }
    } else {
      standaloneSketchNodes.push(sketchNode)
    }
  }
  const objectNodes = nestSketchesUnderOwners(baseObjectNodes, sketchesByOwnerId)
  const treeNodes = [...datumNodes, ...objectNodes, ...standaloneSketchNodes]

  // Sections over the OBJECT rows only. Datums and standalone sketches keep
  // their own places; they are already distinguishable and re-sorting them
  // would move furniture the reader navigates by.
  const agentParts = useAgentParts()
  const solidIdOf = useCallback(
    (n: TreeNode): number | undefined => objects.get(n.id)?.analyticalGeometry?.solidId,
    [objects],
  )
  const sections = useMemo(
    () => groupIntoSections(objectNodes, agentParts, solidIdOf),
    // objectNodes is rebuilt every render; key the memo on what it is derived
    // from instead, so this does not recompute on unrelated state changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [objects, objectOrder, backendNodes, agentParts, solidIdOf],
  )
  const [closedSections, setClosedSections] = useState<Set<SolidClass>>(
    () => new Set<SolidClass>(['unnamed']),
  )
  const toggleSection = useCallback((key: SolidClass) => {
    setClosedSections((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }, [])

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
            }).then((resp) => {
              if (!resp.ok) {
                console.error(
                  `[ModelTree] PATCH datum ${d.id} visibility failed: ${resp.status}`,
                )
              }
            }).catch((err) => console.error('[ModelTree] datum visibility PATCH threw:', err))
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
        }).then((resp) => {
          if (!resp.ok) {
            console.error(
              `[ModelTree] PATCH datum ${numericId} visibility failed: ${resp.status}`,
            )
          }
        }).catch((err) => console.error('[ModelTree] datum visibility PATCH threw:', err))
        setDatums((prev) =>
          prev.map((d) => (d.id === numericId ? { ...d, visible: next } : d)),
        )
        return
      }
      const obj = objects.get(id)
      if (obj) {
        updateObject(id, { visible: !obj.visible })
        return
      }
      // A committed sketch is a first-class, hideable entity too.
      if (serverSketches.has(id)) toggleSketchVisibility(id)
    },
    [datums, objects, updateObject, serverSketches, toggleSketchVisibility],
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
    <div className="flex flex-col h-full min-h-0">
      {/* Consolidated header chip.
          ─ Outer area is the expand/collapse toggle (one click target
            mirrors the previous single-mode behaviour).
          ─ Inline segmented control flips body mode between the
            assembly browser and the timeline-derived feature tree.
            Mode buttons stop propagation so clicking a tab does not
            also collapse the panel.
          When collapsed, only the chip remains visible — the mode
          control is hidden because there's no body for it to drive. */}
      <div
        // No border, no radius, no shadow: this is a docked column's header
        // now, not a card floating on the canvas. A shadow is for something
        // ON TOP of the model; the rail is beside it.
        // The `cad-panel-header` TOKEN is deliberately not used here, and a
        // `px-2` utility beside it does not work: the token sets padding via
        // `@apply px-3` inside a component layer, which wins, so the override
        // reads as applied and measures as 12px. Silently losing to a token is
        // worse than not trying — the type is copied out, the padding is this
        // header's own.
        //
        // The density differs for a reason: a panel header spans a panel, this
        // one spans a 224px rail carrying a label, a two-tab control and a
        // chevron, and the token's padding pushed that row past its own edge.
        className={cn(
          'flex w-full items-center gap-1.5 border-b border-border px-2 py-1.5',
          'font-mono text-[11px] font-medium uppercase tracking-[0.1em] text-muted-foreground',
          expanded ? 'bg-transparent' : 'bg-transparent',
        )}
      >
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={expanded}
          aria-controls="browser-tree"
          title={expanded ? 'Collapse browser' : 'Expand browser'}
          className={cn(
            'cursor-pointer text-left transition-colors hover:text-foreground',
            // Collapsed, the rail is a 40px strip and the word does not fit —
            // it rendered as "brow", which a tooltip technically admits and no
            // reader forgives. The chevron carries the affordance at that
            // width; the label returns with the room to hold it.
            expanded ? 'flex-1' : 'sr-only',
          )}
        >
          browser
        </button>
        {!expanded && (
          <span aria-hidden className="flex-1 text-center text-muted-foreground">
            ⌸
          </span>
        )}
        {expanded && (
          <div
            role="tablist"
            aria-label="Browser mode"
            className="flex items-center gap-0 text-[11px] leading-none"
          >
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'browser'}
              onClick={(e) => {
                e.stopPropagation()
                setMode('browser')
              }}
              className={cn(
                'px-1.5 py-0.5 rounded-sm transition-colors',
                mode === 'browser'
                  ? 'bg-accent text-foreground'
                  : 'text-muted-foreground hover:text-foreground',
              )}
            >
              parts
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={mode === 'features'}
              onClick={(e) => {
                e.stopPropagation()
                setMode('features')
              }}
              className={cn(
                'px-1.5 py-0.5 rounded-sm transition-colors',
                mode === 'features'
                  ? 'bg-accent text-foreground'
                  : 'text-muted-foreground hover:text-foreground',
              )}
            >
              features
            </button>
          </div>
        )}
        <button
          type="button"
          onClick={onToggle}
          aria-label={expanded ? 'Collapse browser' : 'Expand browser'}
          className="text-muted-foreground/70 hover:text-foreground transition-colors px-1"
        >
          {expanded ? '«' : '»'}
        </button>
      </div>
      {expanded && mode === 'browser' && (
        <ScrollArea id="browser-tree" className="flex-1 min-h-0">
          {treeNodes.length === 0 ? (
            <div className="p-3 text-[12px] text-muted-foreground/60 text-center font-mono">
              ∅ empty document — add a primitive from the toolbar,
              sketch on a plane, or drop a STEP file
            </div>
          ) : (
            <div className="py-1 px-1">
              {[
                { key: null as SolidClass | null, label: null, nodes: datumNodes },
                ...sections.map((s: Section) => ({ key: s.key as SolidClass | null, label: s.label as string | null, nodes: s.nodes })),
                { key: null as SolidClass | null, label: null, nodes: standaloneSketchNodes },
              ].map((group) => {
                if (group.nodes.length === 0) return null
                const collapsed = group.key !== null && closedSections.has(group.key)
                return (
                  <div key={group.label ?? `plain-${group.nodes[0]?.id ?? 'x'}`}>
                    {group.label !== null && group.key !== null && (
                      <button
                        type="button"
                        onClick={() => toggleSection(group.key as SolidClass)}
                        className="w-full flex items-center gap-1.5 px-1 pt-2 pb-0.5 text-[11px] font-mono uppercase tracking-[0.14em] text-muted-foreground/55 hover:text-muted-foreground transition-colors"
                        aria-expanded={!collapsed}
                      >
                        <span className="w-2 text-[11px]">{collapsed ? '▸' : '▾'}</span>
                        <span>{group.label}</span>
                        <span className="text-muted-foreground/35">{group.nodes.length}</span>
                      </button>
                    )}
                    {!collapsed &&
                      group.nodes.map((node: TreeNode, idx: number) => (
                        <TreeItem
                          key={node.id}
                          node={node}
                          siblings={group.nodes.map((n: TreeNode) => n.name)}
                          isLast={idx === group.nodes.length - 1}
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
                )
              })}
            </div>
          )}
        </ScrollArea>
      )}
      {expanded && mode === 'features' && (
        <ScrollArea id="browser-tree" className="flex-1 min-h-0">
          <FeatureTreeBody />
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

  const handleRename = useCallback(() => {
    onClose()
    if (!localObj) return
    const next = window.prompt('Rename', localObj.name)?.trim()
    if (!next || next === localObj.name) return
    // Optimistic local update; the backend write is what makes the rename
    // DURABLE — the kernel solid's name is the single source every reload
    // (scene snapshot), the timeline lanes, and the agent surface derive
    // from. Without it a refresh reverted the rename to solid_N.
    const previous = localObj.name
    updateObject(localObj.id, { name: next })
    void (async () => {
      // Roll the optimistic update back on a failed persist — otherwise the
      // tree shows the new name while the kernel (the source every reload
      // derives from) kept the old one, and the next snapshot silently
      // reverts it with no explanation.
      try {
        const resp = await fetch(
          `/api/parts/uuid/${encodeURIComponent(localObj.id)}/name`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ name: next }),
          },
        )
        if (!resp.ok) {
          console.error('[browser] rename persist failed:', resp.status)
          updateObject(localObj.id, { name: previous })
          window.alert(`Rename failed (${resp.status}) — reverted to "${previous}".`)
        }
      } catch (err) {
        console.error('[browser] rename persist error:', err)
        updateObject(localObj.id, { name: previous })
        window.alert(`Rename failed (backend unreachable) — reverted to "${previous}".`)
      }
    })()
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

  const handleCreateDrawing = useCallback(async () => {
    onClose()
    if (!localObj) return
    try {
      // Server builds the standard third-angle sheet (Front/Top/Right +
      // HLR + centerlines + auto dimensions), registers it, and returns
      // its id; we then open the Drawing workspace focused on it.
      const drawingId = await createPartDrawing(localObj.id, localObj.name)
      useDocModeStore.getState().openDrawing(drawingId)
    } catch (err) {
      console.error('[browser] create-drawing failed:', err)
    }
  }, [localObj, onClose])

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

  // The shared primitive portals to `document.body`, which this menu
  // needs specifically: the browser panel uses `backdrop-blur-sm`
  // (a containing block for fixed descendants) AND `overflow-hidden`,
  // so a `position: fixed` menu rendered inline would be clipped to
  // the panel's width and invisible for most clicks.
  return (
    <ContextMenu x={menu.x} y={menu.y} onClose={onClose} aria-label="Tree node actions">
      <ContextMenuHeader>{menu.node.name}</ContextMenuHeader>
      <ContextMenuSeparator />
      <ContextMenuItem
        onClick={() => void handleEdit()}
        disabled={!editEnabled}
        title={!editEnabled && isDefaultDatum ? 'Default datums cannot be renamed' : undefined}
      >
        <Pencil size={13} />
        {editLabel}
      </ContextMenuItem>
      <ContextMenuItem onClick={handleToggleVisibility} disabled={!visibilityEnabled}>
        {isVisible ? <EyeOff size={13} /> : <Eye size={13} />}
        {isVisible ? 'Hide' : 'Show'}
      </ContextMenuItem>
      {visibilityEnabled && (
        <ContextMenuItem onClick={() => void handleCreateDrawing()}>
          <FileText size={13} />
          Create Drawing
        </ContextMenuItem>
      )}
      <ContextMenuSeparator />
      <ContextMenuItem
        onClick={() => void handleDelete()}
        danger
        disabled={!deleteEnabled}
        title={!deleteEnabled && isDefaultDatum ? 'Default datums cannot be deleted' : undefined}
      >
        <Trash2 size={13} />
        Delete
      </ContextMenuItem>
    </ContextMenu>
  )
}
