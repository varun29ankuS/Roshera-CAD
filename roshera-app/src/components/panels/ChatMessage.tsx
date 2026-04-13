import { cn } from '@/lib/utils'
import type { ChatMessage as ChatMessageType } from '@/stores/chat-store'
import { Bot, User, AlertCircle, Info } from 'lucide-react'

interface Props {
  message: ChatMessageType
}

export function ChatMessage({ message }: Props) {
  const isUser = message.role === 'user'
  const isSystem = message.role === 'system'
  const isError = message.isError

  if (isSystem) {
    return (
      <div className="flex items-start gap-2 px-3 py-2">
        <Info size={14} className="text-muted-foreground mt-0.5 shrink-0" />
        <p className="text-xs text-muted-foreground italic">{message.content}</p>
      </div>
    )
  }

  return (
    <div
      className={cn(
        'flex items-start gap-2 px-3 py-2',
        isUser ? 'flex-row-reverse' : 'flex-row',
      )}
    >
      <div
        className={cn(
          'w-6 h-6 rounded-full flex items-center justify-center shrink-0 mt-0.5',
          isUser ? 'bg-primary/20' : 'bg-accent',
        )}
      >
        {isUser ? (
          <User size={12} className="text-primary" />
        ) : (
          <Bot size={12} className="text-foreground" />
        )}
      </div>

      <div
        className={cn(
          'max-w-[85%] rounded-lg px-3 py-2 text-sm',
          isUser
            ? 'bg-primary text-primary-foreground'
            : isError
              ? 'bg-destructive/10 text-destructive border border-destructive/20'
              : 'bg-accent text-accent-foreground',
        )}
      >
        {isError && (
          <AlertCircle size={12} className="inline mr-1 -mt-0.5" />
        )}
        <span className="whitespace-pre-wrap">{message.content}</span>
        {message.objectsAffected && message.objectsAffected.length > 0 && (
          <div className="mt-1 text-[10px] text-muted-foreground">
            Objects: {message.objectsAffected.join(', ').slice(0, 60)}
          </div>
        )}
      </div>
    </div>
  )
}
