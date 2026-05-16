/**
 * Command palette.
 *
 * Ctrl/Cmd-K opens a centred overlay with a search input and a
 * categorised, fuzzy-matched list of every action the user can take
 * from the keyboard or menus. Mirrors the VS Code / Figma / Linear
 * pattern — one keybinding to do anything.
 *
 * Design notes:
 * - Commands are *derived from existing stores at render time*, not
 *   maintained as a separate registry. The TopBar / ToolBar / sketch
 *   panel each own their canonical actions; this surface only wraps
 *   them. That guarantees the palette never goes stale relative to
 *   the menubar.
 * - Filtering is a small in-house scorer (substring-prefix > substring
 *   > subsequence). Good enough for ~50 commands; bringing in `cmdk`
 *   or `fzf-for-js` would dwarf this entire file.
 * - Keyboard model: ↑/↓ to move, Enter to invoke, Esc to close. Tab
 *   does *not* move focus inside the palette — it cycles back to the
 *   input so the user can immediately keep typing after a wrong nav.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Search } from 'lucide-react'
import { useCommandPaletteStore } from '@/stores/command-palette-store'
import { useDocModeStore, type DocumentMode } from '@/stores/doc-mode-store'
import { useSceneStore, CAMERA_PRESETS } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { useThemeStore } from '@/stores/theme-store'
import { useChatStore } from '@/stores/chat-store'
import { wsClient } from '@/lib/ws-client'
import { exportSceneAs } from '@/lib/export-api'

const API_BASE = import.meta.env.VITE_API_URL || ''

// ── Types ───────────────────────────────────────────────────────────

interface Command {
  /** Stable identifier — used as React key + telemetry id. */
  id: string
  /** Title shown in the palette row. */
  label: string
  /** Category header. Commands with the same group render together. */
  group: string
  /** Secondary keyboard shortcut hint, displayed right-aligned. */
  hint?: string
  /** Synonyms / extra search tokens that don't fit in the label. */
  keywords?: string[]
  /** Fired when the user picks this row. Runs after the palette closes. */
  run: () => void
}

interface ScoredCommand {
  cmd: Command
  score: number
}

// ── Scoring ─────────────────────────────────────────────────────────

/**
 * Score `haystack` against `needle`. Higher is better; 0 means "no
 * match". Tuned so that prefix matches dominate substring matches,
 * which dominate subsequence matches — predictable ordering with
 * tiny code.
 */
function score(haystack: string, needle: string): number {
  if (!needle) return 1
  const h = haystack.toLowerCase()
  const n = needle.toLowerCase()
  if (h.startsWith(n)) return 1000 + (n.length / h.length) * 100
  const idx = h.indexOf(n)
  if (idx >= 0) return 500 + (n.length / h.length) * 100 - idx
  // Subsequence: every char of `needle` appears in `haystack` in order.
  let hi = 0
  for (let ni = 0; ni < n.length; ni++) {
    while (hi < h.length && h[hi] !== n[ni]) hi++
    if (hi >= h.length) return 0
    hi++
  }
  return 100
}

function rank(cmd: Command, query: string): number {
  if (!query) return 1
  const labelScore = score(cmd.label, query)
  const groupScore = score(cmd.group, query) * 0.4
  const keywordScore = cmd.keywords
    ? Math.max(...cmd.keywords.map((k) => score(k, query))) * 0.6
    : 0
  return Math.max(labelScore, groupScore, keywordScore)
}

// ── Side-effect helpers (mirrors of TopBar's local handlers) ────────
//
// Duplicating the bodies here is deliberate: TopBar's `timelineAction`
// is a module-local closure over `useWSStore.getState()`; importing it
// would mean exporting it, which would bloat TopBar's public surface
// for one consumer. The bodies are 6 lines each.

async function timelineAction(action: 'undo' | 'redo'): Promise<void> {
  const sessionId = useWSStore.getState().sessionId
  if (!sessionId) return
  try {
    await fetch(`${API_BASE}/api/timeline/${action}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ session_id: sessionId }),
    })
  } catch {
    /* backend not running — no-op */
  }
}

async function runExport(format: string): Promise<void> {
  const { addMessage } = useChatStore.getState()
  addMessage({ role: 'user', content: `Export scene as ${format}` })
  const result = await exportSceneAs(format)
  addMessage({
    role: 'assistant',
    content: result.ok
      ? result.filename
        ? `Exported as ${result.filename}.`
        : 'Export ready.'
      : `Export failed: ${result.error ?? 'unknown error'}`,
  })
}

function deleteSelected(): void {
  const state = useSceneStore.getState()
  for (const id of Array.from(state.selectedIds)) {
    void fetch(`${API_BASE}/api/geometry/${id}`, { method: 'DELETE' }).catch(
      (err) => console.error('[command-palette] delete error:', err),
    )
  }
}

// ── Component ───────────────────────────────────────────────────────

export function CommandPalette() {
  const open = useCommandPaletteStore((s) => s.open)
  const query = useCommandPaletteStore((s) => s.query)
  const setQuery = useCommandPaletteStore((s) => s.setQuery)
  const close = useCommandPaletteStore((s) => s.close)

  // Whatever the palette dispatches happens against the live stores.
  // We read setters here (not state, to keep the selectors stable so
  // the palette doesn't re-render on every viewport tick).
  const setDocMode = useDocModeStore((s) => s.setMode)
  const clearScene = useSceneStore((s) => s.clearScene)
  const setCameraPreset = useSceneStore((s) => s.setCameraPreset)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const setActiveTool = useSceneStore((s) => s.setActiveTool)
  const setGridSettings = useSceneStore((s) => s.setGridSettings)
  const toggleTheme = useThemeStore((s) => s.toggleTheme)
  const gridVisible = useSceneStore((s) => s.gridSettings.visible)

  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const [activeIndex, setActiveIndex] = useState(0)

  // ── Command catalogue ────────────────────────────────────────────
  const commands: Command[] = useMemo(() => {
    const cmds: Command[] = []

    // Workspace switch
    const workspaceLabels: Record<DocumentMode, string> = {
      part: 'Modeling',
      drawing: 'Drawing',
      assembly: 'Assembly',
    }
    for (const mode of ['part', 'drawing'] as DocumentMode[]) {
      cmds.push({
        id: `workspace.${mode}`,
        label: `Switch to ${workspaceLabels[mode]}`,
        group: 'Workspace',
        keywords: ['mode', 'workspace', mode],
        run: () => setDocMode(mode),
      })
    }

    // File
    cmds.push({
      id: 'file.new',
      label: 'New Project',
      group: 'File',
      hint: 'Ctrl+N',
      keywords: ['clear', 'reset', 'fresh'],
      run: () => {
        clearScene()
        wsClient.send({ type: 'Command', payload: { cmd: 'NewProject' } })
      },
    })
    cmds.push({
      id: 'file.clear-scene',
      label: 'Clear Scene',
      group: 'File',
      keywords: ['delete all', 'reset'],
      run: () => clearScene(),
    })
    cmds.push({
      id: 'file.demos',
      label: 'Open Demo Gallery',
      group: 'File',
      keywords: ['examples', 'samples', 'demo'],
      run: () => {
        window.location.hash = '#/demos'
      },
    })
    for (const fmt of ['ROS', 'STEP', 'STL', 'OBJ']) {
      cmds.push({
        id: `file.export.${fmt.toLowerCase()}`,
        label: `Export as ${fmt}`,
        group: 'File',
        keywords: ['save', 'download', fmt.toLowerCase()],
        run: () => {
          void runExport(fmt)
        },
      })
    }

    // Edit
    cmds.push({
      id: 'edit.undo',
      label: 'Undo',
      group: 'Edit',
      hint: 'Ctrl+Z',
      run: () => {
        void timelineAction('undo')
      },
    })
    cmds.push({
      id: 'edit.redo',
      label: 'Redo',
      group: 'Edit',
      hint: 'Ctrl+Shift+Z',
      run: () => {
        void timelineAction('redo')
      },
    })
    cmds.push({
      id: 'edit.delete',
      label: 'Delete Selected',
      group: 'Edit',
      hint: 'Del',
      keywords: ['remove'],
      run: () => deleteSelected(),
    })
    cmds.push({
      id: 'edit.select-all',
      label: 'Select All',
      group: 'Edit',
      hint: 'Ctrl+A',
      run: () => {
        const s = useSceneStore.getState()
        for (const id of s.objectOrder) s.selectObject(id, true)
      },
    })

    // View — camera presets
    for (const [key, preset] of Object.entries(CAMERA_PRESETS)) {
      cmds.push({
        id: `view.camera.${key}`,
        label: `View: ${preset.name}`,
        group: 'View',
        keywords: ['camera', 'orient', key],
        run: () => setCameraPreset(key),
      })
    }
    cmds.push({
      id: 'view.grid.toggle',
      label: `${gridVisible ? 'Hide' : 'Show'} Grid`,
      group: 'View',
      keywords: ['grid', 'toggle'],
      run: () => setGridSettings({ visible: !gridVisible }),
    })

    // Selection modes
    for (const [mode, hint] of [
      ['object', '1'],
      ['face', '2'],
      ['edge', '3'],
      ['vertex', '4'],
    ] as const) {
      cmds.push({
        id: `select.${mode}`,
        label: `Selection: ${mode[0].toUpperCase() + mode.slice(1)} Mode`,
        group: 'Selection',
        hint,
        run: () => setSelectionMode(mode),
      })
    }

    // Transform tools
    for (const [tool, hint] of [
      ['select', 'V'],
      ['translate', 'G'],
      ['rotate', 'R'],
      ['scale', 'S'],
    ] as const) {
      cmds.push({
        id: `tool.${tool}`,
        label: `Tool: ${tool[0].toUpperCase() + tool.slice(1)}`,
        group: 'Tools',
        hint,
        run: () => setActiveTool(tool),
      })
    }

    // Theme
    cmds.push({
      id: 'theme.toggle',
      label: 'Toggle Theme',
      group: 'Appearance',
      keywords: ['dark', 'light', 'colors'],
      run: () => toggleTheme(),
    })

    return cmds
  }, [
    setDocMode,
    clearScene,
    setCameraPreset,
    setSelectionMode,
    setActiveTool,
    setGridSettings,
    toggleTheme,
    gridVisible,
  ])

  // ── Filter + group ───────────────────────────────────────────────
  const filtered: ScoredCommand[] = useMemo(() => {
    const scored = commands
      .map((cmd) => ({ cmd, score: rank(cmd, query.trim()) }))
      .filter((s) => s.score > 0)
    scored.sort((a, b) => b.score - a.score)
    return scored.slice(0, 50)
  }, [commands, query])

  // Keep the highlight inside the visible range as the user types.
  useEffect(() => {
    setActiveIndex(0)
  }, [query])

  // ── Open / focus management ──────────────────────────────────────
  useEffect(() => {
    if (open) {
      // Defer focus to next frame so the input is mounted.
      const t = window.requestAnimationFrame(() => {
        inputRef.current?.focus()
        inputRef.current?.select()
      })
      return () => window.cancelAnimationFrame(t)
    }
    return undefined
  }, [open])

  // Scroll the active row into view on arrow-nav.
  useEffect(() => {
    if (!open || !listRef.current) return
    const row = listRef.current.querySelector<HTMLElement>(
      `[data-cmd-index="${activeIndex}"]`,
    )
    if (row) {
      row.scrollIntoView({ block: 'nearest' })
    }
  }, [activeIndex, open])

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
        return
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setActiveIndex((i) => Math.min(filtered.length - 1, i + 1))
        return
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault()
        setActiveIndex((i) => Math.max(0, i - 1))
        return
      }
      if (e.key === 'Home') {
        e.preventDefault()
        setActiveIndex(0)
        return
      }
      if (e.key === 'End') {
        e.preventDefault()
        setActiveIndex(filtered.length - 1)
        return
      }
      if (e.key === 'Enter') {
        e.preventDefault()
        const picked = filtered[activeIndex]
        if (picked) {
          close()
          // Defer execution one tick so any focus-side-effects in the
          // command (e.g. opening another modal) don't race with the
          // palette's own close-and-cleanup.
          window.setTimeout(() => picked.cmd.run(), 0)
        }
      }
    },
    [filtered, activeIndex, close],
  )

  if (!open) return null

  return (
    // Backdrop. Click to dismiss; stops mouse events from reaching the
    // viewport behind so the user can't accidentally pick a face while
    // the palette is up.
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Command palette"
      onKeyDown={onKeyDown}
      className="fixed inset-0 z-50 flex items-start justify-center pt-[15vh] bg-background/60 backdrop-blur-sm"
      onMouseDown={(e) => {
        // Mouse-down on backdrop closes; mouse-down on the panel
        // bubbles up here, so check the target identity.
        if (e.target === e.currentTarget) close()
      }}
    >
      <div className="w-[min(640px,90vw)] max-h-[60vh] flex flex-col rounded-lg border border-border bg-card shadow-2xl overflow-hidden">
        {/* Input row */}
        <div className="flex items-center gap-2 px-3 py-2 border-b border-border/60">
          <Search size={14} className="text-muted-foreground shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Type a command…"
            spellCheck={false}
            className="cad-focus flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
          <kbd className="text-[10px] font-mono text-muted-foreground border border-border/60 rounded px-1.5 py-0.5">
            Esc
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="flex-1 min-h-0 overflow-y-auto py-1">
          {filtered.length === 0 ? (
            <div className="px-3 py-6 text-xs text-muted-foreground text-center">
              No commands match “{query}”.
            </div>
          ) : (
            (() => {
              // Render with sticky group headers, in score order. We
              // render the header the first time a new group appears.
              const seenGroup = new Set<string>()
              return filtered.map((s, i) => {
                const header = !seenGroup.has(s.cmd.group)
                seenGroup.add(s.cmd.group)
                const isActive = i === activeIndex
                return (
                  <div key={s.cmd.id}>
                    {header && (
                      <div className="px-3 pt-2 pb-0.5 text-[10px] uppercase tracking-wider text-muted-foreground">
                        {s.cmd.group}
                      </div>
                    )}
                    <button
                      type="button"
                      data-cmd-index={i}
                      onMouseEnter={() => setActiveIndex(i)}
                      onClick={() => {
                        close()
                        window.setTimeout(() => s.cmd.run(), 0)
                      }}
                      className={[
                        'w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left text-xs',
                        isActive
                          ? 'bg-accent text-accent-foreground'
                          : 'text-foreground hover:bg-accent/40',
                      ].join(' ')}
                    >
                      <span className="truncate">{s.cmd.label}</span>
                      {s.cmd.hint && (
                        <kbd className="ml-2 text-[10px] font-mono text-muted-foreground border border-border/60 rounded px-1 py-0.5 shrink-0">
                          {s.cmd.hint}
                        </kbd>
                      )}
                    </button>
                  </div>
                )
              })
            })()
          )}
        </div>

        {/* Footer hints */}
        <div className="flex items-center justify-between gap-3 px-3 py-1.5 border-t border-border/60 text-[10px] text-muted-foreground">
          <span>
            <kbd className="font-mono border border-border/60 rounded px-1">↑</kbd>{' '}
            <kbd className="font-mono border border-border/60 rounded px-1">↓</kbd>{' '}
            navigate
          </span>
          <span>
            <kbd className="font-mono border border-border/60 rounded px-1">↵</kbd> run
          </span>
          <span>
            <kbd className="font-mono border border-border/60 rounded px-1">Esc</kbd>{' '}
            close
          </span>
        </div>
      </div>

    </div>
  )
}
