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
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react'

const DEMOS_HASH = '#/demos'

function isDemosRoute(): boolean {
  return typeof window !== 'undefined' && window.location.hash === DEMOS_HASH
}

export function App() {
  useKeyboardShortcuts()
  const hasSelection = useSceneStore((s) => s.selectedIds.size > 0)
  const [leftPanelOpen, setLeftPanelOpen] = useState(true)
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

        {/* Left panel: Model Tree + Timeline */}
        {leftPanelOpen && (
          <div className="flex flex-col w-52 border-r border-white/5 bg-transparent">
            {/* Model Tree — top half */}
            <div className="flex-1 min-h-0 border-b border-white/5">
              <ModelTree />
            </div>
            {/* Timeline — bottom half */}
            <div className="flex-1 min-h-0">
              <Timeline />
            </div>
          </div>
        )}

        {/* Viewport + overlays */}
        <div className="relative flex-1 overflow-hidden">
          <CADViewport />
          <AIChatPanel />

          {/* Toggle left panel button */}
          <button
            onClick={() => setLeftPanelOpen(!leftPanelOpen)}
            className="absolute top-2 left-2 z-10 w-7 h-7 flex items-center justify-center rounded-md bg-card/80 backdrop-blur-sm border border-border text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
            title={leftPanelOpen ? 'Hide panels' : 'Show panels'}
          >
            {leftPanelOpen ? <PanelLeftClose size={14} /> : <PanelLeftOpen size={14} />}
          </button>
        </div>

        {/* Right panel: Properties (conditional) */}
        {hasSelection && <PropertiesPanel />}
      </div>
      <StatusBar />
    </div>
  )
}
