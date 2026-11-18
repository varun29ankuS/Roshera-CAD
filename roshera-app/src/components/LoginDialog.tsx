import { useEffect, useState } from 'react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Eye, EyeOff } from '@/components/ui/icons'
import { isAuthRequired, login, onAuthChange } from '@/lib/auth'
import {
  friendlyLoginError,
  parseShowcase,
  SHOWCASE_LABEL,
  type ShowcaseItem,
} from '@/lib/first-run'

/**
 * The sign-in screen.
 *
 * Opens when a backend request comes back 401 (the auth-required signal from
 * `lib/auth`) and covers the app until the person signs in. Varun
 * (2026-11-18): it "has to be elegant and showcase some assemblies /
 * drawings / parts ... with roshera logo". So it is a full page, not a bare
 * dialog: on the left, real Roshera output from /showcase/manifest.json
 * (part and assembly renders, certified drawing sheets), turning every few
 * seconds; on the right, the logo, one line on what Roshera is, and the
 * sign-in form with labelled fields and failures said in plain words
 * (`friendlyLoginError`). A local dev backend running `ROSHERA_DEV_INSECURE=1`
 * never 401s, so this never appears there.
 */
export function LoginDialog() {
  const [open, setOpen] = useState(isAuthRequired())
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [showcase, setShowcase] = useState<ShowcaseItem[]>([])
  const [shown, setShown] = useState(0)

  useEffect(() => {
    return onAuthChange(() => {
      if (isAuthRequired()) setOpen(true)
    })
  }, [])

  // The showcase is static content beside the app, read once when the
  // screen opens. A missing or malformed manifest leaves the left side as
  // the plain brand panel.
  useEffect(() => {
    if (!open) return
    let cancelled = false
    fetch('/showcase/manifest.json')
      .then((r) => (r.ok ? r.json() : null))
      .then((json) => {
        if (!cancelled) setShowcase(parseShowcase(json))
      })
      .catch(() => {
        if (!cancelled) setShowcase([])
      })
    return () => {
      cancelled = true
    }
  }, [open])

  useEffect(() => {
    if (!open || showcase.length < 2) return
    const reduce = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    if (reduce) return
    const t = setInterval(() => setShown((i) => (i + 1) % showcase.length), 6000)
    return () => clearInterval(t)
  }, [open, showcase.length])

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError(null)
    const result = await login(username.trim(), password)
    setBusy(false)
    if (result.success) {
      setOpen(false)
      setPassword('')
      // Reload so every panel refetches with the credential attached and the
      // WebSocket reconnects and re-authenticates cleanly.
      window.location.reload()
    } else {
      setError(friendlyLoginError(result.error ?? ''))
    }
  }

  if (!open) return null
  const current = showcase[shown % Math.max(showcase.length, 1)]

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="signin-title"
      className="fixed inset-0 z-[100] flex bg-background text-foreground"
    >
      {/* Showcase: what Roshera makes. */}
      <section className="relative hidden flex-1 overflow-hidden border-r border-border bg-muted/40 md:flex md:flex-col">
        <div className="relative min-h-0 flex-1">
          {showcase.map((item, i) => (
            <img
              key={item.src}
              src={item.src}
              alt={`${SHOWCASE_LABEL[item.kind]}: ${item.title}`}
              className="absolute inset-0 h-full w-full object-contain p-10 transition-opacity duration-700"
              style={{ opacity: i === shown % showcase.length ? 1 : 0 }}
              draggable={false}
            />
          ))}
          {showcase.length === 0 && (
            <div className="flex h-full items-center justify-center">
              <img src="/logo.png" alt="" className="h-28 w-28 opacity-80" draggable={false} />
            </div>
          )}
        </div>
        {current && (
          <div className="flex items-end justify-between gap-4 px-10 pb-8">
            <div className="flex min-w-0 flex-col gap-1">
              <span className="text-[11px] font-medium uppercase tracking-[0.12em] text-muted-foreground">
                {SHOWCASE_LABEL[current.kind]}
              </span>
              <span className="truncate text-base font-semibold">{current.title}</span>
              {current.caption && (
                <span className="text-xs text-muted-foreground">{current.caption}</span>
              )}
            </div>
            {showcase.length > 1 && (
              <div className="flex shrink-0 gap-1.5" role="tablist" aria-label="Showcase">
                {showcase.map((item, i) => (
                  <button
                    key={item.src}
                    type="button"
                    role="tab"
                    aria-selected={i === shown % showcase.length}
                    aria-label={`${SHOWCASE_LABEL[item.kind]}: ${item.title}`}
                    onClick={() => setShown(i)}
                    className={
                      'h-1.5 rounded-full transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ' +
                      (i === shown % showcase.length ? 'w-6 bg-foreground' : 'w-1.5 bg-muted-foreground/40 hover:bg-muted-foreground')
                    }
                  />
                ))}
              </div>
            )}
          </div>
        )}
      </section>

      {/* Sign-in. */}
      <section className="flex w-full flex-col justify-between px-8 py-10 md:w-[26rem] md:shrink-0">
        <div className="flex items-center gap-2.5">
          <img src="/logo.png" alt="" width={28} height={28} className="h-7 w-7" />
          <span className="text-sm font-semibold tracking-tight">Roshera</span>
        </div>

        <div className="flex flex-col gap-7">
          <div className="flex flex-col gap-2">
            <h1 id="signin-title" className="text-2xl font-semibold tracking-tight">
              Sign in
            </h1>
            <p className="text-sm leading-relaxed text-muted-foreground">
              Describe the part. The agent builds it. The kernel checks every step and says
              whether the result is sound.
            </p>
          </div>

          <form onSubmit={submit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="signin-username" className="text-xs font-medium">
                Username
              </label>
              <Input
                id="signin-username"
                autoFocus
                value={username}
                onChange={(e) => setUsername(e.target.value)}
                autoComplete="username"
                spellCheck={false}
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor="signin-password" className="text-xs font-medium">
                Password
              </label>
              {/* A masked field you cannot unmask makes a typo look like a
                  wrong password; the toggle is off by default and never
                  persisted. */}
              <div className="relative">
                <Input
                  id="signin-password"
                  type={showPassword ? 'text' : 'password'}
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  autoComplete="current-password"
                  className="pr-9"
                />
                <button
                  type="button"
                  onClick={() => setShowPassword((v) => !v)}
                  aria-label={showPassword ? 'Hide password' : 'Show password'}
                  aria-pressed={showPassword}
                  title={showPassword ? 'Hide password' : 'Show password'}
                  className="absolute inset-y-0 right-0 flex w-9 items-center justify-center rounded-r-md text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                >
                  {showPassword ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                </button>
              </div>
            </div>
            {error && (
              <p role="alert" className="text-xs leading-relaxed text-red-500">
                {error}
              </p>
            )}
            <Button type="submit" disabled={busy || !username.trim() || !password} className="mt-1 h-10">
              {busy ? 'Signing in…' : 'Sign in'}
            </Button>
          </form>
        </div>

        <p className="text-[11px] leading-relaxed text-muted-foreground">
          Your parts, drawings and history are saved on this server. New here? After you sign
          in, a short guide shows where everything is.
        </p>
      </section>
    </div>
  )
}
