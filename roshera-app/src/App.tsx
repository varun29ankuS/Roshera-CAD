import { useEffect, useState, useCallback } from 'react'
import { TopBar } from '@/components/layout/TopBar'
import { DocumentTabs } from '@/components/layout/DocumentTabs'
import { ToolBar } from '@/components/layout/ToolBar'
import { StatusBar } from '@/components/layout/StatusBar'
import { CADViewport } from '@/components/viewport/CADViewport'
import { AgentEyePanel } from '@/components/viewport/AgentEyePanel'
import { StepImportDropzone } from '@/components/viewport/StepImportDropzone'
import { PropertiesPanel } from '@/components/panels/PropertiesPanel'
import { Blackboard } from '@/components/panels/Blackboard'
import { ModelTree } from '@/components/panels/ModelTree'
import { Timeline } from '@/components/panels/Timeline'
import { DrawingsWorkspace } from '@/components/panels/DrawingsWorkspace'
import { AssemblyWorkspace } from '@/components/panels/AssemblyWorkspace'
import { DemoGallery } from '@/components/demo-gallery/DemoGallery'
import { BlackboardFixtures } from '@/components/dev/BlackboardFixtures'
import { SemanticsPanel } from '@/components/dev/SemanticsPanel'
import { CommandPalette } from '@/components/CommandPalette'
import { LoginDialog } from '@/components/LoginDialog'
import { cn } from '@/lib/utils'
import { useKeyboardShortcuts } from '@/lib/shortcuts'
import { useSceneStore } from '@/stores/scene-store'
import { useDocModeStore } from '@/stores/doc-mode-store'
import { initWebSocket, teardownWebSocket } from '@/lib/ws-bridge'
import { installBackendBlackboard } from '@/lib/blackboard-api'
import { establishAcpSession } from '@/lib/acp-blackboard'
import { ViewportBridge } from '@/lib/viewport-bridge'

const DEMOS_HASH = '#/demos'
// Dev harness: every Blackboard state (streaming, typed cards, attention
// layout, GD&T symbol table) in one place for visual verification without
// staging them by hand. See components/dev/BlackboardFixtures.tsx.
const BLACKBOARD_FIXTURES_HASH = '#/blackboard-fixtures'
// Dev panel: the agent-facing tool ontology (live from /api/agent/tool-registry)
// + provenance coverage with the gaps rendered as gaps. See
// components/dev/SemanticsPanel.tsx.
const SEMANTICS_HASH = '#/semantics'

type Route = 'workspace' | 'demos' | 'blackboard-fixtures' | 'semantics'

function routeFromHash(): Route {
  if (typeof window === 'undefined') return 'workspace'
  if (window.location.hash === DEMOS_HASH) return 'demos'
  if (window.location.hash === BLACKBOARD_FIXTURES_HASH) return 'blackboard-fixtures'
  if (window.location.hash === SEMANTICS_HASH) return 'semantics'
  return 'workspace'
}

export function App() {
  useKeyboardShortcuts()
  const hasSelection = useSceneStore((s) => s.selectedIds.size > 0)
  // Default to Part workspace when nothing has been chosen yet so the
  // UI is never blank. The tab strip still mirrors the store so users
  // can switch freely.
  const docMode = useDocModeStore((s) => s.mode) ?? 'part'
  const [browserOpen, setBrowserOpen] = useState(true)
  const [route, setRoute] = useState<Route>(routeFromHash)

  useEffect(() => {
    const onHashChange = () => setRoute(routeFromHash())
    window.addEventListener('hashchange', onHashChange)
    return () => window.removeEventListener('hashchange', onHashChange)
  }, [])

  // Install the backend-backed Blackboard adapter once at bootstrap: the
  // notebook now reads + writes the server (lines an agent adds over MCP
  // appear live here via a short poll), with a localStorage fallback when
  // the backend is unreachable. Swaps only the persistence adapter — the
  // store's reducers are untouched. See `lib/blackboard-api.ts`.
  useEffect(() => {
    const stop = installBackendBlackboard()
    return stop
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

  // Start the agent session when the workspace mounts, rather than
  // waiting for the user's first blackboard message.
  //
  // `acpLive` — the one fact the AI chip is allowed to render (see
  // `ProviderSettingsDialog`'s note that driving the chip off config
  // "was the bug") — is CLIENT-side state with no rehydration: a page
  // reload resets it to `false` while goose's session keeps running on
  // the backend. So the chip read RED over a genuinely live harness,
  // and only went green once something happened to call
  // `getAcpClient()`. That is the mirror image of the bug the `acpLive`
  // switch fixed: green over a dead agent became red over a live one.
  // Both are honest about what the client knows and wrong about what is
  // true.
  //
  // `establishAcpSession()` already existed for exactly this ("...not to
  // leave the agent unstarted until the user's first blackboard
  // message") but its only caller was the settings dialog's connect
  // button. This is the missing call site, not new machinery.
  //
  // Failures are swallowed deliberately: the agent not starting must
  // never block the CAD workspace from rendering. The chip stays red,
  // which is then TRUE, and the dialog reports why.
  useEffect(() => {
    if (route !== 'workspace') return
    let cancelled = false
    void (async () => {
      try {
        await establishAcpSession()
      } catch (err) {
        if (!cancelled) {
          console.warn('[App] agent session did not start on load:', err)
        }
      }
    })()
    return () => {
      cancelled = true
    }
  }, [route])

  const exitGallery = useCallback(() => {
    window.location.hash = ''
  }, [])

  if (route === 'demos') {
    return <DemoGallery onExit={exitGallery} />
  }

  if (route === 'blackboard-fixtures') {
    return <BlackboardFixtures onExit={exitGallery} />
  }

  if (route === 'semantics') {
    return <SemanticsPanel onExit={exitGallery} />
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
      <DocumentTabs />
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

            {/* The tree lives INSIDE the viewport, over the model — not beside
                it. I had docked it as a column for symmetry with the right
                rail, and Varun rejected that on the ground that matters more
                than symmetry: a docked tree permanently narrows the graphics
                area, and in CAD the graphics area is the work. Every kernel
                this competes with overlays its feature tree for exactly that
                reason.
                What survives from the dock is the part that was actually an
                improvement — it collapses to a 40px strip, which the old
                floating card could not do, so the model can be seen whole
                without losing the tree entirely. */}
            <div className="relative flex-1 overflow-hidden">
              <CADViewport />
              <StepImportDropzone />

              <div
                className={cn(
                  // Transparent, so the model reads THROUGH the tree instead
                  // of behind a slab. No blur: blur over a CAD viewport smears
                  // the one thing the app exists to show, and the point here is
                  // to see the geometry, not a frosted version of it.
                  'absolute bottom-2 left-2 top-2 z-10 flex flex-col overflow-hidden rounded border border-border/70 bg-card/40 transition-[width] duration-200',
                  browserOpen ? 'w-56' : 'w-10',
                )}
              >
                <ModelTree
                  expanded={browserOpen}
                  onToggle={() => setBrowserOpen((open) => !open)}
                />
              </div>

              {/* Properties: about the SELECTED OBJECT, so it belongs with the
                  object. Top-right, appearing only when there is a selection —
                  it was a permanent tenant of the right rail, which meant the
                  conversation lost a third of its column to a panel that is
                  empty most of the time. */}
              {hasSelection && (
                <div className="absolute right-2 top-2 z-10 flex max-h-[45%] w-56 flex-col overflow-hidden rounded border border-border bg-card/95">
                  <PropertiesPanel />
                </div>
              )}

              {/* Agent Eye: a RENDER of the part, which makes it an instrument
                  of the model rather than of the dialogue. Bottom-right, out of
                  the tree's corner, and it keeps its own collapse. */}
              <div className="absolute bottom-2 right-2 z-10 flex w-56 flex-col overflow-hidden rounded border border-border bg-card/95">
                <AgentEyePanel />
              </div>

              {/* The Blackboard is an overlay too. It has now been a floating
                  slab, a docked rail tenant, and a rail of its own; what
                  settled it is that the viewport is the work, and everything
                  else is an instrument laid over it that the human can put
                  away. Bottom-left, clear of the tree's column, closable to a
                  pill. */}
              <div className="pointer-events-none absolute bottom-[7.5rem] left-[15.5rem] right-[15.5rem] top-2 z-20 flex flex-col justify-end">
                <Blackboard />
              </div>

              {/* The timeline is an overlay as well, starting to the RIGHT of
                  the tree's column. It was a full-width strip below the
                  viewport, which is height the 3D view never got back —
                  and the 3D view is the work. Sitting inside means the
                  history is read against the model rather than beneath it. */}
              {/* Transparent, like the tree: an overlay on a CAD viewport should
                  let the model read through it rather than stamping a slab over
                  the geometry. No blur — blur smears the one thing the app
                  exists to show. */}
              <div className="pointer-events-auto absolute bottom-2 left-[15.5rem] right-[15.5rem] z-20 overflow-hidden rounded-lg border border-border/70 bg-card/45">
                <Timeline />
              </div>
            </div>

          </>
        )}
      </div>
      {/* Timeline — horizontal strip, full width. Hidden in Drawing
          mode where the sheet *is* the work product. */}
      <StatusBar />
      {/* Command palette — fixed-position overlay; reachable from
          every workspace via Ctrl/Cmd-K. Mounted unconditionally so
          the keybinding works the moment the app loads, not after
          the first interaction. */}
      <CommandPalette />
      {/* Sign-in dialog — mounted unconditionally but only opens when a
          backend request returns 401 (auth-required signal). Invisible
          against a local insecure-bypass backend. */}
      <LoginDialog />
    </div>
  )
}
