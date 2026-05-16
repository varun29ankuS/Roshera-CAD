import { useCallback } from 'react'
import {
  Menubar,
  MenubarContent,
  MenubarItem,
  MenubarMenu,
  MenubarSeparator,
  MenubarShortcut,
  MenubarTrigger,
  MenubarSub,
  MenubarSubContent,
  MenubarSubTrigger,
} from '@/components/ui/menubar'
import { useSceneStore, CAMERA_PRESETS } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { useThemeStore } from '@/stores/theme-store'
import { useDocModeStore, type DocumentMode } from '@/stores/doc-mode-store'
import { useCommandPaletteStore } from '@/stores/command-palette-store'
import { Badge } from '@/components/ui/badge'
import { Sun, Moon } from 'lucide-react'
import { wsClient } from '@/lib/ws-client'
import { exportSceneAs } from '@/lib/export-api'
import { useChatStore } from '@/stores/chat-store'

const API_BASE = import.meta.env.VITE_API_URL || ''

async function timelineAction(action: 'undo' | 'redo') {
  // Backend `undo_operation` / `redo_operation` 400 without a
  // `session_id` in the JSON body. Pull the live one from the WS
  // store (seeded by the `Welcome` frame in `ws-bridge.ts`); if it
  // hasn't arrived yet there is no session to undo against, so
  // silently no-op rather than hit the backend with a malformed body.
  const sessionId = useWSStore.getState().sessionId
  if (!sessionId) return
  try {
    await fetch(`${API_BASE}/api/timeline/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: sessionId }),
    })
  } catch {
    // backend not running
  }
}

// Two-step export (POST /api/export → GET /api/download/:filename) is
// implemented in `lib/export-api.ts` and shared with the ToolBar Export
// flyout. Both surfaces hit the kernel directly so a missing AI key
// can't 5xx a deterministic export. Both success and failure post a
// chat-panel message so the user always gets visible feedback — a
// silent `console.error` was misread as "click does nothing" because
// the user had no reason to crack open DevTools.
async function exportGeometry(format: string) {
  const { addMessage } = useChatStore.getState()
  addMessage({ role: 'user', content: `Export scene as ${format}` })
  const result = await exportSceneAs(format)
  if (result.ok) {
    addMessage({
      role: 'assistant',
      content: result.filename
        ? `Exported as ${result.filename}.`
        : `Export ready.`,
    })
  } else {
    addMessage({
      role: 'assistant',
      content: `Export failed: ${result.error ?? 'unknown error'}`,
    })
  }
}

// Workspace switcher. Mirrors SolidWorks's File-menu pattern: the user
// is *always* in a workspace; the chip on the right of the TopBar names
// the current one and the dropdown lets you switch. All three modes
// (Modeling / Assembly / Drawing) are exposed now that the Assembly
// workspace ships the mate editor + outliner shell.
const WORKSPACE_LABELS: Record<DocumentMode, string> = {
  part: 'Modeling',
  assembly: 'Assembly',
  drawing: 'Drawing',
}

const WORKSPACE_CHOICES: DocumentMode[] = ['part', 'assembly', 'drawing']

export function TopBar() {
  const clearScene = useSceneStore((s) => s.clearScene)
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const gridSettings = useSceneStore((s) => s.gridSettings)
  const setGridSettings = useSceneStore((s) => s.setGridSettings)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const status = useWSStore((s) => s.status)
  const docMode = useDocModeStore((s) => s.mode) ?? 'part'
  const setDocMode = useDocModeStore((s) => s.setMode)
  const openCommandPalette = useCommandPaletteStore((s) => s.openWith)

  const handleNewProject = useCallback(() => {
    clearScene()
    wsClient.send({ type: 'Command', payload: { cmd: 'NewProject' } })
  }, [clearScene])

  const handleDelete = useCallback(() => {
    // Route through the canonical REST endpoint — the same one
    // ModelTree's context menu uses — so the kernel, timeline, and
    // viewport stay in sync. Local removal arrives via the
    // `ObjectDeleted` broadcast (ws-bridge.ts); the previous
    // implementation sent a `DeleteObject` WS command that had no
    // backend handler.
    const state = useSceneStore.getState()
    const ids = Array.from(state.selectedIds)
    for (const id of ids) {
      void fetch(`${API_BASE}/api/geometry/${id}`, { method: 'DELETE' })
        .then((resp) => {
          if (!resp.ok) {
            return resp.text().catch(() => '').then((text) => {
              console.error('[topbar] delete failed:', resp.status, text)
            })
          }
          return undefined
        })
        .catch((err) => console.error('[topbar] delete error:', err))
    }
  }, [])

  const handleSelectAll = useCallback(() => {
    const state = useSceneStore.getState()
    for (const id of state.objectOrder) {
      state.selectObject(id, true)
    }
  }, [])

  return (
    <div className="flex items-center h-9 cad-panel border-b px-1">
      <div className="flex items-center gap-1.5 px-2">
        <div className="w-4 h-4 rounded-sm bg-primary flex items-center justify-center">
          <span className="text-[8px] font-bold text-primary-foreground">R</span>
        </div>
        <span className="text-xs font-semibold tracking-tight text-foreground">
          Roshera CAD
        </span>
      </div>

      <Menubar className="border-none bg-transparent h-7 px-0">
        <MenubarMenu>
          <MenubarTrigger className="text-xs px-2 py-0.5 h-6">
            File
          </MenubarTrigger>
          <MenubarContent>
            <MenubarItem onClick={handleNewProject}>
              New Project <MenubarShortcut>Ctrl+N</MenubarShortcut>
            </MenubarItem>
            <MenubarSeparator />
            <MenubarSub>
              <MenubarSubTrigger>Export</MenubarSubTrigger>
              <MenubarSubContent>
                <MenubarItem onClick={() => exportGeometry('ROS')}>ROS</MenubarItem>
                <MenubarItem onClick={() => exportGeometry('STEP')}>STEP</MenubarItem>
                <MenubarItem onClick={() => exportGeometry('STL')}>STL</MenubarItem>
                <MenubarItem onClick={() => exportGeometry('OBJ')}>OBJ</MenubarItem>
              </MenubarSubContent>
            </MenubarSub>
            <MenubarSeparator />
            <MenubarItem onClick={clearScene}>Clear Scene</MenubarItem>
            <MenubarSeparator />
            <MenubarItem onClick={() => (window.location.hash = '#/demos')}>
              Demo Gallery
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>

        <MenubarMenu>
          <MenubarTrigger className="text-xs px-2 py-0.5 h-6">
            Edit
          </MenubarTrigger>
          <MenubarContent>
            <MenubarItem onClick={() => timelineAction('undo')}>
              Undo <MenubarShortcut>Ctrl+Z</MenubarShortcut>
            </MenubarItem>
            <MenubarItem onClick={() => timelineAction('redo')}>
              Redo <MenubarShortcut>Ctrl+Shift+Z</MenubarShortcut>
            </MenubarItem>
            <MenubarSeparator />
            <MenubarItem onClick={handleDelete}>
              Delete <MenubarShortcut>Del</MenubarShortcut>
            </MenubarItem>
            <MenubarItem onClick={handleSelectAll}>
              Select All <MenubarShortcut>Ctrl+A</MenubarShortcut>
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>

        <MenubarMenu>
          <MenubarTrigger className="text-xs px-2 py-0.5 h-6">
            View
          </MenubarTrigger>
          <MenubarContent>
            {Object.entries(CAMERA_PRESETS).map(([key, preset]) => (
              <MenubarItem key={key} onClick={() => setCameraPreset(key)}>
                {preset.name}
                {key === 'front' && <MenubarShortcut>1</MenubarShortcut>}
                {key === 'right' && <MenubarShortcut>3</MenubarShortcut>}
                {key === 'top' && <MenubarShortcut>7</MenubarShortcut>}
              </MenubarItem>
            ))}
            <MenubarSeparator />
            <MenubarItem
              onClick={() =>
                setGridSettings({ visible: !gridSettings.visible })
              }
            >
              {gridSettings.visible ? 'Hide' : 'Show'} Grid
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>

        <MenubarMenu>
          <MenubarTrigger className="text-xs px-2 py-0.5 h-6">
            Select
          </MenubarTrigger>
          <MenubarContent>
            <MenubarItem
              onClick={() => setSelectionMode('object')}
              className={selectionMode === 'object' ? 'bg-accent' : ''}
            >
              Object Mode <MenubarShortcut>1</MenubarShortcut>
            </MenubarItem>
            <MenubarItem
              onClick={() => setSelectionMode('face')}
              className={selectionMode === 'face' ? 'bg-accent' : ''}
            >
              Face Mode <MenubarShortcut>2</MenubarShortcut>
            </MenubarItem>
            <MenubarItem
              onClick={() => setSelectionMode('edge')}
              className={selectionMode === 'edge' ? 'bg-accent' : ''}
            >
              Edge Mode <MenubarShortcut>3</MenubarShortcut>
            </MenubarItem>
            <MenubarItem
              onClick={() => setSelectionMode('vertex')}
              className={selectionMode === 'vertex' ? 'bg-accent' : ''}
            >
              Vertex Mode <MenubarShortcut>4</MenubarShortcut>
            </MenubarItem>
          </MenubarContent>
        </MenubarMenu>
      </Menubar>

      <div className="flex-1" />

      <div className="flex items-center gap-2 px-2">
        {/* Command palette trigger. The hotkey lives in
            `lib/shortcuts.ts`; this chip is the visible affordance so
            new users discover it without having to read docs. The
            label uses ⌘ on macOS-class platforms and Ctrl elsewhere
            so the hint matches the actual modifier the OS reports. */}
        <button
          type="button"
          onClick={() => openCommandPalette()}
          title="Command palette (Ctrl/Cmd-K)"
          className="cad-focus inline-flex items-center gap-1.5 h-6 px-2 rounded border border-border/60 bg-background/40 hover:bg-accent/30 text-[11px] text-muted-foreground"
        >
          <span>Commands</span>
          <kbd className="font-mono text-[10px] text-foreground/80 border border-border/60 rounded px-1">
            {typeof navigator !== 'undefined' && /mac/i.test(navigator.platform)
              ? '⌘K'
              : 'Ctrl K'}
          </kbd>
        </button>

        {/* Workspace switcher. Tucked into the right-side TopBar so
            it is reachable from anywhere but never competes with the
            primary File/Edit/View menus for attention. The dropdown
            renders every workspace the app currently ships — Modeling,
            Assembly, and Drawing. */}
        <Menubar className="border-none bg-transparent h-7 px-0">
          <MenubarMenu>
            <MenubarTrigger
              className="text-xs px-2 py-0.5 h-6 gap-1"
              title="Switch workspace"
            >
              <span className="text-muted-foreground">Workspace:</span>
              <span className="font-medium">{WORKSPACE_LABELS[docMode]}</span>
              <span className="text-muted-foreground text-[10px]">▾</span>
            </MenubarTrigger>
            <MenubarContent align="end">
              {WORKSPACE_CHOICES.map((m) => (
                <MenubarItem
                  key={m}
                  onClick={() => setDocMode(m)}
                  className={docMode === m ? 'bg-accent' : ''}
                >
                  {WORKSPACE_LABELS[m]}
                </MenubarItem>
              ))}
            </MenubarContent>
          </MenubarMenu>
        </Menubar>

        <button
          onClick={useThemeStore.getState().toggleTheme}
          className="cad-icon-btn h-6 w-6"
          title="Toggle theme"
          aria-label="Toggle theme"
        >
          {useThemeStore((s) => s.theme) === 'dark' ? <Sun size={14} /> : <Moon size={14} />}
        </button>
        <Badge
          variant={status === 'connected' ? 'default' : 'secondary'}
          className="text-[10px] h-4 px-1.5"
        >
          <span
            className={`inline-block w-1.5 h-1.5 rounded-full mr-1 ${
              status === 'connected'
                ? 'bg-green-400'
                : status === 'connecting'
                  ? 'bg-yellow-400 animate-pulse'
                  : 'bg-red-400'
            }`}
          />
          {status}
        </Badge>
      </div>
    </div>
  )
}
