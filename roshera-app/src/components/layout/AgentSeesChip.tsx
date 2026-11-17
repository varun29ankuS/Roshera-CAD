import { useEffect, useState } from 'react'
import { Eye, EyeOff } from 'lucide-react'
import { cn } from '@/lib/utils'
import { useSceneStore } from '@/stores/scene-store'
import { useUserContextStore } from '@/stores/user-context-store'
import { LOOK_SHOWN_MS, agentSeesLabel, isLooking, lookLabel } from '@/lib/user-context'

/**
 * "Agent sees: Flange · face 56" -- in the status bar, what an agent reading
 * the person's context would be told right now (the last report the server
 * acknowledged), and whether one has just read it: the eye lights while an
 * agent is looking. Clicking it pauses or resumes sharing; paused, the agent
 * is told so and nothing else.
 */
export function AgentSeesChip() {
  const sharing = useUserContextStore((s) => s.sharing)
  const setSharing = useUserContextStore((s) => s.setSharing)
  const acknowledged = useUserContextStore((s) => s.acknowledged)
  const look = useUserContextStore((s) => s.look)
  const objects = useSceneStore((s) => s.objects)

  // The clock the "looking" light reads. It only moves when a read stops
  // being recent: until then it is at or before the read, which is "now".
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!look) return
    const left = Math.max(0, look.atMs + LOOK_SHOWN_MS - Date.now())
    const t = window.setTimeout(() => setNow(Date.now()), left + 20)
    return () => window.clearTimeout(t)
  }, [look])

  const looking = sharing && isLooking(look, now)
  // Paused is shown at once, before the server has acknowledged it.
  const label = sharing ? agentSeesLabel(acknowledged, (id) => objects.get(id)?.name ?? null) : 'nothing · paused'
  const looked = lookLabel(look, now)
  const title = sharing
    ? `What an agent is told you are pointing at and have selected${looked ? ` · ${looked}` : ' · no agent has looked yet'}. Click to pause.`
    : 'Paused: an agent is told only that you paused. Click to share again.'
  const Icon = sharing ? Eye : EyeOff

  return (
    <button
      type="button"
      aria-pressed={sharing}
      aria-label={sharing ? 'Pause sharing what you point at with the agent' : 'Share what you point at with the agent'}
      title={title}
      onClick={() => setSharing(!sharing)}
      className="cad-focus flex h-[18px] min-w-0 items-center gap-1 rounded px-1.5 leading-none hover:bg-accent hover:text-foreground"
    >
      <Icon
        size={12}
        className={cn('shrink-0', looking && 'animate-pulse text-[var(--cad-selected)]')}
        aria-hidden
      />
      <span className="shrink-0">Agent sees:</span>
      <span className={cn('min-w-0 truncate', sharing ? 'text-foreground' : 'text-muted-foreground')}>{label}</span>
      {looking && look && <span className="shrink-0 text-[var(--cad-selected)]">· {look.agent} is looking</span>}
    </button>
  )
}
