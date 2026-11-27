import { useEffect, useState } from 'react'
import { Eye, EyeOff } from '@/components/ui/icons'
import { SignInHero } from '@/components/SignInHero'
import { isAuthRequired, login, onAuthChange } from '@/lib/auth'
import { friendlyLoginError } from '@/lib/first-run'

const API_BASE = import.meta.env.VITE_API_URL || ''

/**
 * The sign-in screen (Varun 2026-11-18).
 *
 * Opens when a backend request comes back 401 (the auth-required signal from
 * `lib/auth`) and covers the app until the person signs in. Paper and ink,
 * after nTop's hero: a stealth fighter concept with a particle wake
 * (`SignInHero`), the headline saying what Roshera does, a parametric
 * callout of the aircraft's own numbers, and a sign-in card. Everything on it
 * is real: the fields sign in, the status line reads the server's /health,
 * and nothing claims a kernel verdict before anything has been checked.
 */
export function LoginDialog() {
  const [open, setOpen] = useState(isAuthRequired())
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [online, setOnline] = useState<boolean | null>(null)

  useEffect(() => {
    return onAuthChange(() => {
      if (isAuthRequired()) setOpen(true)
    })
  }, [])

  // The status line is the server's own answer, checked while the screen is up.
  useEffect(() => {
    if (!open) return
    let live = true
    const check = () =>
      fetch(`${API_BASE}/health`)
        .then((r) => live && setOnline(r.ok))
        .catch(() => live && setOnline(false))
    check()
    const timer = window.setInterval(check, 15000)
    return () => {
      live = false
      window.clearInterval(timer)
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

  const mono = 'font-mono text-[11px] uppercase tracking-[0.08em]'
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="signin-title"
      className="fixed inset-0 z-[100] overflow-hidden bg-background text-foreground"
    >
      <SignInHero />

      {/* Brand. */}
      <div className="pointer-events-none absolute left-9 top-7 flex items-center gap-2.5">
        <img src="/logo.png" alt="" width={30} height={30} className="h-[30px] w-[30px]" />
        <span className="text-[17px] font-semibold tracking-tight">Roshera</span>
      </div>

      {/* The aircraft's own numbers, the way a parametric model shows them. */}
      <div
        aria-hidden="true"
        className={`pointer-events-none absolute right-9 top-28 grid gap-2 rounded-[10px] border border-border bg-background/90 px-4 py-3.5 text-foreground/85 ${mono}`}
      >
        {[
          ['Leading-edge sweep', '42', 'deg'],
          ['Wing section', 'NACA 0004.5', ''],
          ['Tail cant', '27', 'deg'],
        ].map(([label, value, unit]) => (
          <div key={label} className="flex items-center gap-2">
            <i className="inline-block h-[7px] w-[7px] bg-[#1f55d6] dark:bg-[#7ea4ff]" />
            {label}
            <b className="min-w-[2.2em] border border-foreground/60 bg-card/70 px-1.5 py-px text-center font-medium">{value}</b>
            {unit}
          </div>
        ))}
      </div>

      {/* What Roshera does. */}
      <div className="pointer-events-none absolute bottom-14 left-9 flex max-w-[38rem] flex-col gap-4">
        <h2 className="text-[60px] font-semibold leading-[1.02] tracking-[-0.025em]">
          Describe the part.
          <br />
          <span className="text-[#1f55d6] dark:text-[#7ea4ff]">Get a verified model.</span>
        </h2>
        <p className="max-w-[34rem] text-[14px] leading-relaxed text-muted-foreground">
          Tell Roshera what you need in plain words. It builds the 3D part and its drawing, and checks every dimension
          against the geometry before you see it.
        </p>
      </div>

      {/* Sign-in card. */}
      <div className="absolute bottom-9 right-9 w-[380px] max-w-[calc(100vw-4.5rem)] overflow-hidden rounded-2xl border border-border bg-card/95 shadow-[0_30px_60px_-20px_rgba(30,32,28,0.25),0_8px_18px_-10px_rgba(30,32,28,0.15)]">
        <form onSubmit={submit} className="px-6 pb-5 pt-6">
          <h1 id="signin-title" className="flex items-center gap-2.5 text-[20px] font-semibold tracking-tight">
            <img src="/logo.png" alt="" width={22} height={22} className="h-[22px] w-[22px]" />
            Sign in
          </h1>
          <p className="mb-5 mt-1.5 text-[13px] leading-snug text-muted-foreground">
            Welcome back. Your parts, drawings and design history are where you left them.
          </p>

          <label htmlFor="signin-username" className={`mb-1.5 block text-muted-foreground ${mono}`}>
            Username
          </label>
          <div className="mb-3.5 flex h-[42px] items-center gap-2.5 rounded-[10px] border border-border bg-background px-3 transition focus-within:border-[#1f55d6] dark:focus-within:border-[#7ea4ff] focus-within:shadow-[0_0_0_3px_rgba(31,85,214,0.14)]">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true" className="shrink-0 text-muted-foreground">
              <circle cx="12" cy="8" r="4" />
              <path d="M4 21c1.5-4 5-6 8-6s6.5 2 8 6" />
            </svg>
            <input
              id="signin-username"
              autoFocus
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoComplete="username"
              spellCheck={false}
              className="flex-1 bg-transparent text-[14px] outline-none"
            />
          </div>

          <label htmlFor="signin-password" className={`mb-1.5 block text-muted-foreground ${mono}`}>
            Password
          </label>
          {/* A masked field you cannot unmask makes a typo look like a wrong
              password; the toggle is off by default and never stored. */}
          <div className="mb-4 flex h-[42px] items-center gap-2.5 rounded-[10px] border border-border bg-background px-3 transition focus-within:border-[#1f55d6] dark:focus-within:border-[#7ea4ff] focus-within:shadow-[0_0_0_3px_rgba(31,85,214,0.14)]">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" aria-hidden="true" className="shrink-0 text-muted-foreground">
              <rect x="4" y="10" width="16" height="11" rx="2" />
              <path d="M8 10V7a4 4 0 0 1 8 0v3" />
            </svg>
            <input
              id="signin-password"
              type={showPassword ? 'text' : 'password'}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password"
              className="flex-1 bg-transparent text-[14px] outline-none"
            />
            <button
              type="button"
              onClick={() => setShowPassword((v) => !v)}
              aria-label={showPassword ? 'Hide password' : 'Show password'}
              aria-pressed={showPassword}
              title={showPassword ? 'Hide password' : 'Show password'}
              className="grid place-items-center rounded p-1 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[#1f55d6]"
            >
              {showPassword ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
            </button>
          </div>

          {error && (
            <p role="alert" className="mb-3 text-[12.5px] leading-snug text-red-600 dark:text-red-400">
              {error}
            </p>
          )}

          <button
            type="submit"
            disabled={busy || !username.trim() || !password}
            className="flex h-11 w-full items-center justify-center gap-2.5 rounded-[10px] bg-primary text-[14px] font-semibold text-primary-foreground transition hover:opacity-90 active:translate-y-px disabled:opacity-50"
          >
            {busy ? 'Signing in…' : 'Sign in'}
            {!busy && (
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                <path d="M5 12h14M13 6l6 6-6 6" />
              </svg>
            )}
          </button>
          <p className="mt-3 text-center text-[12px] text-muted-foreground">Need an account? Ask your Roshera admin.</p>
        </form>
        <div className="flex items-center justify-between border-t border-border bg-background/60 px-6 py-3 font-mono text-[11px] text-muted-foreground">
          <span className="flex items-center gap-2">
            <span
              className={`inline-block h-[7px] w-[7px] rounded-full ${
                online === null ? 'bg-zinc-400' : online ? 'bg-[#16a36b] shadow-[0_0_0_3px_rgba(22,163,107,0.18)]' : 'bg-red-500'
              }`}
            />
            {online === null ? 'Checking the server…' : online ? 'Kernel online' : 'Server unreachable'}
          </span>
        </div>
      </div>
    </div>
  )
}
