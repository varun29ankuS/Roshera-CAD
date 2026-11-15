/**
 * The chapter list: the part's history read like a CAD feature tree -- one
 * row per engineering feature, in build order, each with the icon of the tool
 * that made it, its name, and the kernel's verdict (✓ sound / ✗ with the
 * kernel's own reason). A pattern opens to its instances. Rows are chapters
 * grouped by the kernel from its recorded steps (GET /api/timeline/chapters).
 */
import { useEffect, useMemo, useState } from 'react'
import { cn } from '@/lib/utils'
import { chapterIconKey, chapterTitle, groupByPart, inFeatureTree, partLabel, sketchOwners, type Chapter, type TreePart } from '@/lib/chapters-api'
import { useSceneStore } from '@/stores/scene-store'
import { useChapters } from '@/hooks/use-chapters'
import { useHistoryView } from '@/stores/history-view-store'
import { featureFields, fetchEditableParameters, sizeSummary, type EditableParameter } from '@/lib/feature-edit'
import { Box, Check, ChevronDown, ChevronRight, Eye, EyeOff, X, ToolIcons } from '@/components/ui/icons'
import { isStandardPlane, type SketchPlane } from '@/stores/scene-store'
import type { ServerSketchSession } from '@/lib/sketch-api'

/** A sketch as the tree lists it. */
export interface TreeSketch {
  id: string
  /** "Sketch 3", numbered in creation order as CAD trees number them. */
  name: string
  /** The plane it is drawn on, for the row's detail column. */
  plane: string
}

function planeLabel(plane: SketchPlane): string {
  // Face-anchored sketches arrive as { origin, u_axis, v_axis } objects.
  return isStandardPlane(plane) ? plane.toUpperCase() : 'Face'
}

/** The document's sketches, numbered in creation order. */
export function treeSketches(sessions: Map<string, ServerSketchSession>): TreeSketch[] {
  return Array.from(sessions.values())
    .sort((a, b) => a.created_at - b.created_at)
    .map((s, i) => ({ id: s.id, name: `Sketch ${i + 1}`, plane: planeLabel(s.plane) }))
}

/** What a sketch row can do, from the tree that owns sketch actions. */
export interface SketchRowActions {
  selectedIds: Set<string>
  hidden: Set<string>
  onSelect: (id: string, additive: boolean) => void
  onToggleVisible: (id: string) => void
  onContextMenu?: (e: React.MouseEvent, sketch: TreeSketch) => void
}

/** A sketch row: the same row, font and icon family as a feature. */
function SketchRow({ sketch, depth, actions }: { sketch: TreeSketch; depth: number; actions: SketchRowActions }) {
  const visible = !actions.hidden.has(sketch.id)
  const selected = actions.selectedIds.has(sketch.id)
  return (
    <li>
      <div
        role="treeitem"
        aria-selected={selected}
        tabIndex={0}
        onClick={(e) => actions.onSelect(sketch.id, e.shiftKey || e.ctrlKey || e.metaKey)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') actions.onSelect(sketch.id, false)
        }}
        onContextMenu={actions.onContextMenu ? (e) => actions.onContextMenu?.(e, sketch) : undefined}
        title={`${sketch.name} -- on the ${sketch.plane} plane`}
        className={cn(
          'group flex h-6 items-center gap-1.5 rounded px-1.5 cursor-default outline-none',
          'hover:bg-accent/40 focus-visible:ring-1 focus-visible:ring-ring',
          selected && 'bg-accent/60',
          !visible && 'opacity-50',
        )}
        style={{ paddingLeft: 6 + depth * 14 }}
      >
        <span className="w-3 shrink-0" />
        <ToolIcons.sketch size={17} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-foreground">{sketch.name}</span>
        <span className="shrink-0 font-mono text-[11px] text-muted-foreground">{sketch.plane}</span>
        <button
          type="button"
          aria-label={visible ? `Hide ${sketch.name}` : `Show ${sketch.name}`}
          onClick={(e) => {
            e.stopPropagation()
            actions.onToggleVisible(sketch.id)
          }}
          className={cn('shrink-0 text-muted-foreground hover:text-foreground', visible && 'opacity-0 group-hover:opacity-100')}
        >
          {visible ? <Eye size={12} /> : <EyeOff size={12} />}
        </button>
      </div>
    </li>
  )
}

function ChapterIcon({ c, size }: { c: Chapter; size: number }) {
  const key = chapterIconKey(c.kind)
  const Icon = key ? ToolIcons[key] : Box
  return <Icon size={size} />
}

function Verdict({ c }: { c: Chapter }) {
  if (c.sound)
    return (
      <Check size={12} className="text-emerald-600/60 dark:text-emerald-400/60" title="sound — the kernel re-measured every step" />
    )
  const why = c.refusals.map((r) => `step ${r.sequence}: ${r.status}${r.reason ? ` — ${r.reason}` : ''}`).join('\n')
  return <X size={14} title={why || 'not sound'} />
}

/** Who made the feature and why, for the row's hover: the tree shows one
 *  line per feature, as CAD trees do (the approved mockup). */
function byline(c: Chapter): string {
  if (c.author === 'agent') return `Made by ${c.author_detail.model ?? 'the agent'}${c.intent ? ` · ${c.intent}` : ''}`
  if (c.author === 'user') return `Made by ${c.author_detail.name ?? 'you'}`
  return ''
}

/** The feature's name as the tree shows it: capitalised, as a CAD tree
 *  reads ("Flange", not "flange"); the stored name is unchanged. */
function displayName(c: Chapter): string {
  const t = chapterTitle(c)
  return t.charAt(0).toUpperCase() + t.slice(1)
}

function Row({
  c,
  depth,
  selected,
  onSelect,
  ahead = false,
  params,
  sketchOf,
  sketchActions,
}: {
  c: Chapter
  depth: number
  /** The sketch a chapter was made from, when the tree knows it. */
  sketchOf: (c: Chapter) => TreeSketch | undefined
  sketchActions: SketchRowActions
  /** Every size an edit can change, for the row's size column. */
  params: EditableParameter[]
  selected: number | null
  onSelect: (anchor: number) => void
  /** After the slider's position: not applied in what the viewport shows. */
  ahead?: boolean
}) {
  const [open, setOpen] = useState(false)
  const isPattern = c.instances.length > 0
  // A feature made from a sketch owns it, as in every CAD tree
  // (Extrude1 > Sketch1); it folds open like a pattern.
  const sketch = sketchOf(c)
  const opens = isPattern || sketch !== undefined
  return (
    <li>
      <div
        role="treeitem"
        aria-selected={selected === c.anchor}
        aria-expanded={opens ? open : undefined}
        tabIndex={0}
        onClick={() => onSelect(c.anchor)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onSelect(c.anchor)
          if (opens && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) setOpen(e.key === 'ArrowRight')
        }}
        title={byline(c) || undefined}
        className={cn(
          'flex h-6 items-center gap-1.5 rounded px-1.5 cursor-default outline-none',
          'hover:bg-accent/40 focus-visible:ring-1 focus-visible:ring-ring',
          selected === c.anchor && 'bg-accent/60',
          ahead && 'opacity-45',
        )}
        style={{ paddingLeft: 6 + depth * 14 }}
      >
        {opens ? (
          <button
            type="button"
            aria-label={open ? `Fold ${displayName(c)}` : `Open ${displayName(c)}`}
            onClick={(e) => {
              e.stopPropagation()
              setOpen((o) => !o)
            }}
            className="shrink-0 text-muted-foreground"
          >
            {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}
        <ChapterIcon c={c} size={17} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-foreground">{displayName(c)}</span>
        {/* The feature's sizes as an engineer writes them (the mockup's size
            column): Ø80 × 12, R3, 4× Ø8. */}
        <span className="shrink-0 font-mono text-[11px] tabular-nums text-muted-foreground">
          {sizeSummary(c.kind, featureFields(c, params), c.instances.length)}
        </span>
        {c.author === 'agent' && (
          <span aria-label="made by the agent" className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-500" />
        )}
        <span className="shrink-0">
          <Verdict c={c} />
        </span>
      </div>
      {!c.sound && c.refusals[0]?.reason ? (
        <div className="pb-1 text-[11px] leading-4 text-destructive" style={{ paddingLeft: 30 + depth * 14 }}>
          {c.refusals[0].reason}
        </div>
      ) : null}
      {opens && open ? (
        <ul role="group">
          {sketch ? <SketchRow sketch={sketch} depth={depth + 1} actions={sketchActions} /> : null}
          {c.instances.map((i) => (
            <Row
              key={i.anchor}
              c={i}
              depth={depth + 1}
              selected={selected}
              onSelect={onSelect}
              ahead={ahead}
              params={params}
              sketchOf={sketchOf}
              sketchActions={sketchActions}
            />
          ))}
        </ul>
      ) : null}
    </li>
  )
}

/** A part row of the one tree: select it, hide / show it, fold its features. */
function PartRow({
  part,
  count,
  open,
  onFold,
  selected,
  visible,
  onSelect,
  onToggleVisible,
  onContextMenu,
}: {
  part: TreePart
  count: number
  open: boolean
  onFold: () => void
  selected: boolean
  visible: boolean
  onSelect: (additive: boolean) => void
  onToggleVisible: () => void
  onContextMenu?: (e: React.MouseEvent) => void
}) {
  return (
    <div
      role="treeitem"
      aria-selected={selected}
      aria-expanded={count > 0 ? open : undefined}
      tabIndex={0}
      onClick={(e) => onSelect(e.shiftKey || e.ctrlKey || e.metaKey)}
      onContextMenu={onContextMenu}
      onKeyDown={(e) => {
        if (e.key === 'Enter') onSelect(false)
        if (count > 0 && (e.key === 'ArrowRight' || e.key === 'ArrowLeft')) {
          if ((e.key === 'ArrowRight') !== open) onFold()
        }
      }}
      title={`${part.name} -- ${count} feature${count === 1 ? '' : 's'}`}
      className={cn(
        'group flex h-6 items-center gap-1.5 rounded px-1.5 cursor-default outline-none',
        'hover:bg-accent/40 focus-visible:ring-1 focus-visible:ring-ring',
        selected && 'bg-accent/60',
        !visible && 'opacity-50',
      )}
    >
      {count > 0 ? (
        <button
          type="button"
          aria-label={open ? `Fold ${part.name}` : `Open ${part.name}`}
          onClick={(e) => {
            e.stopPropagation()
            onFold()
          }}
          className="shrink-0 text-muted-foreground"
        >
          {open ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
        </button>
      ) : (
        <span className="w-3 shrink-0" />
      )}
      <Box size={16} />
      <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-foreground">{part.name}</span>
      <button
        type="button"
        aria-label={visible ? `Hide ${part.name}` : `Show ${part.name}`}
        onClick={(e) => {
          e.stopPropagation()
          onToggleVisible()
        }}
        className={cn('shrink-0 text-muted-foreground hover:text-foreground', visible && 'opacity-0 group-hover:opacity-100')}
      >
        {visible ? <Eye size={12} /> : <EyeOff size={12} />}
      </button>
    </div>
  )
}

/**
 * One tree for parts and features (Varun 2026-11-14: "shouldn't features
 * and parts be part of the same tree"): each part, with the features that
 * built it nested under it in build order, as established CAD trees show a
 * body and its features. A feature's part is the body its result ended up
 * in (kernel chapters, `part`); features in no standing part follow.
 */
export function ChapterList({
  branch = 'main',
  onPartContextMenu,
  onSketchContextMenu,
}: {
  branch?: string
  /** Right-click on a part row (rename / delete ...). */
  onPartContextMenu?: (e: React.MouseEvent, id: string, name: string) => void
  /** Right-click on a sketch row (edit sketch ...). */
  onSketchContextMenu?: (e: React.MouseEvent, sketch: TreeSketch) => void
}) {
  const { chapters: all, loading, error } = useChapters(branch)
  // Features only: a size edit is history (it stays on the slider), not a
  // row -- the edited feature shows its new size itself.
  const chapters = useMemo(() => all.filter(inFeatureTree), [all])
  const objects = useSceneStore((s) => s.objects)
  const objectOrder = useSceneStore((s) => s.objectOrder)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  const selectObject = useSceneStore((s) => s.selectObject)
  const updateObject = useSceneStore((s) => s.updateObject)
  const parts = useMemo<TreePart[]>(
    () =>
      objectOrder
        .map((id) => objects.get(id))
        .flatMap((o) => {
          const solidId = o?.analyticalGeometry?.solidId
          return o && solidId !== undefined ? [{ id: o.id, solidId, name: o.name }] : []
        }),
    [objects, objectOrder],
  )
  const groups = useMemo(() => groupByPart(chapters, parts), [chapters, parts])
  const serverSketches = useSceneStore((s) => s.serverSketches)
  const hiddenSketchIds = useSceneStore((s) => s.hiddenSketchIds)
  const toggleSketchVisibility = useSceneStore((s) => s.toggleSketchVisibility)
  const sketches = useMemo(() => treeSketches(serverSketches), [serverSketches])
  // Which feature each sketch belongs to (see `sketchOwners`).
  const sketchOwner = useMemo(() => {
    const bodies = [...objects.values()].flatMap((o) => {
      const params = o.analyticalGeometry?.params as Record<string, unknown> | undefined
      const sketchId = params?.['sketch_id']
      const solidId = o.analyticalGeometry?.solidId
      return typeof sketchId === 'string' && solidId !== undefined ? [{ sketchId, solidId }] : []
    })
    return sketchOwners(chapters, sketches.map((s) => s.id), bodies)
  }, [chapters, sketches, objects])
  const sketchById = new Map(sketches.map((s) => [s.id, s]))
  const sketchOf = (c: Chapter) => {
    const id = sketchOwner.owner.get(c.anchor)
    return id ? sketchById.get(id) : undefined
  }
  const looseSketches = sketchOwner.loose.flatMap((id) => sketchById.get(id) ?? [])
  const sketchActions: SketchRowActions = {
    selectedIds,
    hidden: hiddenSketchIds,
    onSelect: (id, additive) => selectObject(id, additive),
    onToggleVisible: toggleSketchVisibility,
    onContextMenu: onSketchContextMenu,
  }
  const [folded, setFolded] = useState<Set<string>>(new Set())
  // Selecting a feature highlights it, as in a feature tree. It never
  // moves the timeline: only the slider travels in time. Rows after the
  // slider's position are greyed, like features below a rollback bar.
  const selected = useHistoryView((s) => s.selectedChapter)
  const select = useHistoryView((s) => s.selectChapter)
  const viewing = useHistoryView((s) => s.step)
  // The sizes, refetched whenever the history changes. A failure leaves the
  // size column empty -- the rows and verdicts still stand.
  const [params, setParams] = useState<EditableParameter[]>([])
  useEffect(() => {
    let live = true
    fetchEditableParameters(branch)
      .then((p) => live && setParams(p))
      .catch(() => live && setParams([]))
    return () => {
      live = false
    }
  }, [branch, chapters])

  if (error && chapters.length === 0 && parts.length === 0) {
    return <div className="px-3 py-2 text-[11px] text-destructive">history unavailable — {error}</div>
  }
  if (chapters.length === 0 && parts.length === 0 && sketches.length === 0) {
    return (
      <div className="px-3 py-2 text-[11px] text-muted-foreground">
        {loading ? '⋯ loading history' : '∅ no parts yet — build something and it appears here'}
      </div>
    )
  }
  const featureRows = (list: Chapter[], depth: number) =>
    list.map((c) => (
      <Row
        key={c.anchor}
        c={c}
        depth={depth}
        selected={selected}
        onSelect={select}
        ahead={viewing !== null && c.anchor > viewing}
        params={params}
        sketchOf={sketchOf}
        sketchActions={sketchActions}
      />
    ))
  return (
    <ul role="tree" aria-label="Parts and features" className="px-1 py-1">
      {groups.map((g) =>
        g.part ? (
          <li key={g.part.id}>
            <PartRow
              part={{ ...g.part, name: partLabel(g.part, g.features) }}
              count={g.features.length}
              open={!folded.has(g.part.id)}
              onFold={() =>
                setFolded((f) => {
                  const next = new Set(f)
                  const id = (g.part as TreePart).id
                  if (next.has(id)) next.delete(id)
                  else next.add(id)
                  return next
                })
              }
              selected={selectedIds.has(g.part.id)}
              visible={objects.get(g.part.id)?.visible ?? true}
              onSelect={(additive) => selectObject((g.part as TreePart).id, additive)}
              onToggleVisible={() => {
                const id = (g.part as TreePart).id
                updateObject(id, { visible: !(objects.get(id)?.visible ?? true) })
              }}
              onContextMenu={
                onPartContextMenu
                  ? (e) => onPartContextMenu(e, (g.part as TreePart).id, (g.part as TreePart).name)
                  : undefined
              }
            />
            {!folded.has(g.part.id) && g.features.length > 0 && <ul role="group">{featureRows(g.features, 1)}</ul>}
          </li>
        ) : (
          featureRows(g.features, 0)
        ),
      )}
      {/* Sketches no feature has used yet: after the parts, in the same rows. */}
      {looseSketches.map((sk) => (
        <SketchRow key={sk.id} sketch={sk} depth={0} actions={sketchActions} />
      ))}
    </ul>
  )
}
