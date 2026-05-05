/**
 * Create-datum modal for Slice 4a.
 *
 * Three kinds, picked via the segmented control at the top:
 *   - Plane: name + origin (x/y/z) + normal vector. The kernel API takes a
 *     row-major 4×4 transform whose local +Z is the normal; we synthesise
 *     orthogonal x/y tangents on the fly so the user only has to specify
 *     the conceptually meaningful "where" + "facing".
 *   - Axis:  name + origin + canonical direction (x/y/z radio). Slice 4a
 *     intentionally restricts user-axes to canonical directions; arbitrary
 *     directions belong to Slice 4b's derived datums (TwoPointsAxis,
 *     NormalAxis), and the renderer in `Datums.tsx` keys on these three
 *     anyway.
 *   - Point: name + position.
 *
 * The dialog calls `POST /api/datums` with the request body documented in
 * `api-server::handlers::datums::CreateDatumRequest` and invokes
 * `onCreated()` on success — the parent re-fetches the datum list so the
 * tree updates without waiting for the 5 s poll.
 */

import { useCallback, useState } from 'react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'

const API_BASE = import.meta.env.VITE_API_URL || ''

type DatumKindUi = 'plane' | 'axis' | 'point'

type Vec3 = [number, number, number]

interface CreateDatumDialogProps {
  open: boolean
  onOpenChange: (next: boolean) => void
  /** Called after a successful POST /api/datums so the caller can refresh. */
  onCreated: () => void
}

/**
 * Build a row-major 4×4 transform whose local +Z axis is the supplied
 * `normal` and whose translation column is `origin`. Local +X / +Y are
 * synthesised orthogonally using a stable reference vector (world +Y when
 * the normal isn't near-parallel to it, world +X otherwise) — same recipe
 * the kernel's anchoring code uses, kept consistent so a user-authored
 * "plane facing world +Z" produces an identity-rotated frame matching
 * `seed_defaults` for `Plane(XY)`.
 *
 * Returns null when the normal vector has zero length, which the caller
 * surfaces as a validation error rather than POSTing a degenerate frame.
 */
function buildPlaneTransform(origin: Vec3, normal: Vec3): number[][] | null {
  const len = Math.hypot(normal[0], normal[1], normal[2])
  if (!Number.isFinite(len) || len < 1e-9) return null
  const z: Vec3 = [normal[0] / len, normal[1] / len, normal[2] / len]

  // Pick a reference vector that's not (near-)parallel to z.
  const dotY = Math.abs(z[1])
  const ref: Vec3 = dotY < 0.9 ? [0, 1, 0] : [1, 0, 0]

  // y = normalize(ref - (ref·z) z)
  const refDotZ = ref[0] * z[0] + ref[1] * z[1] + ref[2] * z[2]
  const yRaw: Vec3 = [
    ref[0] - refDotZ * z[0],
    ref[1] - refDotZ * z[1],
    ref[2] - refDotZ * z[2],
  ]
  const yLen = Math.hypot(yRaw[0], yRaw[1], yRaw[2])
  if (yLen < 1e-9) return null
  const y: Vec3 = [yRaw[0] / yLen, yRaw[1] / yLen, yRaw[2] / yLen]

  // x = y × z
  const x: Vec3 = [
    y[1] * z[2] - y[2] * z[1],
    y[2] * z[0] - y[0] * z[2],
    y[0] * z[1] - y[1] * z[0],
  ]

  // Pack as row-major 4×4 with x/y/z as columns 0/1/2 and origin in col 3.
  return [
    [x[0], y[0], z[0], origin[0]],
    [x[1], y[1], z[1], origin[1]],
    [x[2], y[2], z[2], origin[2]],
    [0, 0, 0, 1],
  ]
}

function parseFloatOrZero(value: string): number {
  const n = parseFloat(value)
  return Number.isFinite(n) ? n : 0
}

export function CreateDatumDialog({
  open,
  onOpenChange,
  onCreated,
}: CreateDatumDialogProps) {
  const [kind, setKind] = useState<DatumKindUi>('plane')
  const [name, setName] = useState('')
  const [originX, setOriginX] = useState('0')
  const [originY, setOriginY] = useState('0')
  const [originZ, setOriginZ] = useState('0')
  const [normalX, setNormalX] = useState('0')
  const [normalY, setNormalY] = useState('0')
  const [normalZ, setNormalZ] = useState('1')
  const [axisDir, setAxisDir] = useState<'x' | 'y' | 'z'>('x')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const reset = useCallback(() => {
    setKind('plane')
    setName('')
    setOriginX('0')
    setOriginY('0')
    setOriginZ('0')
    setNormalX('0')
    setNormalY('0')
    setNormalZ('1')
    setAxisDir('x')
    setError(null)
    setSubmitting(false)
  }, [])

  const handleOpenChange = useCallback(
    (next: boolean) => {
      if (!next) reset()
      onOpenChange(next)
    },
    [onOpenChange, reset],
  )

  const handleSubmit = useCallback(async () => {
    setError(null)
    const trimmedName = name.trim()
    if (!trimmedName) {
      setError('Name is required.')
      return
    }
    const origin: Vec3 = [
      parseFloatOrZero(originX),
      parseFloatOrZero(originY),
      parseFloatOrZero(originZ),
    ]

    let body: unknown
    if (kind === 'plane') {
      const normal: Vec3 = [
        parseFloatOrZero(normalX),
        parseFloatOrZero(normalY),
        parseFloatOrZero(normalZ),
      ]
      const transform = buildPlaneTransform(origin, normal)
      if (!transform) {
        setError('Normal vector must have non-zero length.')
        return
      }
      body = { kind: 'plane', name: trimmedName, transform }
    } else if (kind === 'axis') {
      body = {
        kind: 'axis',
        name: trimmedName,
        origin,
        direction: axisDir,
      }
    } else {
      body = { kind: 'point', name: trimmedName, position: origin }
    }

    setSubmitting(true)
    try {
      const resp = await fetch(`${API_BASE}/api/datums`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      if (!resp.ok) {
        const text = await resp.text().catch(() => '')
        // 400 = validation, 409 = name conflict / default mutate (won't
        // happen for create today but kept for symmetry with PATCH).
        const detail = text || `HTTP ${resp.status}`
        setError(`Create failed: ${detail}`)
        setSubmitting(false)
        return
      }
      onCreated()
      reset()
      onOpenChange(false)
    } catch (err) {
      setError(`Network error: ${err instanceof Error ? err.message : String(err)}`)
      setSubmitting(false)
    }
  }, [
    name,
    kind,
    originX,
    originY,
    originZ,
    normalX,
    normalY,
    normalZ,
    axisDir,
    onCreated,
    onOpenChange,
    reset,
  ])

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New datum</DialogTitle>
          <DialogDescription>
            Create a user-authored reference plane, axis, or point.
          </DialogDescription>
        </DialogHeader>

        {/* Kind picker — segmented control */}
        <div className="flex gap-1 rounded-lg bg-muted p-1 text-[12px] font-mono">
          {(['plane', 'axis', 'point'] as DatumKindUi[]).map((k) => (
            <button
              key={k}
              type="button"
              onClick={() => setKind(k)}
              className={cn(
                'flex-1 rounded-md px-2 py-1 transition-colors',
                kind === k
                  ? 'bg-background text-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground',
              )}
            >
              {k}
            </button>
          ))}
        </div>

        <div className="flex flex-col gap-3">
          <FormField label="Name">
            <Input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={
                kind === 'plane'
                  ? 'OffsetTop'
                  : kind === 'axis'
                    ? 'GuideAxis'
                    : 'AnchorPoint'
              }
            />
          </FormField>

          <FormField label={kind === 'point' ? 'Position' : 'Origin'}>
            <div className="grid grid-cols-3 gap-2">
              <NumericInput value={originX} onChange={setOriginX} placeholder="x" />
              <NumericInput value={originY} onChange={setOriginY} placeholder="y" />
              <NumericInput value={originZ} onChange={setOriginZ} placeholder="z" />
            </div>
          </FormField>

          {kind === 'plane' && (
            <FormField label="Normal">
              <div className="grid grid-cols-3 gap-2">
                <NumericInput value={normalX} onChange={setNormalX} placeholder="nx" />
                <NumericInput value={normalY} onChange={setNormalY} placeholder="ny" />
                <NumericInput value={normalZ} onChange={setNormalZ} placeholder="nz" />
              </div>
            </FormField>
          )}

          {kind === 'axis' && (
            <FormField label="Direction">
              <div className="flex gap-1 rounded-lg bg-muted p-1 text-[12px] font-mono">
                {(['x', 'y', 'z'] as const).map((d) => (
                  <button
                    key={d}
                    type="button"
                    onClick={() => setAxisDir(d)}
                    className={cn(
                      'flex-1 rounded-md px-2 py-1 transition-colors uppercase',
                      axisDir === d
                        ? 'bg-background text-foreground shadow-sm'
                        : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    {d}
                  </button>
                ))}
              </div>
            </FormField>
          )}

          {error && (
            <div className="text-[12px] text-destructive font-mono">{error}</div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => handleOpenChange(false)}
            disabled={submitting}
          >
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={submitting}>
            {submitting ? 'Creating…' : 'Create'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function FormField({
  label,
  children,
}: {
  label: string
  children: React.ReactNode
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] uppercase tracking-wider text-muted-foreground/70 font-mono">
        {label}
      </span>
      {children}
    </label>
  )
}

function NumericInput({
  value,
  onChange,
  placeholder,
}: {
  value: string
  onChange: (next: string) => void
  placeholder?: string
}) {
  return (
    <Input
      type="text"
      inputMode="decimal"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      className="font-mono text-[13px]"
    />
  )
}
