import { useEffect, useState, useCallback } from 'react'
import { TopBar } from '@/components/layout/TopBar'
import { ToolBar } from '@/components/layout/ToolBar'
import { StatusBar } from '@/components/layout/StatusBar'
import { CADViewport } from '@/components/viewport/CADViewport'
import { PropertiesPanel } from '@/components/panels/PropertiesPanel'
import { AIChatPanel } from '@/components/panels/AIChatPanel'
import { ModelTree } from '@/components/panels/ModelTree'
import { Timeline } from '@/components/panels/Timeline'
import { DemoGallery } from '@/components/demo-gallery/DemoGallery'
import { useKeyboardShortcuts } from '@/lib/shortcuts'
import { useSceneStore } from '@/stores/scene-store'
import { initWebSocket, teardownWebSocket } from '@/lib/ws-bridge'
import { ViewportBridge } from '@/lib/viewport-bridge'

const DEMOS_HASH = '#/demos'

function isDemosRoute(): boolean {
  return typeof window !== 'undefined' && window.location.hash === DEMOS_HASH
}

export function App() {
  useKeyboardShortcuts()
  const hasSelection = useSceneStore((s) => s.selectedIds.size > 0)
  const [browserOpen, setBrowserOpen] = useState(true)
  const [route, setRoute] = useState<'workspace' | 'demos'>(
    isDemosRoute() ? 'demos' : 'workspace',
  )

  useEffect(() => {
    const onHashChange = () => setRoute(isDemosRoute() ? 'demos' : 'workspace')
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])

  // Only the workspace owns the live websocket. The gallery is purely
  // local file fetches, so we tear the socket down when entering it and
  // re-init when returning.
  useEffect(() => {
    if (route === 'workspace') {
      initWebSocket()
      return () => teardownWebSocket()
    }
  }, [route])

  const exitGallery = useCallback(() => {
    window.location.hash = ''
  }, [])

  if (route === 'demos') {
    return <DemoGallery onExit={exitGallery} />
  }

  return (
    <div className="flex flex-col h-screen w-screen bg-background text-foreground select-none">
      <ViewportBridge />
      <TopBar />
      <div className="flex flex-1 min-h-0">
        <ToolBar />

        {/* Viewport + Fusion-style floating overlays */}
        <div className="relative flex-1 overflow-hidden">
          <CADViewport />
          <AIChatPanel />

          {/* Browser — single consolidated panel. The header chip is
              always visible and acts as the collapse toggle; an
              inline segmented control flips the body between the
              assembly hierarchy ("parts") and the timeline-derived
              feature tree ("features"). Only the header carries its
              own outline, so the chip stays as an anchor even when
              the tree is hidden. */}
          <div className="absolute top-2 left-2 z-10 w-56 max-h-[calc(100%-1rem)] flex flex-col overflow-hidden">
            <ModelTree
              expanded={browserOpen}
              onToggle={() => setBrowserOpen((open) => !open)}
            />
          </div>
        </div>

        {/* Right panel: Properties (conditional) */}
        {hasSelection && <PropertiesPanel />}
      </div>
      {/* Timeline — horizontal strip, full width */}
      <Timeline />
      <StatusBar />
    </div>
  )
}
