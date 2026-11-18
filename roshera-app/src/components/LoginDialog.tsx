import { useEffect, useState } from 'react'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Eye, EyeOff } from '@/components/ui/icons'
import { HeroModel } from '@/components/HeroModel'
import { isAuthRequired, login, onAuthChange } from '@/lib/auth'
import { friendlyLoginError, heroOf, parseShowcase, type ShowcaseItem } from '@/lib/first-run'

/**
 * The sign-in screen.
 *
 * Opens when a backend request comes back 401 (the auth-required signal from
 * `lib/auth`) and covers the app until the person signs in. Varun
 * (2026-11-18) chose the "hero model" design: a real Roshera part filling
 * the screen live in 3D, turning slowly (`HeroModel`, from the mesh listed in
 * /showcase/manifest.json), the logo top-left, what the part is bottom-left,
 * and a small sign-in card floating bottom-right -- labelled fields,
 * failures in plain words (`friendlyLoginError`). No blur. A local dev
 * backend running `ROSHERA_DEV_INSECURE=1` never 401s, so this never appears
 * there.
 */
export function LoginDialog() {
  const [open, setOpen] = useState(isAuthRequired())
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [hero, setHero] = useState<ShowcaseItem | null>(null)

  useEffect(() => {
    return onAuthChange(() => {
      if (isAuthRequired()) setOpen(true)
    })
  }, [])

  useEffect(() => {
    if (!open) return
    let cancelled = false
    fetch('/showcase/manifest.json')
      .then((r) => (r.ok ? r.json() : null))
      .then((json) => {
        if (!cancelled) setHero(heroOf(parseShowcase(json)))
      })
      .catch(() => {
        if (!cancelled) setHero(null)
      })
    return () => {
      cancelled = true
    }
  }, [open])

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

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="signin-title"
      className="fixed inset-0 z-[100] overflow-hidden bg-[radial-gradient(ellipse_at_40%_35%,#2a3038_0%,#15181c_70%)] text-zinc-100"
    >
      {/* The part, live. */}
      <div className="absolute inset-0">
        {hero ? (
          <HeroModel src={hero.src} />
        ) : (
          <div className="flex h-full items-center justify-center">
            <img src="/logo.png" alt="" className="h-24 w-24 opacity-80" draggable={false} />
          </div>
        )}
      </div>

      {/* Logo. */}
      <div className="pointer-events-none absolute left-8 top-7 flex items-center gap-2.5">
        <img src="/logo.png" alt="" width={30} height={30} className="h-[30px] w-[30px]" />
        <span className="text-[15px] font-semibold tracking-tight">Roshera</span>
      </div>

      {/* What you are looking at. */}
      {hero && (
        <div className="pointer-events-none absolute bottom-8 left-8 flex max-w-sm flex-col gap-1">
          <span className="text-[11px] font-medium uppercase tracking-[0.14em] text-zinc-400">
            {hero.title}
          </span>
          {hero.caption && <span className="text-xs leading-relaxed text-zinc-400">{hero.caption}</span>}
        </div>
      )}

      {/* Sign-in card. */}
      <div className="absolute bottom-8 right-8 w-[22rem] max-w-[calc(100vw-4rem)] rounded-xl border border-white/10 bg-zinc-900/85 p-6 shadow-2xl">
        <div className="flex flex-col gap-1.5">
          <h1 id="signin-title" className="text-lg font-semibold tracking-tight">
            Sign in
          </h1>
          <p className="text-xs leading-relaxed text-zinc-400">
            Describe the part. The agent builds it. The kernel checks every step.
          </p>
        </div>
        <form onSubmit={submit} className="mt-5 flex flex-col gap-3.5">
          <div className="flex flex-col gap-1.5">
            <label htmlFor="signin-username" className="text-xs font-medium text-zinc-300">
              Username
            </label>
            <Input
              id="signin-username"
              autoFocus
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="username"
              spellCheck={false}
              className="border-white/15 bg-black/30 text-zinc-100"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor="signin-password" className="text-xs font-medium text-zinc-300">
              Password
            </label>
            {/* A masked field you cannot unmask makes a typo look like a
                wrong password; the toggle is off by default, never stored. */}
            <div className="relative">
              <Input
                id="signin-password"
                type={showPassword ? 'text' : 'password'}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="current-password"
                className="border-white/15 bg-black/30 pr-9 text-zinc-100"
              />
              <button
                type="button"
                onClick={() => setShowPassword((v) => !v)}
                aria-label={showPassword ? 'Hide password' : 'Show password'}
                aria-pressed={showPassword}
                title={showPassword ? 'Hide password' : 'Show password'}
                className="absolute inset-y-0 right-0 flex w-9 items-center justify-center rounded-r-md text-zinc-400 transition-colors hover:text-zinc-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                {showPassword ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
              </button>
            </div>
          </div>
          {error && (
            <p role="alert" className="text-xs leading-relaxed text-red-400">
              {error}
            </p>
          )}
          <Button type="submit" disabled={busy || !username.trim() || !password} className="mt-1 h-9">
            {busy ? 'Signing in…' : 'Sign in'}
          </Button>
        </form>
      </div>
    </div>
  )
}
