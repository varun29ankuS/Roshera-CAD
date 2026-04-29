import { Loader2 } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import type { DemoEntry } from '@/lib/demo-types'

interface DemoCardProps {
  demo: DemoEntry
  isActive: boolean
  isLoading: boolean
  onLoad: () => void
}

const numberFmt = new Intl.NumberFormat('en-US')

export function DemoCard({ demo, isActive, isLoading, onLoad }: DemoCardProps) {
  return (
    <button
      onClick={onLoad}
      disabled={isLoading}
      className={[
        'flex flex-col items-start gap-2 p-4 rounded-lg border text-left transition-all',
        'bg-card/60 hover:bg-card hover:border-primary/50',
        isActive ? 'border-primary ring-1 ring-primary/40' : 'border-border',
        isLoading ? 'opacity-60 cursor-wait' : 'cursor-pointer',
      ].join(' ')}
    >
      <div className="flex items-center justify-between w-full gap-2">
        <span className="font-mono text-sm text-foreground truncate">{demo.filename}</span>
        {isLoading && <Loader2 className="w-3 h-3 animate-spin text-muted-foreground" />}
      </div>

      <div className="flex flex-wrap gap-1.5">
        <Badge variant="secondary" className="text-[10px] font-mono">
          {numberFmt.format(demo.verts)} verts
        </Badge>
        <Badge variant="secondary" className="text-[10px] font-mono">
          {numberFmt.format(demo.tris)} tris
        </Badge>
        <Badge variant="outline" className="text-[10px] font-mono">
          {demo.tess_ms.toFixed(1)} ms
        </Badge>
      </div>
    </button>
  )
}
