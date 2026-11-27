import { useEffect, useRef, useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { answerDialog, useAppDialog, type DialogRequest } from '@/lib/app-dialog'

/**
 * Draws the dialog `lib/app-dialog.ts` has on screen. Mounted once, at the
 * app root.
 *
 * Keyboard: the dialog is a form, so Enter confirms from the text field or
 * the confirming button; Esc (and a click outside) cancels; the dialog
 * primitive keeps focus inside while it is open and hands it back after.
 * Focus starts on the text field of a prompt (its text selected, as a rename
 * expects) and on the confirming button otherwise.
 */
export function AppDialogHost() {
  const current = useAppDialog((s) => s.current)
  // Keep the last request while the dialog animates shut, so its words do
  // not vanish a frame before its frame does.
  const [shown, setShown] = useState<DialogRequest | null>(current)
  if (current && current !== shown) setShown(current)

  return (
    <Dialog
      open={current !== null}
      onOpenChange={(open) => {
        if (!open) answerDialog(false)
      }}
    >
      {shown && <DialogBody key={dialogKey(shown)} request={shown} />}
    </Dialog>
  )
}

let serial = 0
const keys = new WeakMap<DialogRequest, number>()
/** A fresh form (and a fresh text field) per request. */
function dialogKey(r: DialogRequest): number {
  let k = keys.get(r)
  if (k === undefined) {
    k = ++serial
    keys.set(r, k)
  }
  return k
}

function DialogBody({ request }: { request: DialogRequest }) {
  const [text, setText] = useState(request.kind === 'prompt' ? request.initial : '')
  const inputRef = useRef<HTMLInputElement>(null)
  const confirmRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    inputRef.current?.select()
  }, [])

  return (
    <DialogContent
      showCloseButton={false}
      initialFocus={request.kind === 'prompt' ? inputRef : confirmRef}
      className="gap-3 sm:max-w-md"
    >
      <form
        className="contents"
        onSubmit={(e) => {
          e.preventDefault()
          answerDialog(true, text)
        }}
      >
        <DialogHeader>
          <DialogTitle className="text-sm leading-snug">{request.title}</DialogTitle>
          {request.body && (
            <DialogDescription className="whitespace-pre-line text-xs leading-relaxed">
              {request.body}
            </DialogDescription>
          )}
        </DialogHeader>
        {request.kind === 'prompt' && (
          <input
            ref={inputRef}
            name="value"
            aria-label={request.label}
            value={text}
            onChange={(e) => setText(e.target.value)}
            // Selected when focus lands, so typing replaces the old name.
            onFocus={(e) => e.currentTarget.select()}
            spellCheck={false}
            autoComplete="off"
            className="h-8 w-full rounded-md border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/40"
          />
        )}
        <DialogFooter className="flex-row justify-end">
          {request.kind !== 'alert' && (
            <Button type="button" variant="outline" size="sm" onClick={() => answerDialog(false)}>
              {request.cancelLabel}
            </Button>
          )}
          <Button
            ref={confirmRef}
            type="submit"
            size="sm"
            variant={request.kind === 'confirm' && request.destructive ? 'destructive' : 'default'}
          >
            {request.confirmLabel}
          </Button>
        </DialogFooter>
      </form>
    </DialogContent>
  )
}
