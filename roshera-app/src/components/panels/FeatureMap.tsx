/**
 * The timeline Map: one feature graph (Varun 2026-11-18: "one look at map
 * should tell everything ... how it was made .. what was agent / what was
 * human ... can it be used to rebuild the part").
 *
 * - Features left to right in build order, each with its picture, who made
 *   it (✦ the agent, or a person's initials), its sizes, the reason the agent
 *   gave, and whether the kernel re-measured it sound (✓) or refused it (✗,
 *   with the kernel's reason).
 * - Wires from each feature to what it was built on.
 * - Rebuild & certify: the recipe replayed into a fresh model and measured
 *   against the live part, by the kernel.
 * - Recipe: the same graph as a numbered build sheet, every size and what it
 *   was built on, to copy or print -- enough to rebuild it by hand.
 *
 * One view; no kernel ids in what a person reads.
 */
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ReactFlow,
  Background,
  Controls,
  Handle,
  Position,
  MarkerType,
  type Node,
  type Edge,
  type NodeProps,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { X } from '@/components/ui/icons'
import { cn } from '@/lib/utils'
import { framePictureUrl } from '@/lib/chapters-api'
import { useHistoryView } from '@/stores/history-view-store'
import {
  NODE_HEIGHT,
  NODE_WIDTH,
  authorMark,
  layoutFeatureGraph,
  rebuildLine,
  summaryLine,
  type FeatureGraph,
  type MapFeature,
  type RebuildCheck,
} from '@/lib/feature-graph'
import { fetchFeatureGraph, rebuildCheck } from '@/lib/feature-graph-api'

type FeatureNodeData = { feature: MapFeature; branch: string; selected: boolean }
type SetAsideNodeData = { count: number; reason: string }

function FeatureNode({ data }: NodeProps<Node<FeatureNodeData>>) {
  const f = data.feature
  const who = authorMark(f)
  const refusal = f.refusals?.[0]
  const sound = f.sound !== false
  return (
    <div
      className={cn(
        'flex flex-col overflow-hidden rounded-md border bg-card text-left shadow-sm',
        data.selected ? 'border-primary ring-1 ring-primary' : 'border-border',
      )}
      style={{ width: NODE_WIDTH, height: NODE_HEIGHT }}
    >
      <Handle type="target" position={Position.Left} className="!opacity-0" />
      <div
        className={cn('h-0.5 w-full', f.author === 'agent' ? 'bg-sky-500/80' : f.author === 'user' ? 'bg-amber-500/80' : 'bg-border')}
      />
      <div className="h-[92px] w-full shrink-0 bg-muted/30">
        <img
          src={framePictureUrl(data.branch, f.anchor, 256)}
          alt=""
          draggable={false}
          className="h-full w-full object-contain"
        />
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-0.5 px-2 py-1.5">
        <div className="flex items-center gap-1.5">
          <span
            title={who.title}
            className={cn(
              'grid h-4 min-w-4 shrink-0 place-items-center rounded px-0.5 text-[11px] font-semibold',
              f.author === 'agent' ? 'bg-sky-500/15 text-sky-500' : 'bg-amber-500/15 text-amber-600 dark:text-amber-400',
            )}
          >
            {who.glyph}
          </span>
          <span className="truncate text-[12px] font-medium" title={f.display_name ?? f.name ?? f.kind}>
            {f.display_name ?? f.name ?? f.kind}
          </span>
          <span
            className={cn('ml-auto shrink-0 text-[11px]', sound ? 'text-emerald-500' : 'text-red-500')}
            title={sound ? 'Rebuilds sound: re-measured by the kernel' : `Does not rebuild: ${refusal?.reason ?? refusal?.status ?? ''}`}
          >
            {sound ? '✓' : '✗'}
          </span>
        </div>
        {f.sizes && f.sizes.length > 0 && (
          <div className="truncate font-mono text-[11px] text-foreground/80" title={f.sizes.join(' · ')}>
            {f.sizes.join(' · ')}
          </div>
        )}
        {f.intent ? (
          <div className="line-clamp-2 text-[11px] leading-snug text-muted-foreground" title={f.intent}>
            “{f.intent}”
          </div>
        ) : !sound && refusal ? (
          <div className="line-clamp-2 text-[11px] leading-snug text-red-500" title={refusal.reason ?? refusal.status}>
            {refusal.reason ?? refusal.status}
          </div>
        ) : null}
      </div>
      <Handle type="source" position={Position.Right} className="!opacity-0" />
    </div>
  )
}

function SetAsideNode({ data }: NodeProps<Node<SetAsideNodeData>>) {
  return (
    <div
      className="flex flex-col justify-center gap-1 rounded-md border border-red-500/60 bg-red-500/5 px-3 text-left"
      style={{ width: NODE_WIDTH, height: NODE_HEIGHT }}
      title={data.reason}
    >
      <Handle type="target" position={Position.Left} className="!opacity-0" />
      <div className="text-[12px] font-medium text-red-500">✗ {data.count} steps set aside</div>
      <div className="text-[11px] leading-snug text-muted-foreground">
        They cannot be replayed faithfully, so they are not rebuilt or guessed.
      </div>
    </div>
  )
}

const nodeTypes = { feature: FeatureNode, setAside: SetAsideNode }

export default function FeatureMap({ branch = 'main', onClose }: { branch?: string; onClose: () => void }) {
  const [graph, setGraph] = useState<FeatureGraph | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [showRecipe, setShowRecipe] = useState(false)
  const [rebuild, setRebuild] = useState<{ running: boolean; result: RebuildCheck | null; error: string | null }>({
    running: false,
    result: null,
    error: null,
  })
  const [hoverEdge, setHoverEdge] = useState<string | null>(null)
  const selected = useHistoryView((s) => s.selectedChapter)

  useEffect(() => {
    let live = true
    fetchFeatureGraph(branch)
      .then((g) => live && setGraph(g))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : String(e)))
    return () => {
      live = false
    }
  }, [branch])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  const lastAnchor = useMemo(
    () => (graph ? Math.max(-1, ...graph.features.map((f) => f.anchor)) : -1),
    [graph],
  )

  const { nodes, edges } = useMemo(() => {
    if (!graph) return { nodes: [] as Node[], edges: [] as Edge[] }
    const at = layoutFeatureGraph(graph.features, graph.edges)
    const ns: Node[] = graph.features.map((f) => ({
      id: String(f.anchor),
      type: 'feature',
      position: at.get(f.anchor) ?? { x: 0, y: 0 },
      data: { feature: f, branch, selected: selected === f.anchor },
      draggable: false,
    }))
    const es: Edge[] = graph.edges.map((e) => {
      const id = `${e.from}-${e.to}`
      return {
        id,
        source: String(e.from),
        target: String(e.to),
        label: hoverEdge === id ? `built on ${e.via}` : undefined,
        markerEnd: { type: MarkerType.ArrowClosed, width: 14, height: 14 },
        style: { strokeWidth: hoverEdge === id ? 2 : 1.25 },
      }
    })
    if (graph.summary.set_aside > 0) {
      const maxX = Math.max(0, ...[...at.values()].map((p) => p.x))
      const id = 'set-aside'
      ns.push({
        id,
        type: 'setAside',
        position: { x: maxX + NODE_WIDTH + 60, y: at.get(lastAnchor)?.y ?? 0 },
        data: {
          count: graph.summary.set_aside,
          reason: 'The saved history after this point cannot be replayed faithfully.',
        },
        draggable: false,
      })
      if (lastAnchor >= 0) {
        es.push({ id: 'to-set-aside', source: String(lastAnchor), target: id, style: { strokeDasharray: '4 4' } })
      }
    }
    return { nodes: ns, edges: es }
  }, [graph, branch, selected, hoverEdge, lastAnchor])

  const openFeature = useCallback(
    (anchor: number, title: string) => {
      const hv = useHistoryView.getState()
      if (anchor === lastAnchor) hv.view(null)
      else hv.view(anchor, title, branch)
      useHistoryView.setState({ selectedChapter: anchor })
    },
    [branch, lastAnchor],
  )

  const runRebuild = useCallback(() => {
    setRebuild({ running: true, result: null, error: null })
    rebuildCheck(branch)
      .then((result) => setRebuild({ running: false, result, error: null }))
      .catch((e: unknown) => setRebuild({ running: false, result: null, error: e instanceof Error ? e.message : String(e) }))
  }, [branch])

  const recipeText = graph ? graph.recipe.join('\n') : ''
  const copyRecipe = () => {
    void navigator.clipboard?.writeText(recipeText)
  }
  const printRecipe = () => {
    const w = window.open('', '_blank', 'noopener=no,width=720,height=900')
    if (!w) return
    const pre = w.document.createElement('pre')
    pre.style.font = '13px/1.6 ui-monospace, monospace'
    pre.style.whiteSpace = 'pre-wrap'
    pre.textContent = `Recipe — ${branch}\n\n${recipeText}`
    w.document.body.appendChild(pre)
    w.print()
  }

  return (
    <div
      role="dialog"
      aria-label="Map: how this part was made"
      className="fixed inset-6 z-50 flex flex-col overflow-hidden rounded-lg border border-border bg-card shadow-2xl"
    >
      <header className="flex items-center gap-3 border-b border-border px-4 py-2">
        <div className="min-w-0">
          <div className="text-[13px] font-semibold">How this part was made</div>
          <div className="truncate text-[11px] text-muted-foreground">
            {graph ? `${branch} · ${summaryLine(graph.summary)}` : error ? '' : 'Reading the history…'}
          </div>
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <button
            type="button"
            onClick={() => setShowRecipe((r) => !r)}
            aria-pressed={showRecipe}
            disabled={!graph}
            className={cn(
              'cad-focus rounded-md border px-2.5 py-1 text-[12px]',
              showRecipe ? 'border-primary bg-primary/10' : 'border-border hover:bg-accent/40',
            )}
            title="The same history as a written build sheet: every size and what it was built on"
          >
            Recipe
          </button>
          <button
            type="button"
            onClick={runRebuild}
            disabled={!graph || rebuild.running || graph.features.length === 0}
            className="cad-focus rounded-md border border-border px-2.5 py-1 text-[12px] hover:bg-accent/40 disabled:opacity-50"
            title="Replay the recipe into a fresh model and have the kernel measure it against the live part"
          >
            {rebuild.running ? 'Rebuilding…' : 'Rebuild & certify'}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close the map"
            className="cad-focus grid h-7 w-7 place-items-center rounded-md hover:bg-accent/40"
          >
            <X size={14} />
          </button>
        </div>
      </header>

      {(rebuild.result || rebuild.error) && (
        <div
          role="status"
          className={cn(
            'border-b px-4 py-1.5 text-[12px]',
            rebuild.error
              ? 'border-red-500/40 text-red-500'
              : rebuild.result?.identical
                ? 'border-emerald-500/40 text-emerald-600 dark:text-emerald-400'
                : 'border-red-500/40 text-red-500',
          )}
        >
          {rebuild.error ?? (rebuild.result ? rebuildLine(rebuild.result) : '')}
        </div>
      )}

      <div className="relative min-h-0 flex-1">
        {error ? (
          <div className="grid h-full place-items-center text-[12px] text-red-500">{error}</div>
        ) : !graph ? (
          <div className="grid h-full place-items-center text-[12px] text-muted-foreground">Reading the history…</div>
        ) : graph.features.length === 0 ? (
          <div className="grid h-full place-items-center text-[12px] text-muted-foreground">
            ∅ no steps yet — build something and it appears here
          </div>
        ) : showRecipe ? (
          <div className="h-full overflow-auto px-6 py-4">
            <div className="mb-3 flex items-center gap-2">
              <div className="text-[12px] text-muted-foreground">
                Every step in order, with its sizes and what it was built on.
              </div>
              <button type="button" onClick={copyRecipe} className="cad-focus ml-auto rounded-md border border-border px-2 py-0.5 text-[11px] hover:bg-accent/40">
                Copy
              </button>
              <button type="button" onClick={printRecipe} className="cad-focus rounded-md border border-border px-2 py-0.5 text-[11px] hover:bg-accent/40">
                Print
              </button>
            </div>
            <ol className="space-y-1.5 font-mono text-[12px] leading-relaxed">
              {graph.recipe.map((line) => (
                <li key={line}>{line}</li>
              ))}
              {graph.summary.set_aside > 0 && (
                <li className="text-red-500">
                  ✗ {graph.summary.set_aside} later steps set aside: they cannot be replayed faithfully.
                </li>
              )}
            </ol>
          </div>
        ) : (
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            fitView
            fitViewOptions={{ padding: 0.15, maxZoom: 1.1 }}
            minZoom={0.2}
            nodesConnectable={false}
            elementsSelectable={false}
            proOptions={{ hideAttribution: true }}
            onNodeClick={(_, n) => {
              if (n.type !== 'feature') return
              const f = (n.data as FeatureNodeData).feature
              openFeature(f.anchor, f.display_name ?? f.name ?? f.kind ?? 'feature')
            }}
            onEdgeMouseEnter={(_, e) => setHoverEdge(e.id)}
            onEdgeMouseLeave={() => setHoverEdge(null)}
          >
            <Background gap={24} size={1} />
            <Controls showInteractive={false} />
          </ReactFlow>
        )}
      </div>

      <footer className="flex items-center gap-4 border-t border-border px-4 py-1.5 text-[11px] text-muted-foreground">
        <span>
          <span className="text-sky-500">✦</span> made by the agent
        </span>
        <span>
          <span className="text-amber-600 dark:text-amber-400">VS</span> made by a person
        </span>
        <span>
          <span className="text-emerald-500">✓</span> rebuilds sound · <span className="text-red-500">✗</span> does not
        </span>
        <span className="ml-auto">click a feature to see the part after it and edit its sizes · Esc closes</span>
      </footer>
    </div>
  )
}
