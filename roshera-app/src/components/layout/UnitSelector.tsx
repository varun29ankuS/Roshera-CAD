/**
 * Compact document-unit selector.
 *
 * Renders a native `<select>` element showing mm · cm · m · in · ft.
 * On change it PATCHes `/api/document/units`; on success it calls
 * `setDocumentUnitState` (bumps `unitEpoch`) so every dimension, label,
 * and pinned-measurement hook re-fires its fetch in the new unit.
 *
 * On a 400 or network error the selection is REVERTED to the last known
 * good value and the backend reason is surfaced verbatim via the chat
 * panel (the house warn pattern — the same surface export/delete failures
 * use; no toast utility exists in this codebase).
 *
 * ## Why a native select rather than Menubar/Menubar-style dropdown
 * The workspace switcher in TopBar uses `Menubar` which is styled as a
 * menu trigger — it is the right pattern for a switching UX with a label
 * ("Workspace: Modeling"). The unit selector is a compact inline picker
 * that sits between controls and should not expand into a full menu; a
 * native `<select>` scoped to the same cad-panel / cad-focus class gives
 * consistent height (h-6) and keyboard behaviour without extra markup.
 *
 * ## No conversion math
 * The selector's only job is PATCH + epoch bump. All formatted labels
 * arrive from the kernel already in the chosen unit. There is zero
 * frontend unit arithmetic here.
 */

import { useState } from 'react'
import { useUnitsStore } from '@/stores/units-store'
import { useChatStore } from '@/stores/chat-store'
import {
  setDocumentUnit,
  UnitSetError,
  UNIT_OPTIONS,
  type UnitToken,
} from '@/lib/units-api'

export function UnitSelector() {
  const documentUnit = useUnitsStore((s) => s.documentUnit)
  const setDocumentUnitState = useUnitsStore((s) => s.setDocumentUnitState)
  const addMessage = useChatStore((s) => s.addMessage)

  // Optimistic local state: update immediately on change, revert on error.
  const [pending, setPending] = useState(false)

  const handleChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const next = e.currentTarget.value as UnitToken
    if (next === documentUnit || pending) return

    setPending(true)
    setDocumentUnit(next)
      .then((confirmed) => {
        setDocumentUnitState(confirmed)
      })
      .catch((err: unknown) => {
        // Revert: documentUnit in the store was NOT changed yet (we only
        // call setDocumentUnitState on success), so the select's value
        // returns to documentUnit on the next render automatically.
        const reason =
          err instanceof UnitSetError
            ? err.reason
            : err instanceof Error
              ? err.message
              : String(err)
        addMessage({
          role: 'assistant',
          content: `Unit change failed: ${reason}`,
          isError: true,
        })
      })
      .finally(() => {
        setPending(false)
      })
  }

  return (
    <select
      value={documentUnit}
      onChange={handleChange}
      disabled={pending}
      title="Document display unit"
      aria-label="Document display unit"
      className={[
        'cad-focus',
        'h-6 px-1.5 rounded border border-border/60',
        'bg-background/40 hover:bg-accent/30',
        'text-[11px] text-muted-foreground',
        'appearance-none cursor-pointer',
        pending ? 'opacity-50 cursor-wait' : '',
      ]
        .filter(Boolean)
        .join(' ')}
    >
      {UNIT_OPTIONS.map((u) => (
        <option key={u} value={u}>
          {u}
        </option>
      ))}
    </select>
  )
}
