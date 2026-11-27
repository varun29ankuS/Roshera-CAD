import { confirmDialog } from '@/lib/app-dialog'

/** The one question asked before File > New Project (menu and palette). */
export function confirmNewDocument(): Promise<boolean> {
  return confirmDialog({
    title: 'Start a new, empty document?',
    body: 'Your current document is saved and stays open in its tab.',
    confirmLabel: 'New document',
  })
}
