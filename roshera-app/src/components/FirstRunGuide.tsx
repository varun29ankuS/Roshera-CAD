import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { isAuthenticated, isAuthRequired, onAuthChange } from '@/lib/auth'
import { GUIDE_STEPS, guideMayOpen, markGuideSeen, onOpenGuide, shouldShowGuide } from '@/lib/first-run'

function storage(): Pick<Storage, 'getItem' | 'setItem'> {
  try {
    return window.localStorage
  } catch {
    // Storage blocked: behave as a first visit, remember nothing.
    return { getItem: () => null, setItem: () => undefined }
  }
}

/**
 * Getting started: four cards that tell a first-time engineer what to do
 * (lib/first-run.ts has the steps and the rules). Shown once per browser
 * after sign-in, until dismissed; reopenable from the command palette
 * ("Getting started"). A transparent overlay card on the canvas, no blur,
 * per the layout rules; it never opens over the sign-in screen.
 */
export function FirstRunGuide() {
  const decide = () =>
    guideMayOpen({
      authenticated: isAuthenticated(),
      authRequired: isAuthRequired(),
      seen: !shouldShowGuide(storage()),
    })
  const [open, setOpen] = useState(decide)
  const [step, setStep] = useState(0)

  useEffect(
    () =>
      onOpenGuide(() => {
        setStep(0)
        setOpen(true)
      }),
    [],
  )

  // Sign-in state changes after mount: a 401 trips "auth required" (close,
  // the sign-in screen shows) and a sign-in sets the token (open if unseen).
  useEffect(
    () =>
      onAuthChange(() => {
        if (isAuthRequired()) setOpen(false)
        else if (decide()) setOpen(true)
      }),
    // `decide` reads module state only; it has no React dependencies.
    [],
  )

  function close() {
    markGuideSeen(storage())
    setOpen(false)
  }

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        markGuideSeen(storage())
        setOpen(false)
      }
      if (e.key === 'ArrowRight') setStep((s) => Math.min(s + 1, GUIDE_STEPS.length - 1))
      if (e.key === 'ArrowLeft') setStep((s) => Math.max(s - 1, 0))
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open])

  if (!open) return null
  const s = GUIDE_STEPS[step]
  const last = step === GUIDE_STEPS.length - 1

  return (
    <div className="pointer-events-none fixed inset-0 z-[90] flex items-center justify-center p-4">
      <div
        role="dialog"
        aria-modal="false"
        aria-labelledby="guide-title"
        className="pointer-events-auto flex w-full max-w-md flex-col gap-4 rounded-xl border border-border bg-card/95 p-5 shadow-lg"
      >
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-medium uppercase tracking-[0.12em] text-muted-foreground">
            Getting started · {step + 1} of {GUIDE_STEPS.length}
          </span>
          <button
            type="button"
            onClick={close}
            className="rounded px-1.5 text-xs text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            Skip
          </button>
        </div>

        <div className="flex flex-col gap-2">
          <h2 id="guide-title" className="text-lg font-semibold tracking-tight">
            {s.title}
          </h2>
          <p className="text-sm leading-relaxed text-foreground/90">{s.body}</p>
          <p className="text-xs text-muted-foreground">Where: {s.where}</p>
        </div>

        <div className="flex items-center justify-between">
          <div className="flex gap-1.5" aria-hidden="true">
            {GUIDE_STEPS.map((g, i) => (
              <span
                key={g.title}
                className={'h-1.5 rounded-full ' + (i === step ? 'w-5 bg-foreground' : 'w-1.5 bg-muted-foreground/40')}
              />
            ))}
          </div>
          <div className="flex gap-2">
            {step > 0 && (
              <Button variant="ghost" size="sm" onClick={() => setStep(step - 1)}>
                Back
              </Button>
            )}
            <Button size="sm" onClick={() => (last ? close() : setStep(step + 1))}>
              {last ? 'Start' : 'Next'}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}
