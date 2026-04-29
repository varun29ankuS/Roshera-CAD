import { useEffect, useMemo, useState, useCallback } from 'react'
import { ArrowLeft, AlertTriangle, RefreshCw } from 'lucide-react'
import { CADViewport } from '@/components/viewport/CADViewport'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Button } from '@/components/ui/button'
import { useSceneStore } from '@/stores/scene-store'
import { categoryInfo, type DemoEntry, type DemoManifest } from '@/lib/demo-types'
import { loadStl, stlToCadObject } from '@/lib/stl-loader'
import { DemoCard } from './DemoCard'

const DEMO_OBJECT_ID = 'demo-gallery-active'

/// Group demos by category, preserving manifest order within each group.
function groupByCategory(demos: DemoEntry[]): Map<string, DemoEntry[]> {
  const groups = new Map<string, DemoEntry[]>()
  for (const demo of demos) {
    const list = groups.get(demo.category) ?? []
    list.push(demo)
    groups.set(demo.category, list)
  }
  return groups
}

interface DemoGalleryProps {
  onExit: () => void
}

export function DemoGallery({ onExit }: DemoGalleryProps) {
  const [manifest, setManifest] = useState<DemoManifest | null>(null)
  const [manifestError, setManifestError] = useState<string | null>(null)
  const [activeId, setActiveId] = useState<string | null>(null)
  const [loadingId, setLoadingId] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)

  const addObject = useSceneStore((s) => s.addObject)
  const removeObject = useSceneStore((s) => s.removeObject)
  const clearScene = useSceneStore((s) => s.clearScene)

  // Clear any prior workspace objects when entering the gallery so the
  // viewport shows ONLY the demo mesh. On unmount restore the empty
  // scene — the workspace state is owned by the live session anyway.
  useEffect(() => {
    clearScene()
    return () => {
      clearScene()
    }
  }, [clearScene])

  const loadManifest = useCallback(async () => {
    setManifestError(null)
    try {
      const response = await fetch('/demos/manifest.json', { cache: 'no-store' })
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`)
      }
      const data = (await response.json()) as DemoManifest
      setManifest(data)
    } catch (err) {
      setManifestError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useEffect(() => {
    void loadManifest()
  }, [loadManifest])

  const grouped = useMemo(
    () => (manifest ? groupByCategory(manifest.demos) : new Map<string, DemoEntry[]>()),
    [manifest],
  )

  const handleLoad = useCallback(
    async (demo: DemoEntry) => {
      const id = `${demo.category}/${demo.filename}`
      setLoadingId(id)
      setLoadError(null)
      try {
        const loaded = await loadStl(`/demos/${demo.stl_path}`)
        // Replace previously-loaded demo mesh.
        removeObject(DEMO_OBJECT_ID)
        addObject(stlToCadObject(DEMO_OBJECT_ID, demo.filename, loaded))
        setActiveId(id)
      } catch (err) {
        setLoadError(err instanceof Error ? err.message : String(err))
      } finally {
        setLoadingId(null)
      }
    },
    [addObject, removeObject],
  )

  return (
    <div className="flex flex-col h-screen w-screen bg-background text-foreground">
      {/* Header bar */}
      <div className="cad-panel border-b flex items-center justify-between px-4 py-2">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={onExit} className="gap-1.5 font-mono uppercase tracking-wider text-[11px]">
            <ArrowLeft className="w-4 h-4" />
            Workspace
          </Button>
          <div className="h-4 w-px bg-border" />
          <div>
            <div className="text-[11px] font-mono uppercase tracking-[0.16em] text-foreground">
              Kernel Demo Gallery
            </div>
            <div className="text-[10px] text-muted-foreground mt-0.5">
              Live STL output from each kernel example. Click a card to render it.
            </div>
          </div>
        </div>
        <Button variant="ghost" size="sm" onClick={() => void loadManifest()} className="gap-1.5 font-mono uppercase tracking-wider text-[11px]">
          <RefreshCw className="w-3.5 h-3.5" />
          Refresh
        </Button>
      </div>

      {/* Body: split between gallery list (left) and viewport (right) */}
      <div className="flex flex-1 min-h-0">
        {/* Gallery list */}
        <div className="flex flex-col w-96 cad-panel border-r">
          {manifestError && (
            <div className="m-4 p-3 rounded-md border border-destructive/40 bg-destructive/10 text-xs flex items-start gap-2">
              <AlertTriangle className="w-4 h-4 text-destructive shrink-0 mt-0.5" />
              <div>
                <div className="font-semibold text-destructive">Manifest unavailable</div>
                <div className="text-muted-foreground mt-1">{manifestError}</div>
                <div className="text-muted-foreground mt-1">
                  Run the kernel demos with{' '}
                  <code className="font-mono text-[10px] bg-background px-1 py-0.5 rounded">
                    ROSHERA_DEMO_OUT=../roshera-app/public/demos cargo run --release --example
                    demo_X
                  </code>{' '}
                  to populate <code className="font-mono text-[10px]">/demos/</code>.
                </div>
              </div>
            </div>
          )}

          <ScrollArea className="flex-1 min-h-0">
            <div className="p-4 space-y-6">
              {[...grouped.entries()].map(([category, demos]) => {
                const info = categoryInfo(category)
                return (
                  <div key={category} className="space-y-2">
                    <div className="border-b border-border/60 pb-1.5">
                      <div className="text-[10px] font-mono uppercase tracking-[0.16em] text-foreground">
                        {info.title}
                      </div>
                      {info.description && (
                        <div className="text-[10px] text-muted-foreground mt-0.5">
                          {info.description}
                        </div>
                      )}
                    </div>
                    <div className="grid grid-cols-1 gap-2">
                      {demos.map((demo) => {
                        const id = `${demo.category}/${demo.filename}`
                        return (
                          <DemoCard
                            key={id}
                            demo={demo}
                            isActive={activeId === id}
                            isLoading={loadingId === id}
                            onLoad={() => void handleLoad(demo)}
                          />
                        )
                      })}
                    </div>
                  </div>
                )
              })}

              {!manifestError && manifest && manifest.demos.length === 0 && (
                <div className="text-xs text-muted-foreground">
                  Manifest is empty. Re-run the kernel demos to populate the gallery.
                </div>
              )}
            </div>
          </ScrollArea>
        </div>

        {/* Viewport */}
        <div className="relative flex-1 overflow-hidden">
          <CADViewport />
          {loadError && (
            <div className="absolute top-3 left-1/2 -translate-x-1/2 px-3 py-2 rounded-md border border-destructive/40 bg-destructive/10 text-xs text-destructive flex items-center gap-2">
              <AlertTriangle className="w-3.5 h-3.5" />
              {loadError}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
