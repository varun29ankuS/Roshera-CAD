/**
 * Top-level document-mode tab strip.
 *
 * Mirrors SolidWorks / Fusion / Onshape: three top-level workspaces —
 * Part, Assembly, Drawing — each with its own toolset and central
 * pane. The active mode is held in `doc-mode-store` (localStorage +
 * URL hash). When `mode === null` we default to 'part' so the UI is
 * never empty.
 *
 * The strip sits directly under the `TopBar` and above the main
 * flex container. It is purely visual; the actual mode-driven layout
 * swap happens in `App.tsx`.
 */

import { useDocModeStore, type DocumentMode } from '@/stores/doc-mode-store'

interface TabSpec {
  key: DocumentMode
  label: string
  hint: string
}

const TABS: TabSpec[] = [
  { key: 'part', label: 'Part', hint: 'Single-solid modelling' },
  { key: 'assembly', label: 'Assembly', hint: 'Multi-part scene + mates' },
  { key: 'drawing', label: 'Drawing', hint: '2D views from 3D' },
]

export function DocumentModeTabs() {
  const mode = useDocModeStore((s) => s.mode) ?? 'part'
  const setMode = useDocModeStore((s) => s.setMode)

  return (
    <div
      role="tablist"
      aria-label="Document mode"
      className="flex items-center gap-0 px-2 border-b border-border/60 bg-background/80 backdrop-blur-sm h-9 select-none"
    >
      {TABS.map((tab) => {
        const active = mode === tab.key
        return (
          <button
            key={tab.key}
            type="button"
            role="tab"
            aria-selected={active}
            title={tab.hint}
            onClick={() => setMode(tab.key)}
            className={[
              'cad-focus relative h-full px-4 text-xs font-medium transition-colors',
              active
                ? 'text-foreground'
                : 'text-muted-foreground hover:text-foreground',
            ].join(' ')}
          >
            {tab.label}
            {/* Underline accent on the active tab */}
            <span
              aria-hidden="true"
              className={[
                'absolute left-2 right-2 -bottom-px h-0.5 rounded-t-sm',
                active ? 'bg-primary' : 'bg-transparent',
              ].join(' ')}
            />
          </button>
        )
      })}
    </div>
  )
}
