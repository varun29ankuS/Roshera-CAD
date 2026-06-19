import { useEffect, useState, useCallback } from 'react'
import { TopBar } from '@/components/layout/TopBar'
import { ToolBar } from '@/components/layout/ToolBar'
import { StatusBar } from '@/components/layout/StatusBar'
import { CADViewport } from '@/components/viewport/CADViewport'
import { AgentEyePanel } from '@/components/viewport/AgentEyePanel'
import { PropertiesPanel } from '@/components/panels/PropertiesPanel'
import { Blackboard } from '@/components/panels/Blackboard'
import { ModelTree } from '@/components/panels/ModelTree'
import { Timeline } from '@/components/panels/Timeline'
import { DrawingsWorkspace } from '@/components/panels/DrawingsWorkspace'
import { AssemblyWorkspace } from '@/components/panels/AssemblyWorkspace'
import { DemoGallery } from '@/components/demo-gallery/DemoGallery'
import { CommandPalette } from '@/components/CommandPalette'
import { useKeyboardShortcuts } from '@/lib/shortcuts'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { initWebSocket, teardownWebSocket } from '@/lib/ws-bridge'
import { ViewportBridge } from '@/lib/viewport-bridge'

const DEMOS_HASH = '#/demos'

function isDemosRoute(): boolean {
  return typeof window !== 'undefined' && window.location.hash === DEMOS_HASH
}

export function App() {
  useKeyboardShortcuts()
  const hasSelection = useSceneStore((s) => s.selectedIds.size > 0)
  // Default to Part workspace when nothing has been chosen yet so the
  // UI is never blank. The tab strip still mirrors the store so users
  // can switch freely.
  const docMode = useDocModeStore((s) => s.mode) ?? 'part'
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
      {/* The active workspace (Modeling / Drawing / future Assembly) is
          chosen from the right-side switcher in `TopBar`. We never
          present the three as equal sibling tabs because they aren't —
          Modeling is the dominant mode; Drawing is derived; Assembly
          combines existing parts. Treating them as a top-level tab
          strip implied otherwise. */}
      <TopBar />
      <div className="flex flex-1 min-h-0">
        {/* The Drawing workspace replaces the 3D pipeline entirely:
            no ToolBar (modelling primitives are irrelevant in a 2D
            sheet workspace), no PropertiesPanel, no Timeline strip.
            The kernel still drives the projection; this pane is the
            sheet inspector. */}
        {docMode === 'drawing' ? (
          <DrawingsWorkspace />
        ) : docMode === 'assembly' ? (
          <AssemblyWorkspace />
        ) : (
          <>
            <ToolBar />

            {/* Viewport + floating overlays in the standard CAD layout */}
            <div className="relative flex-1 overflow-hidden">
              <CADViewport />
              <Blackboard />
              <AgentEyePanel />

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
          </>
        )}
      </div>
      {/* Timeline — horizontal strip, full width. Hidden in Drawing
          mode where the sheet *is* the work product. */}
      {docMode !== 'drawing' && <Timeline />}
      <StatusBar />
      {/* Command palette — fixed-position overlay; reachable from
          every workspace via Ctrl/Cmd-K. Mounted unconditionally so
          the keybinding works the moment the app loads, not after
          the first interaction. */}
      <CommandPalette />
    </div>
  )
}
