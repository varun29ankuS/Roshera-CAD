import { useEffect, useRef, useState, useCallback } from 'react'
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
import { RailSplitter, useRailSplit } from '@/components/layout/RailSplitter'
import { useKeyboardShortcuts } from '@/lib/shortcuts'
import { useBlackboardStore } from '@/stores/blackboard-store'
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
  // Rail tenancy. The right rail has two widths, and which one it is depends
  // on whether anybody needs the room — not on how much content happens to
  // exist. Content-driven width resizes the canvas while the agent is
  // streaming, which moves the model under the cursor mid-read.
  const agentBusy = useBlackboardStore((s) => s.isProcessing)
  const boardOpen = useBlackboardStore((s) => s.isPanelOpen)
  const railWants = hasSelection || agentBusy || boardOpen
  const [railWide, setRailWide] = useState(railWants)
  const railRef = useRef<HTMLDivElement>(null)
  const split = useRailSplit(railRef)
  // A dragged height applies only while Agent Eye is actually showing.
  // Forcing it on a collapsed panel would render a tall empty card.
  const [eyeMinimized, setEyeMinimized] = useState(false)
  useEffect(() => {
    if (railWants) {
      setRailWide(true)
      return
    }
    // Closing is delayed; opening is not. A quick select/deselect would
    // otherwise thrash the viewport's width twice in a few hundred ms.
    const t = setTimeout(() => setRailWide(false), 500)
    return () => clearTimeout(t)
  }, [railWants])
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

            {/* Left rail — DOCKED, like the right one. It was
                `absolute top-2 bottom-2 left-2 z-10`, a 224×670 card sitting
                ON the canvas, and it had no closed state at all, so it covered
                the model permanently. That is the same objection that moved the
                Blackboard off the viewport; the symmetry was only half true
                while this stayed a floating card pretending to be a rail.

                Docking does not newly cost the viewport anything — those pixels
                were already hidden whenever the tree was open. What it adds is
                the ability to give them back: collapsed, the rail is a 40px
                strip rather than a 224px overlay, which is strictly more canvas
                than the floating version could ever return. */}
            <div
              className={cn(
                'flex shrink-0 flex-col overflow-hidden border-r border-border bg-card transition-[width] duration-200',
                browserOpen ? 'w-56' : 'w-10',
              )}
            >
              <ModelTree
                expanded={browserOpen}
                onToggle={() => setBrowserOpen((open) => !open)}
              />
            </div>

            <div className="relative flex-1 overflow-hidden">
              <CADViewport />
              <StepImportDropzone />
            </div>

            {/* Right dock — ONE column, ONE hairline, three tenants in a fixed
                vertical order. Left rail is the structure of the MODEL, this
                is the structure of the DIALOGUE, and the viewport is an
                unobstructed bench between them.

                The order is load-bearing rather than cosmetic. Streamed agent
                output lands at the Blackboard's BOTTOM edge, and that edge
                stays anchored just above Agent Eye whether or not Properties
                is present above it — so selecting something mid-run inserts a
                panel at the top and compresses the board downward, and the
                live text never jumps.

                Stacked, not tabbed. Agent Eye is an ambient instrument whose
                whole job is being glanceable mid-run; a tab demotes it to
                pull-to-view at exactly the moment it matters, and a selection
                made during a stream needs both tenants live at once, which
                tabs serialise. */}
            <div
              ref={railRef}
              className={cn(
                'flex shrink-0 flex-col border-l border-border bg-card transition-[width] duration-200',
                railWide ? 'w-[560px]' : 'w-56',
              )}
            >
              {hasSelection && (
                <div className="flex max-h-[280px] min-h-0 shrink-0 flex-col overflow-hidden border-b border-border/40">
                  <PropertiesPanel />
                </div>
              )}
              <Blackboard />
              <RailSplitter dragging={split.dragging} {...split.handleProps} />
              {/* An explicit height, so the splitter can GROW this panel and
                  not merely cap it — `maxHeight` could only ever shrink it,
                  which made dragging upward do nothing. Suppressed while Agent
                  Eye is collapsed, where a fixed height would be a tall empty
                  card. */}
              <div
                className="flex shrink-0 flex-col overflow-hidden"
                style={
                  split.height === null || eyeMinimized
                    ? undefined
                    : { height: split.height }
                }
              >
                <AgentEyePanel onMinimizedChange={setEyeMinimized} />
              </div>
            </div>
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
      {/* Sign-in dialog — mounted unconditionally but only opens when a
          backend request returns 401 (auth-required signal). Invisible
          against a local insecure-bypass backend. */}
      <LoginDialog />
    </div>
  )
}
