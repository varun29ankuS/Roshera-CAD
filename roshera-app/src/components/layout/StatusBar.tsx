import { useSceneStore } from '@/stores/scene-store'
import { useWSStore } from '@/stores/ws-store'
import { Separator } from '@/components/ui/separator'

export function StatusBar() {
  const objectCount = useSceneStore((s) => s.objects.size)
  const selectedCount = useSceneStore((s) => s.selectedIds.size)
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const activeTool = useSceneStore((s) => s.activeTool)
  const transformSpace = useSceneStore((s) => s.transformSpace)
  const snapMode = useSceneStore((s) => s.snapMode)
  const snapValue = useSceneStore((s) => s.snapValue)
  const lastPing = useWSStore((s) => s.lastPingMs)
  const gridSettings = useSceneStore((s) => s.gridSettings)

  return (
    <div className="flex items-center h-6 cad-panel border-t px-3 text-[10px] text-muted-foreground">
      <span className="uppercase tracking-wider font-medium">
        {selectionMode === 'object' ? activeTool : `${selectionMode} select`}
      </span>

      <Separator orientation="vertical" className="mx-2 h-3" />

      <span>Space: {transformSpace}</span>

      <Separator orientation="vertical" className="mx-2 h-3" />

      <span>
        Snap: {snapMode !== 'none' ? `${snapValue}mm` : 'off'}
      </span>

      <Separator orientation="vertical" className="mx-2 h-3" />

      <span>
        Grid: {gridSettings.visible ? `${gridSettings.cellSize}mm` : 'off'}
      </span>

      <div className="flex-1" />

      <span>
        {objectCount} object{objectCount !== 1 ? 's' : ''}
        {selectedCount > 0 && ` (${selectedCount} selected)`}
      </span>

      {lastPing !== null && (
        <>
          <Separator orientation="vertical" className="mx-2 h-3" />
          <span>{lastPing}ms</span>
        </>
      )}
    </div>
  )
}
