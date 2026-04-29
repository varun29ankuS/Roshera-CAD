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
import { Badge } from '@/components/ui/badge'
import { Sun, Moon } from 'lucide-react'
import { wsClient } from '@/lib/ws-client'

const API_BASE = import.meta.env.VITE_API_URL || ''

async function timelineAction(action: 'undo' | 'redo') {
  try {
    await fetch(`${API_BASE}/api/timeline/${action}`, { method: 'POST' })
  } catch {
    // backend not running
  }
}

async function exportGeometry(format: string) {
  const sessionId = useWSStore.getState().sessionId || 'default-session'
  const selectedIds = Array.from(useSceneStore.getState().selectedIds)
  try {
    const resp = await fetch(`${API_BASE}/api/export`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        format,
        session_id: sessionId,
        objects: selectedIds.length > 0 ? selectedIds : undefined,
        options: { binary: true, include_materials: true, merge_objects: false },
      }),
    })
    if (!resp.ok) return
    const blob = await resp.blob()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `export.${format.toLowerCase()}`
    a.click()
    URL.revokeObjectURL(url)
  } catch {
    // backend not running
  }
}

export function TopBar() {
  const clearScene = useSceneStore((s) => s.clearScene)
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const gridSettings = useSceneStore((s) => s.gridSettings)
  const setGridSettings = useSceneStore((s) => s.setGridSettings)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const status = useWSStore((s) => s.status)

  const handleNewProject = useCallback(() => {
    clearScene()
    wsClient.send({ type: 'Command', payload: { cmd: 'NewProject' } })
  }, [clearScene])

  const handleDelete = useCallback(() => {
    const state = useSceneStore.getState()
    for (const id of state.selectedIds) {
      wsClient.send({ type: 'Command', payload: { cmd: 'DeleteObject', object_id: id } })
      state.removeObject(id)
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
                <MenubarItem onClick={() => exportGeometry('glTF')}>glTF</MenubarItem>
                <MenubarItem onClick={() => exportGeometry('IGES')}>IGES</MenubarItem>
                <MenubarItem onClick={() => exportGeometry('FBX')}>FBX</MenubarItem>
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
