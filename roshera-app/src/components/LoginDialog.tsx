import { useEffect, useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Eye, EyeOff } from 'lucide-react'
import { isAuthRequired, login, onAuthChange } from '@/lib/auth'

/**
 * Minimal sign-in dialog (Auth Slice 1).
 *
 * Opens when a backend request has come back 401 (the auth-required
 * signal from `lib/auth`). Collects a username + password, calls
 * `POST /api/auth/login`, and on success stores the token — after which
 * the fetch interceptor authenticates every subsequent request and the
 * WebSocket re-authenticates on its next connect.
 *
 * Deliberately minimal: no registration, no password reset, no "remember
 * me". A local dev backend running `ROSHERA_DEV_INSECURE=1` never 401s,
 * so this dialog never appears there.
 */
export function LoginDialog() {
  const [open, setOpen] = useState(isAuthRequired())
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [showPassword, setShowPassword] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    // Open whenever the auth-required signal trips.
    return onAuthChange(() => {
      if (isAuthRequired()) setOpen(true)
    })
  }, [])

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setBusy(true)
    setError(null)
    const result = await login(username, password)
    setBusy(false)
    if (result.success) {
      setOpen(false)
      setPassword('')
      // Reload so every panel refetches with the credential attached and
      // the WebSocket reconnects and re-authenticates cleanly.
      window.location.reload()
    } else {
      setError(result.error ?? 'Login failed')
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Sign in to Roshera</DialogTitle>
          <DialogDescription>
            This instance requires authentication. Enter your credentials to continue.
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={submit} className="flex flex-col gap-3">
          <Input
            autoFocus
            placeholder="Username"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoComplete="username"
          />
          {/* A masked field you cannot unmask makes a typo indistinguishable
              from a wrong password: both come back "Invalid credentials", and
              the only recourse is to retype blind. The toggle is the standard
              affordance and costs nothing — it is off by default, and the
              revealed state is never persisted anywhere. */}
          <div className="relative">
            <Input
              type={showPassword ? 'text' : 'password'}
              placeholder="Password"
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
              className="absolute inset-y-0 right-0 flex w-9 items-center justify-center text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring rounded-r-md"
            >
              {showPassword ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
            </button>
          </div>
          {error && <p className="text-sm text-red-500">{error}</p>}
          <DialogFooter>
            <Button type="submit" disabled={busy || !username || !password}>
              {busy ? 'Signing in…' : 'Sign in'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
