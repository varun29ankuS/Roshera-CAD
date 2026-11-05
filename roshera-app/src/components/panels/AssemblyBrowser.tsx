/**
 * The model browser's PRODUCT TREE for imported assemblies:
 *
 *   ▾ ▤ arm.stp                       327  ✗20
 *     ▾ ▦ base                         42  ✓
 *         ◆ base_plate_upper               ✓
 *       ▸ ◆ DIN 988 - S6 x 12   ×59        ✓
 *
 * Built by `buildAssemblyTree` from each part's kernel `assembly` occurrence.
 *
 * Hover is shared with the viewport both ways: hovering a part row lights
 * the part (the same `hoveredId` the viewport sets), hovering a sub-assembly
 * or `×N` row outlines every part beneath it (`treeHoverIds`), and a part
 * hovered in the viewport highlights its row — or, while that row is folded
 * away, the nearest open row above it.
 *
 * Each row subscribes to its OWN hover / selection / visibility slice, so a
 * hover re-renders the one or two rows whose state flipped, not the tree
 * (the old browser re-rendered every row on every change). Folded rows do
 * not render their children at all.
 */
import { memo, useCallback, useMemo, useState } from 'react'
import { useSceneStore } from '@/stores/scene-store'
import type { AsmNode, ContainerNode, GroupNode, PartNode } from '@/lib/assembly-tree'
import { cn } from '@/lib/utils'

const INDENT_PX = 12

/** Open by default: files and their first level. Groups and deeper levels
 *  start folded so a 300-part assembly opens as a readable outline. */
function defaultOpen(node: AsmNode, depth: number): boolean {
  if (node.kind === 'file') return true
  if (node.kind === 'assembly') return depth <= 1
  return false
}

export interface AssemblyBrowserProps {
  roots: ContainerNode[]
  onPartContextMenu: (e: React.MouseEvent, id: string, name: string) => void
}

export function AssemblyBrowser({ roots, onPartContextMenu }: AssemblyBrowserProps) {
  // Fold state by stable node key; absent = the node's default.
  const [folds, setFolds] = useState<Map<string, boolean>>(() => new Map())
  const toggle = useCallback((key: string, open: boolean) => {
    setFolds((prev) => {
      const next = new Map(prev)
      next.set(key, open)
      return next
    })
  }, [])

  return (
    <div className="font-mono text-[13px] leading-snug" role="tree" aria-label="Assembly tree">
      {roots.map((root) => (
        <Branch
          key={root.key}
          node={root}
          depth={0}
          folds={folds}
          onToggle={toggle}
          onPartContextMenu={onPartContextMenu}
        />
      ))}
    </div>
  )
}

interface BranchProps {
  node: AsmNode
  depth: number
  /** The enclosing row's name (its repeat at the front of a child is dimmed). */
  parentName?: string
  folds: Map<string, boolean>
  onToggle: (key: string, open: boolean) => void
  onPartContextMenu: (e: React.MouseEvent, id: string, name: string) => void
}

function Branch({ node, depth, parentName, folds, onToggle, onPartContextMenu }: BranchProps) {
  if (node.kind === 'part' && node.missing !== undefined) {
    return <MissingRow node={node} depth={depth} parentName={parentName} />
  }
  if (node.kind === 'part') {
    return (
      <PartRow node={node} depth={depth} parentName={parentName} onContextMenu={onPartContextMenu} />
    )
  }
  const open = folds.get(node.key) ?? defaultOpen(node, depth)
  return (
    <div role="group">
      <ContainerRow node={node} depth={depth} parentName={parentName} open={open} onToggle={onToggle} />
      {open &&
        node.children.map((child) => (
          <Branch
            key={child.key}
            node={child}
            depth={depth + 1}
            parentName={node.kind === 'file' ? undefined : node.name}
            folds={folds}
            onToggle={onToggle}
            onPartContextMenu={onPartContextMenu}
          />
        ))}
    </div>
  )
}

// ─── Rows ─────────────────────────────────────────────────────────────

/**
 * A row's name with its parent's name, when repeated at the front, dimmed
 * and allowed to clip — the tail is what tells siblings apart. Exporters name
 * children after their assembly (`base_cover_nest_upper` under `base`); in a
 * 224 px rail the plain name truncated to `base_cover_ne…` and every sibling
 * read alike.
 */
function Name({ name, parent, strong }: { name: string; parent?: string; strong?: boolean }) {
  const lower = name.toLowerCase()
  const head = parent ? parent.toLowerCase() : ''
  const sep = head.length > 0 && lower.startsWith(head) ? name.charAt(head.length) : ''
  const repeats = sep === '_' || sep === ' ' || sep === '-' || sep === '.'
  if (!repeats || name.length <= head.length + 1) {
    return (
      <span className={cn('min-w-0 flex-1 truncate', strong && 'font-medium')} title={name}>
        {name}
      </span>
    )
  }
  return (
    <span className="flex min-w-0 flex-1 items-baseline overflow-hidden" title={name}>
      <span className="min-w-0 max-w-[40%] shrink truncate text-muted-foreground/50">
        {name.slice(0, head.length + 1)}
      </span>
      <span className={cn('min-w-0 truncate', strong && 'font-medium')}>
        {name.slice(head.length + 1)}
      </span>
    </span>
  )
}

const GLYPH: Record<AsmNode['kind'], string> = {
  file: '▤',
  assembly: '▦',
  group: '◆',
  part: '◆',
}

const ContainerRow = memo(function ContainerRow({
  node,
  depth,
  parentName,
  open,
  onToggle,
}: {
  node: ContainerNode | GroupNode
  depth: number
  parentName?: string
  open: boolean
  onToggle: (key: string, open: boolean) => void
}) {
  const ids = node.partIds
  const idSet = useMemo(() => new Set(ids), [ids])
  // Lit when a part beneath it is hovered in the viewport — the nearest
  // open row above a folded part answers "where is the thing I'm pointing at".
  const containsHover = useSceneStore((s) => s.hoveredId !== null && idSet.has(s.hoveredId))
  const anyVisible = useSceneStore((s) => ids.some((id) => s.objects.get(id)?.visible !== false))
  const setTreeHover = useSceneStore((s) => s.setTreeHover)
  const selectObjects = useSceneStore((s) => s.selectObjects)
  const updateObject = useSceneStore((s) => s.updateObject)

  const toggleVisible = (e: React.MouseEvent) => {
    e.stopPropagation()
    for (const id of ids) updateObject(id, { visible: !anyVisible })
  }

  return (
    <div
      role="treeitem"
      aria-expanded={open}
      className={cn(
        'group flex cursor-pointer select-none items-center gap-1 pr-1 transition-colors',
        containsHover
          ? 'bg-accent/60 text-foreground'
          : 'text-foreground/75 hover:bg-accent/50 hover:text-foreground',
      )}
      style={{ paddingLeft: depth * INDENT_PX }}
      onMouseEnter={() => setTreeHover(ids)}
      onMouseLeave={() => setTreeHover([])}
      onClick={(e) => selectObjects(ids, e.shiftKey || e.ctrlKey || e.metaKey)}
    >
      <button
        type="button"
        className="w-3 shrink-0 text-center text-foreground/60 hover:text-foreground"
        aria-label={open ? 'Collapse' : 'Expand'}
        onClick={(e) => {
          e.stopPropagation()
          onToggle(node.key, !open)
        }}
      >
        {open ? '▾' : '▸'}
      </button>
      <span className="shrink-0 text-muted-foreground/80">{GLYPH[node.kind]}</span>
      <Name name={node.name} parent={parentName} strong={node.kind === 'file'} />
      {node.kind === 'group' && (
        <span className="shrink-0 text-muted-foreground" title={`${node.count} uses of this part`}>
          ×{node.count}
        </span>
      )}
      {node.kind !== 'group' && (
        <span className="shrink-0 text-muted-foreground/55" title={`${node.partCount} parts`}>
          {node.partCount}
        </span>
      )}
      {node.missingCount > 0 && (
        <span
          className="shrink-0 text-destructive"
          title={`${node.missingCount} part use(s) beneath did not import — they are not in the viewport`}
        >
          {node.missingCount} missing
        </span>
      )}
      <Verdict sound={node.soundCount} unsound={node.unsoundCount} total={node.partCount} />
      <button
        type="button"
        onClick={toggleVisible}
        className={cn(
          'w-3 shrink-0 text-center opacity-0 transition-opacity group-hover:opacity-100',
          anyVisible ? 'text-foreground/60 hover:text-foreground' : 'text-orange-500 opacity-100',
        )}
        aria-label={anyVisible ? 'Hide all' : 'Show all'}
        title={anyVisible ? 'Hide all' : 'Show all'}
      >
        {anyVisible ? '●' : '○'}
      </button>
    </div>
  )
})

const PartRow = memo(function PartRow({
  node,
  depth,
  parentName,
  onContextMenu,
}: {
  node: PartNode
  depth: number
  parentName?: string
  onContextMenu: (e: React.MouseEvent, id: string, name: string) => void
}) {
  const { id } = node
  const hovered = useSceneStore((s) => s.hoveredId === id)
  const selected = useSceneStore((s) => s.selectedIds.has(id))
  const visible = useSceneStore((s) => s.objects.get(id)?.visible !== false)
  const setHovered = useSceneStore((s) => s.setHovered)
  const selectObject = useSceneStore((s) => s.selectObject)
  const updateObject = useSceneStore((s) => s.updateObject)

  return (
    <div
      role="treeitem"
      aria-selected={selected}
      className={cn(
        'group flex cursor-pointer select-none items-center gap-1 pr-1 transition-colors',
        selected
          ? 'text-foreground'
          : hovered
            ? 'bg-accent/60 text-foreground'
            : 'text-foreground/70 hover:bg-accent/50 hover:text-foreground',
      )}
      // Leaves sit one fold-arrow's width right of their parent's arrow. A
      // selected row wears the SAME yellow as the selected part in the
      // viewport (`--cad-edge-selected`): a wash plus a bar on the left edge,
      // so the pairing between row and part reads at a glance in both themes.
      style={{
        paddingLeft: depth * INDENT_PX + INDENT_PX,
        ...(selected
          ? {
              backgroundColor: 'color-mix(in oklch, var(--cad-edge-selected) 22%, transparent)',
              boxShadow: 'inset 3px 0 0 var(--cad-edge-selected)',
            }
          : {}),
      }}
      onMouseEnter={() => setHovered(id)}
      onMouseLeave={() => setHovered(null)}
      onClick={(e) => selectObject(id, e.shiftKey || e.ctrlKey || e.metaKey)}
      onContextMenu={(e) => onContextMenu(e, id, node.name)}
    >
      <span className="shrink-0 text-muted-foreground/80">{GLYPH.part}</span>
      <span className={cn('flex min-w-0 flex-1', !visible && 'opacity-50')}>
        <Name name={node.name} parent={parentName} />
      </span>
      <Verdict
        sound={node.sound === true ? 1 : 0}
        unsound={node.sound === false ? 1 : 0}
        total={1}
      />
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation()
          updateObject(id, { visible: !visible })
        }}
        className={cn(
          'w-3 shrink-0 text-center opacity-0 transition-opacity group-hover:opacity-100',
          visible ? 'text-foreground/60 hover:text-foreground' : 'text-orange-500 opacity-100',
        )}
        aria-label={visible ? 'Hide' : 'Show'}
        title={visible ? 'Hide' : 'Show'}
      >
        {visible ? '●' : '○'}
      </button>
    </div>
  )
})

/** A use whose geometry did not import: no mesh, so no hover / select /
 *  visibility — just the gap, where it sits, and why (tooltip). */
function MissingRow({
  node,
  depth,
  parentName,
}: {
  node: PartNode
  depth: number
  parentName?: string
}) {
  return (
    <div
      role="treeitem"
      aria-disabled
      className="flex select-none items-center gap-1 pr-1 text-destructive/80"
      style={{ paddingLeft: depth * INDENT_PX + INDENT_PX }}
      title={`Not imported: ${node.missing}`}
    >
      <span className="shrink-0">◇</span>
      <span className="flex min-w-0 flex-1 line-through decoration-destructive/50">
        <Name name={node.name} parent={parentName} />
      </span>
      <span className="shrink-0 text-[11px]">not imported</span>
    </div>
  )
}

/** ✓ when every part beneath is sound; an amber ⚠N when N are not; a dim ·
 *  when nothing beneath has been certified yet. The warning is deliberately
 *  a different SHAPE and colour from the `×N` use-count beside it — a red
 *  `✗7` read as "×7" (seven copies) or "7 missing", when it meant "7 parts
 *  rendered but not sound". */
function Verdict({ sound, unsound, total }: { sound: number; unsound: number; total: number }) {
  if (unsound > 0) {
    return (
      <span
        className="w-9 shrink-0 text-right text-amber-500"
        title={
          total > 1
            ? `${unsound} of ${total} parts not sound (they render, but fail the kernel's check — usually faces that did not import)`
            : 'not sound (renders, but fails the kernel check — usually faces that did not import)'
        }
      >
        ⚠{total > 1 ? unsound : ''}
      </span>
    )
  }
  if (sound === total) {
    return (
      <span className="w-9 shrink-0 text-right text-emerald-600 dark:text-emerald-400" title="sound">
        ✓
      </span>
    )
  }
  return (
    <span className="w-9 shrink-0 text-right text-muted-foreground/40" title="not certified yet">
      ·
    </span>
  )
}
