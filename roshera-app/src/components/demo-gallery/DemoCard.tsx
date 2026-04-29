import { Loader2 } from 'lucide-react'
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
        'flex flex-col items-stretch gap-1.5 px-3 py-2 border text-left transition-colors w-full',
        'bg-card hover:bg-accent/30',
        isActive
          ? 'border-primary'
          : 'border-border/60 hover:border-border',
        isLoading ? 'opacity-60 cursor-wait' : 'cursor-pointer',
      ].join(' ')}
    >
      <div className="flex items-center justify-between w-full gap-2">
        <span className="font-mono text-[11px] text-foreground truncate uppercase tracking-wider">
          {demo.filename}
        </span>
        {isLoading && (
          <Loader2 className="w-3 h-3 animate-spin text-muted-foreground shrink-0" />
        )}
      </div>

      <div className="flex flex-col gap-0.5 text-[10px] font-mono text-muted-foreground tabular-nums">
        <div className="flex items-baseline gap-2">
          <span className="uppercase tracking-wider min-w-[40px]">Verts</span>
          <span className="flex-1 border-b border-dotted border-border/60 translate-y-[-2px]" />
          <span className="text-foreground">{numberFmt.format(demo.verts)}</span>
        </div>
        <div className="flex items-baseline gap-2">
          <span className="uppercase tracking-wider min-w-[40px]">Tris</span>
          <span className="flex-1 border-b border-dotted border-border/60 translate-y-[-2px]" />
          <span className="text-foreground">{numberFmt.format(demo.tris)}</span>
        </div>
        <div className="flex items-baseline gap-2">
          <span className="uppercase tracking-wider min-w-[40px]">Tess</span>
          <span className="flex-1 border-b border-dotted border-border/60 translate-y-[-2px]" />
          <span className="text-foreground">{demo.tess_ms.toFixed(1)} ms</span>
        </div>
      </div>
    </button>
  )
}
