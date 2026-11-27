/**
 * The app's own confirm / prompt / alert, in place of the browser's.
 *
 * `window.confirm`, `window.prompt` and `window.alert` freeze the page, wear
 * the browser's chrome instead of the app's, and cannot name a part count or
 * style a destructive action. These three return promises and are drawn by
 * `AppDialogHost` with the app's dialog: Enter confirms, Esc cancels, focus
 * stays inside while it is open.
 *
 * One dialog is on screen at a time; any asked for meanwhile wait in order.
 */

import { create } from 'zustand'

interface DialogBase {
  /** The question or the fact, as one short line. */
  title: string
  /** Detail under the title. Plain text; line breaks are kept. */
  body?: string
}

export interface ConfirmRequest extends DialogBase {
  kind: 'confirm'
  /** The confirming button's words: the action itself ("Delete 3 parts"). */
  confirmLabel: string
  cancelLabel: string
  /** A destructive action: the confirming button is drawn as one. */
  destructive: boolean
}

export interface PromptRequest extends DialogBase {
  kind: 'prompt'
  /** What the field holds, read by assistive tech and shown above it. */
  label: string
  initial: string
  confirmLabel: string
  cancelLabel: string
}

export interface AlertRequest extends DialogBase {
  kind: 'alert'
  confirmLabel: string
}

export type DialogRequest = ConfirmRequest | PromptRequest | AlertRequest

interface Pending {
  request: DialogRequest
  settle: (confirmed: boolean, text?: string) => void
}

interface AppDialogState {
  /** The dialog on screen, or null. */
  current: DialogRequest | null
}

export const useAppDialog = create<AppDialogState>(() => ({ current: null }))

// The queue lives beside the store, not in it: a resolver is not state a
// component should render from.
const queue: Pending[] = []

function show(): void {
  useAppDialog.setState({ current: queue[0]?.request ?? null })
}

function ask(request: DialogRequest, settle: Pending['settle']): void {
  queue.push({ request, settle })
  if (queue.length === 1) show()
}

/** Close the dialog on screen with its answer (the host calls this). */
export function answerDialog(confirmed: boolean, text?: string): void {
  const pending = queue.shift()
  if (!pending) return
  show()
  pending.settle(confirmed, text)
}

/** Ask a yes/no question. Resolves true only when confirmed. */
export function confirmDialog(opts: {
  title: string
  body?: string
  confirmLabel?: string
  cancelLabel?: string
  destructive?: boolean
}): Promise<boolean> {
  return new Promise((resolve) => {
    ask(
      {
        kind: 'confirm',
        title: opts.title,
        body: opts.body,
        confirmLabel: opts.confirmLabel ?? 'OK',
        cancelLabel: opts.cancelLabel ?? 'Cancel',
        destructive: opts.destructive ?? false,
      },
      (confirmed) => resolve(confirmed),
    )
  })
}

/** Ask for one line of text. Resolves the trimmed text, or null when
 *  cancelled or left empty. */
export function promptDialog(opts: {
  title: string
  body?: string
  label?: string
  initial?: string
  confirmLabel?: string
  cancelLabel?: string
}): Promise<string | null> {
  return new Promise((resolve) => {
    ask(
      {
        kind: 'prompt',
        title: opts.title,
        body: opts.body,
        label: opts.label ?? opts.title,
        initial: opts.initial ?? '',
        confirmLabel: opts.confirmLabel ?? 'OK',
        cancelLabel: opts.cancelLabel ?? 'Cancel',
      },
      (confirmed, text) => {
        const trimmed = (text ?? '').trim()
        resolve(confirmed && trimmed.length > 0 ? trimmed : null)
      },
    )
  })
}

/** State a fact that must be acknowledged. Resolves when closed. */
export function alertDialog(opts: { title: string; body?: string; confirmLabel?: string }): Promise<void> {
  return new Promise((resolve) => {
    ask({ kind: 'alert', title: opts.title, body: opts.body, confirmLabel: opts.confirmLabel ?? 'OK' }, () =>
      resolve(),
    )
  })
}
