/**
 * Assembly workspace — minimal vertical slice for the mate editor.
 *
 * Layout:
 *   ┌──────────────┬───────────────────────────────────────────────┐
 *   │ Assemblies   │  Components                                   │
 *   │ (list +      │  ──────────────────────────────────────────── │
 *   │  + New)      │  Mates  [Add Mate] [Solve]                    │
 *   │              │  ──────────────────────────────────────────── │
 *   │              │  <mate rows: name | type | flip | suppress | × │
 *   └──────────────┴───────────────────────────────────────────────┘
 *
 * This is intentionally compact. The richer outliner panel (ASM1)
 * will replace the centre body later; the mate editor (ASM2 — this
 * file's reason for existing) is the demo-critical feature.
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import * as THREE from 'three'
import {
  deleteAssembly,
  getComponentMesh,
  patchMate,
  removeComponent,
  removeMate,
  setComponentTransform,
  solveAssembly,
  translationMatrix,
  translationOf,
  mateTypeTag,
  mateTypeLabel,
  type ComponentSummary,
  type MateReferenceSummary,
  type MateSummary,
} from '@/lib/assembly-api'
import { useAssemblyStore } from '@/stores/assembly-store'
import { useSceneStore, type CADMaterial, type CADMesh } from '@/stores/scene-store'
import { AddMateDialog } from './AddMateDialog'
import { AddComponentDialog } from './AddComponentDialog'
import { AddReferenceDialog } from './AddReferenceDialog'
import { CADViewport } from '@/components/viewport/CADViewport'

/**
 * Prefix for scene-store ids of assembly-mode component objects. Keeps
 * them disjoint from part-mode object ids so the two can coexist in
 * the global scene-store without collisions, and so the workspace's
 * unmount cleanup can target *only* the objects it injected.
 */
const ASM_OBJ_PREFIX = 'asm-comp:'

/** Neutral metallic-ish look for components until per-instance materials land. */
const ASM_DEFAULT_MATERIAL: CADMaterial = {
  color: '#9aa5b1',
  metalness: 0.2,
  roughness: 0.6,
  opacity: 1,
}

/**
 * Decompose a kernel row-major 4×4 transform into the
 * position / Euler-XYZ rotation / scale triple consumed by
 * `CADObject`. Three.js's `Matrix4.set` already takes row-major
 * arguments, so we feed the wire form directly.
 */
function decomposeRowMajor(t: number[][]): {
  position: [number, number, number]
  rotation: [number, number, number]
  scale: [number, number, number]
} {
  const m = new THREE.Matrix4().set(
    t[0][0], t[0][1], t[0][2], t[0][3],
    t[1][0], t[1][1], t[1][2], t[1][3],
    t[2][0], t[2][1], t[2][2], t[2][3],
    t[3][0], t[3][1], t[3][2], t[3][3],
  )
  const position = new THREE.Vector3()
  const quaternion = new THREE.Quaternion()
  const scale = new THREE.Vector3()
  m.decompose(position, quaternion, scale)
  const euler = new THREE.Euler().setFromQuaternion(quaternion, 'XYZ')
  return {
    position: [position.x, position.y, position.z],
    rotation: [euler.x, euler.y, euler.z],
    scale: [scale.x, scale.y, scale.z],
  }
}

export function AssemblyWorkspace() {
  const ids = useAssemblyStore((s) => s.ids)
  const activeId = useAssemblyStore((s) => s.activeId)
  const active = useAssemblyStore((s) => s.active)
  const loading = useAssemblyStore((s) => s.loading)
  const error = useAssemblyStore((s) => s.error)
  const refreshList = useAssemblyStore((s) => s.refreshList)
  const refreshActive = useAssemblyStore((s) => s.refreshActive)
  const setActive = useAssemblyStore((s) => s.setActive)
  const setSnapshot = useAssemblyStore((s) => s.setSnapshot)
  const setError = useAssemblyStore((s) => s.setError)
  const createAndActivate = useAssemblyStore((s) => s.createAndActivate)

  // Scene-store handles for pushing component meshes into the shared
  // viewport. We keep this side-effect contained: only objects whose
  // ids carry `ASM_OBJ_PREFIX` are touched, so part-mode state is
  // never disturbed.
  const addObject = useSceneStore((s) => s.addObject)
  const updateObject = useSceneStore((s) => s.updateObject)
  const removeObject = useSceneStore((s) => s.removeObject)
  const selectedIds = useSceneStore((s) => s.selectedIds)
  /** Scene-store ids we've injected; we own their lifecycle. */
  const injectedIdsRef = useRef<Set<string>>(new Set())

  // When the viewport has exactly one assembly-component selected,
  // resolve it back to its component-id so the sidebar can highlight
  // the matching row. Returns `null` for multi-select, part objects,
  // or empty selection.
  const selectedComponentId = useMemo(() => {
    if (selectedIds.size !== 1) return null
    const id = Array.from(selectedIds)[0]
    return id.startsWith(ASM_OBJ_PREFIX) ? id.slice(ASM_OBJ_PREFIX.length) : null
  }, [selectedIds])

  const [newName, setNewName] = useState('Assembly 1')
  const [addMateOpen, setAddMateOpen] = useState(false)
  const [addComponentOpen, setAddComponentOpen] = useState(false)
  /** When set, the AddReferenceDialog is open for this component. */
  const [refTarget, setRefTarget] = useState<ComponentSummary | null>(null)
  /** Component ids whose slot list is currently expanded. */
  const [expandedComponents, setExpandedComponents] = useState<Set<string>>(new Set())

  const toggleComponentExpanded = (id: string) => {
    setExpandedComponents((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  // First-mount fetch. We don't auto-select an assembly — the user
  // picks one (or creates one) so they aren't surprised by an
  // unfamiliar id snapping into the centre pane.
  useEffect(() => {
    void refreshList()
  }, [refreshList])

  // Cleanup on unmount — strip every component object this workspace
  // ever injected. Other workspaces' scene-store objects (part bodies,
  // sketches, etc.) are untouched.
  useEffect(() => {
    return () => {
      for (const id of injectedIdsRef.current) removeObject(id)
      injectedIdsRef.current = new Set()
    }
  }, [removeObject])

  // Sync the scene-store with the active assembly's components.
  //   • Added components → fetch mesh, push as CADObject.
  //   • Existing components with changed transform → updateObject
  //     (transform-only — skip mesh refetch).
  //   • Components no longer in the snapshot → removeObject.
  //   • No active assembly → clear all injected.
  // We key on `active` so this fires on every snapshot change (solve,
  // setComponentTransform, addComponent, removeComponent).
  useEffect(() => {
    if (!active) {
      for (const id of injectedIdsRef.current) removeObject(id)
      injectedIdsRef.current = new Set()
      return
    }
    const assemblyId = active.id
    const liveIds = new Set(active.components.map((c) => `${ASM_OBJ_PREFIX}${c.id}`))
    // Drop stale (component removed from assembly).
    for (const id of Array.from(injectedIdsRef.current)) {
      if (!liveIds.has(id)) {
        removeObject(id)
        injectedIdsRef.current.delete(id)
      }
    }

    let cancelled = false
    void (async () => {
      for (const c of active.components) {
        if (cancelled) return
        const sceneId = `${ASM_OBJ_PREFIX}${c.id}`
        const decomposed = decomposeRowMajor(c.transform)
        if (injectedIdsRef.current.has(sceneId)) {
          // Transform / name / fixed-flag only — mesh is invariant
          // until the component's part itself changes (out of scope
          // for ASM3).
          updateObject(sceneId, {
            name: c.name,
            position: decomposed.position,
            rotation: decomposed.rotation,
            scale: decomposed.scale,
            locked: c.is_fixed,
          })
          continue
        }
        try {
          const mesh = await getComponentMesh(assemblyId, c.id)
          if (cancelled) return
          const cadMesh: CADMesh = {
            vertices: Float32Array.from(mesh.vertices),
            normals: Float32Array.from(mesh.normals),
            indices: Uint32Array.from(mesh.indices),
          }
          addObject({
            id: sceneId,
            name: c.name,
            objectType: 'assembly-component',
            mesh: cadMesh,
            material: ASM_DEFAULT_MATERIAL,
            position: decomposed.position,
            rotation: decomposed.rotation,
            scale: decomposed.scale,
            visible: true,
            locked: c.is_fixed,
          })
          injectedIdsRef.current.add(sceneId)
        } catch (e) {
          if (!cancelled) {
            setError(e instanceof Error ? e.message : String(e))
          }
        }
      }
    })()

    return () => {
      cancelled = true
    }
  }, [active, addObject, updateObject, removeObject, setError])

  const handleCreate = async () => {
    const trimmed = newName.trim()
    if (!trimmed) return
    setError(null)
    try {
      await createAndActivate(trimmed)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handleDelete = async (id: string) => {
    if (!confirm('Delete this assembly?')) return
    setError(null)
    try {
      await deleteAssembly(id)
      if (activeId === id) {
        await setActive(null)
      }
      await refreshList()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handleSolve = async () => {
    if (!activeId) return
    setError(null)
    try {
      const snap = await solveAssembly(activeId)
      setSnapshot(snap)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handlePatchMate = async (mateId: string, patch: { suppressed?: boolean; flip?: boolean }) => {
    if (!activeId) return
    setError(null)
    try {
      const snap = await patchMate(activeId, mateId, patch)
      setSnapshot(snap)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handleRemoveMate = async (mateId: string) => {
    if (!activeId) return
    setError(null)
    try {
      await removeMate(activeId, mateId)
      await refreshActive()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handleRemoveComponent = async (componentId: string) => {
    if (!activeId) return
    if (!confirm('Delete this component? Any mates targeting it will fail to solve.')) return
    setError(null)
    try {
      await removeComponent(activeId, componentId)
      await refreshActive()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  const handleSetTranslation = async (componentId: string, t: [number, number, number]) => {
    if (!activeId) return
    setError(null)
    try {
      const snap = await setComponentTransform(
        activeId,
        componentId,
        translationMatrix(t[0], t[1], t[2]),
      )
      setSnapshot(snap)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="flex flex-col flex-1 min-h-0 bg-background text-foreground">
      {error && (
        <div className="px-3 py-2 text-xs bg-destructive/10 text-destructive border-b border-destructive/30">
          {error}
        </div>
      )}

      <div className="flex flex-1 min-h-0">
        {/* ── Assembly list sidebar ───────────────────────────────── */}
        <aside className="w-72 flex flex-col border-r border-border/60 bg-background/40 overflow-hidden">
          <div className="px-3 py-2 border-b border-border/60">
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
              Assemblies
            </div>
          </div>

          <div className="px-3 py-2 space-y-2 border-b border-border/40">
            <input
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="Name"
              className="cad-focus w-full px-2 py-1 text-xs rounded border border-border bg-background"
            />
            <button
              type="button"
              onClick={() => void handleCreate()}
              className="cad-focus w-full py-1 text-xs font-medium rounded bg-primary text-primary-foreground hover:opacity-90"
            >
              + New Assembly
            </button>
          </div>

          <div className={`${active ? 'max-h-40' : 'flex-1'} min-h-0 overflow-y-auto py-1`}>
            {ids.length === 0 ? (
              <div className="px-3 py-4 text-xs text-muted-foreground text-center">
                No assemblies yet.
              </div>
            ) : (
              ids.map((id) => {
                const isActive = id === activeId
                const label = active && active.id === id ? active.name : id.slice(0, 8)
                return (
                  <div
                    key={id}
                    className={[
                      'group flex items-stretch gap-0.5 text-xs',
                      isActive
                        ? 'bg-accent/40 text-foreground'
                        : 'text-muted-foreground hover:bg-accent/20',
                    ].join(' ')}
                  >
                    <button
                      type="button"
                      onClick={() => void setActive(id)}
                      className="cad-focus flex-1 min-w-0 px-3 py-1.5 text-left truncate"
                    >
                      {label}
                    </button>
                    <div className="flex-shrink-0 flex items-center pr-2 opacity-60 group-hover:opacity-100 transition-opacity">
                      <button
                        type="button"
                        title="Delete assembly"
                        aria-label="Delete assembly"
                        onClick={() => void handleDelete(id)}
                        className="cad-focus inline-flex items-center justify-center w-6 h-6 rounded text-destructive hover:bg-destructive/15"
                      >
                        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
                          <path d="M4 4l8 8M12 4l-8 8" />
                        </svg>
                      </button>
                    </div>
                  </div>
                )
              })
            )}
          </div>

          {/* When an assembly is active, the components + mates editors
              stack below the assembly list inside this same sidebar.
              The 3D viewport occupies the main pane (axes + grid). */}
          {active && (
            <div className="flex-1 min-h-0 flex flex-col border-t border-border/60">
              <header className="px-3 py-2 border-b border-border/40 flex items-center justify-between gap-2">
                <div className="flex flex-col min-w-0">
                  <h2 className="text-sm font-medium truncate">{active.name}</h2>
                  <span className="text-[10px] uppercase tracking-wider text-muted-foreground">
                    {active.components.length} comp · {active.mates.length} mates
                  </span>
                </div>
                <div className="flex items-center gap-1">
                  <MateModeToggle />
                  <button
                    type="button"
                    onClick={() => void handleSolve()}
                    className="cad-focus px-2 py-1 text-[11px] font-medium rounded border border-border hover:bg-accent/40"
                    title="Run the mate solver against the current constraints"
                  >
                    Solve
                  </button>
                </div>
              </header>

              <div className="flex-1 min-h-0 overflow-y-auto">
                <section className="px-3 py-2 border-b border-border/40">
                  <div className="flex items-center justify-between mb-2">
                    <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
                      Components
                    </div>
                    <button
                      type="button"
                      onClick={() => setAddComponentOpen(true)}
                      className="cad-focus px-2 py-0.5 text-[11px] rounded border border-border hover:bg-accent/40"
                    >
                      + Add
                    </button>
                  </div>
                  {active.components.length === 0 ? (
                    <div className="text-xs text-muted-foreground">
                      No components yet — click <span className="font-medium">+ Add</span> to create one.
                    </div>
                  ) : (
                    <ul className="space-y-1">
                      {active.components.map((c) => (
                        <ComponentRow
                          key={c.id}
                          component={c}
                          expanded={expandedComponents.has(c.id)}
                          selectedInViewport={selectedComponentId === c.id}
                          onToggleExpand={() => toggleComponentExpanded(c.id)}
                          onSetTranslation={(t) => void handleSetTranslation(c.id, t)}
                          onAddReference={() => setRefTarget(c)}
                          onRemove={() => void handleRemoveComponent(c.id)}
                        />
                      ))}
                    </ul>
                  )}
                </section>

                <section className="flex flex-col">
                  <div className="px-3 py-2 border-b border-border/40 flex items-center justify-between">
                    <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
                      Mates
                    </div>
                    <button
                      type="button"
                      onClick={() => setAddMateOpen(true)}
                      disabled={active.components.length < 2}
                      title={
                        active.components.length < 2
                          ? 'Need at least two components to create a mate'
                          : 'Open the Add Mate dialog'
                      }
                      className="cad-focus px-2 py-1 text-[11px] font-medium rounded bg-primary text-primary-foreground hover:opacity-90 disabled:opacity-50"
                    >
                      + Add Mate
                    </button>
                  </div>
                  {active.mates.length === 0 ? (
                    <div className="px-3 py-4 text-xs text-muted-foreground text-center">
                      No mates yet.
                    </div>
                  ) : (
                    <ul className="divide-y divide-border/40">
                      {active.mates.map((m) => (
                        <MateRow
                          key={m.id}
                          mate={m}
                          componentName={(id) =>
                            active.components.find((c) => c.id === id)?.name ??
                            id.slice(0, 8)
                          }
                          onTogglePatch={(patch) => void handlePatchMate(m.id, patch)}
                          onRemove={() => void handleRemoveMate(m.id)}
                        />
                      ))}
                    </ul>
                  )}
                </section>
              </div>
            </div>
          )}
        </aside>

        {/* ── Centre: 3D viewport ─────────────────────────────────────
            Mounted unconditionally so the grid + gizmo are always
            present (matches Part workspace). When no assembly is
            active, a centred prompt sits above the empty viewport. */}
        <main className="relative flex-1 overflow-hidden">
          <CADViewport />
          {!active && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <div className="pointer-events-auto px-3 py-1.5 text-xs text-muted-foreground bg-background/80 border border-border/60 rounded shadow-sm">
                {loading
                  ? 'Loading…'
                  : ids.length === 0
                    ? 'Create an assembly to begin.'
                    : 'Select an assembly from the sidebar.'}
              </div>
            </div>
          )}
        </main>
      </div>

      {addMateOpen && active && (
        <AddMateDialog
          assemblyId={active.id}
          components={active.components}
          onClose={() => setAddMateOpen(false)}
          onCreated={() => void refreshActive()}
        />
      )}

      {addComponentOpen && active && (
        <AddComponentDialog
          assemblyId={active.id}
          defaultName={`Component ${active.components.length + 1}`}
          onClose={() => setAddComponentOpen(false)}
          onCreated={() => void refreshActive()}
        />
      )}

      {refTarget && active && (
        <AddReferenceDialog
          assemblyId={active.id}
          component={refTarget}
          onClose={() => setRefTarget(null)}
          onCreated={() => void refreshActive()}
        />
      )}
    </div>
  )
}

/**
 * Component row with an inline x/y/z translation editor, a slot
 * expander, and Add-Reference / Delete actions. Translation commits
 * on Enter or blur; Escape reverts to the row's last-known snapshot.
 */
function ComponentRow({
  component,
  expanded,
  selectedInViewport,
  onToggleExpand,
  onSetTranslation,
  onAddReference,
  onRemove,
}: {
  component: ComponentSummary
  expanded: boolean
  /** True when the viewport currently has this component selected. */
  selectedInViewport: boolean
  onToggleExpand: () => void
  onSetTranslation: (t: [number, number, number]) => void
  onAddReference: () => void
  onRemove: () => void
}) {
  const snapshotT = useMemo(() => translationOf(component.transform), [component.transform])
  const [t, setT] = useState<[number, number, number]>(snapshotT)

  // Re-seed when the snapshot changes from elsewhere (solver, mate add).
  useEffect(() => {
    setT(snapshotT)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snapshotT[0], snapshotT[1], snapshotT[2]])

  const dirty = t[0] !== snapshotT[0] || t[1] !== snapshotT[1] || t[2] !== snapshotT[2]
  const commit = () => {
    if (dirty) onSetTranslation(t)
  }
  const revert = () => setT(snapshotT)

  return (
    <li
      className={[
        'text-xs rounded transition-colors',
        selectedInViewport
          ? 'bg-accent/40 ring-1 ring-primary/40'
          : 'hover:bg-accent/10',
      ].join(' ')}
    >
      <div className="flex items-center gap-2 px-2 py-1">
        <button
          type="button"
          onClick={onToggleExpand}
          className="cad-focus w-4 h-4 flex items-center justify-center text-muted-foreground hover:text-foreground"
          aria-label={expanded ? 'Collapse' : 'Expand'}
          aria-expanded={expanded}
        >
          <svg
            width="8"
            height="8"
            viewBox="0 0 8 8"
            className={`transition-transform ${expanded ? 'rotate-90' : ''}`}
          >
            <path d="M2 1 L6 4 L2 7 Z" fill="currentColor" />
          </svg>
        </button>
        <span className="flex-1 truncate font-medium">{component.name}</span>
        <span className="text-muted-foreground">DoF: {component.degrees_of_freedom}</span>
        {component.is_fixed && (
          <span className="text-[10px] uppercase text-amber-500/80">fixed</span>
        )}
        <button
          type="button"
          onClick={onRemove}
          title="Delete component"
          aria-label="Delete component"
          className="cad-focus inline-flex items-center justify-center w-5 h-5 rounded text-destructive hover:bg-destructive/15"
        >
          <svg width="10" height="10" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </div>
      {expanded && (
        <div className="ml-6 mb-2 mr-2 space-y-2 px-2 py-2 rounded bg-background/40 border border-border/40">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-muted-foreground mb-1">
              Translation (mm)
            </div>
            <div className="grid grid-cols-3 gap-1">
              {(['x', 'y', 'z'] as const).map((axis, i) => (
                <label key={axis} className="flex items-center gap-1">
                  <span className="w-3 text-muted-foreground">{axis.toUpperCase()}</span>
                  <input
                    type="number"
                    step="1"
                    value={t[i]}
                    onChange={(e) => {
                      const v = Number(e.target.value)
                      const next: [number, number, number] = [...t]
                      next[i] = v
                      setT(next)
                    }}
                    onBlur={commit}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault()
                        commit()
                          ; (e.currentTarget as HTMLInputElement).blur()
                      } else if (e.key === 'Escape') {
                        e.preventDefault()
                        revert()
                          ; (e.currentTarget as HTMLInputElement).blur()
                      }
                    }}
                    className="cad-focus flex-1 min-w-0 px-1.5 py-0.5 rounded border border-border bg-background text-[11px]"
                  />
                </label>
              ))}
            </div>
            {dirty && (
              <div className="mt-1 text-[10px] text-muted-foreground">
                Press Enter to apply, Esc to revert.
              </div>
            )}
          </div>

          <div>
            <div className="flex items-center justify-between mb-1">
              <div className="text-[10px] uppercase tracking-wider text-muted-foreground">
                Mate references ({component.mate_references.length})
              </div>
              <button
                type="button"
                onClick={onAddReference}
                className="cad-focus px-2 py-0.5 text-[10px] rounded border border-border hover:bg-accent/40"
              >
                + Ref
              </button>
            </div>
            {component.mate_references.length === 0 ? (
              <div className="text-[11px] text-muted-foreground">
                No references registered. Add one to enable mates.
              </div>
            ) : (
              <ul className="space-y-0.5">
                {component.mate_references.map((r) => (
                  <li
                    key={r.name}
                    className="flex items-center gap-2 px-1 py-0.5 text-[11px]"
                  >
                    <span className="font-medium">{r.name}</span>
                    <span className="text-muted-foreground">
                      {summarizeReference(r)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </li>
  )
}

function summarizeReference(r: MateReferenceSummary): string {
  if (r.Face) return `Face ${r.Face.face_id.slice(0, 8)}…`
  if (r.Edge) return `Edge ${r.Edge.edge_id.slice(0, 8)}…`
  if (r.Point) return `Point (${fmt(r.Point.position.x)}, ${fmt(r.Point.position.y)}, ${fmt(r.Point.position.z)})`
  if (r.Axis) return `Axis @(${fmt(r.Axis.origin.x)}, ${fmt(r.Axis.origin.y)}, ${fmt(r.Axis.origin.z)})`
  if (r.Plane) return `Plane @(${fmt(r.Plane.origin.x)}, ${fmt(r.Plane.origin.y)}, ${fmt(r.Plane.origin.z)})`
  return '(unknown reference)'
}

function fmt(n: number): string {
  return Number.isInteger(n) ? String(n) : n.toFixed(2)
}

function MateRow({
  mate,
  componentName,
  onTogglePatch,
  onRemove,
}: {
  mate: MateSummary
  componentName: (id: string) => string
  onTogglePatch: (patch: { suppressed?: boolean; flip?: boolean }) => void
  onRemove: () => void
}) {
  const tag = mateTypeTag(mate.mate_type)
  return (
    <li className="flex items-center gap-3 px-4 py-2 text-xs hover:bg-accent/20">
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-2">
          <span className="font-medium truncate">{mate.name || `${tag} mate`}</span>
          <span className="text-[10px] uppercase tracking-wider text-muted-foreground">
            {mateTypeLabel(tag)}
          </span>
          {mate.solved ? (
            <span className="text-[10px] text-emerald-500/80">solved</span>
          ) : mate.error ? (
            <span
              className="text-[10px] text-destructive truncate"
              title={mate.error}
            >
              error
            </span>
          ) : null}
        </div>
        <div className="text-[10px] text-muted-foreground truncate">
          {componentName(mate.component1)} · {mate.reference1}
          {' ↔ '}
          {componentName(mate.component2)} · {mate.reference2}
        </div>
      </div>

      {/* Flip toggle — kernel guards `flip` against suppressed-during-solve;
          we surface it as a stateful chip. */}
      <button
        type="button"
        onClick={() => onTogglePatch({ flip: !mate.flip })}
        title="Flip mate alignment"
        className={[
          'cad-focus px-2 py-0.5 text-[10px] uppercase tracking-wider rounded border',
          mate.flip
            ? 'border-primary text-primary'
            : 'border-border text-muted-foreground hover:text-foreground',
        ].join(' ')}
      >
        Flip
      </button>

      {/* Suppress toggle — pauses the constraint without deleting it. */}
      <button
        type="button"
        onClick={() => onTogglePatch({ suppressed: !mate.suppressed })}
        title={mate.suppressed ? 'Unsuppress mate' : 'Suppress mate'}
        className={[
          'cad-focus px-2 py-0.5 text-[10px] uppercase tracking-wider rounded border',
          mate.suppressed
            ? 'border-amber-500/60 text-amber-500'
            : 'border-border text-muted-foreground hover:text-foreground',
        ].join(' ')}
      >
        {mate.suppressed ? 'Suppressed' : 'Active'}
      </button>

      <button
        type="button"
        onClick={onRemove}
        title="Delete mate"
        aria-label="Delete mate"
        className="cad-focus inline-flex items-center justify-center w-6 h-6 rounded text-destructive hover:bg-destructive/15"
      >
        <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round">
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
      </button>
    </li>
  )
}

/**
 * Mate-mode toggle. Flips the global `selectionMode` between
 * `'object'` (component-select → transform gizmo) and `'face'`
 * (face-pick → mate-suggestion popover). Without this affordance,
 * users have no discoverable way to enter the face-pick path that
 * `MateSuggestionPopover` listens on.
 *
 * Toggling out of mate mode also drops any in-flight pick so the
 * pending-mate state doesn't survive into the next interaction.
 */
function MateModeToggle() {
  const selectionMode = useSceneStore((s) => s.selectionMode)
  const setSelectionMode = useSceneStore((s) => s.setSelectionMode)
  const pendingMate = useAssemblyStore((s) => s.pendingMate)
  const clearPendingMate = useAssemblyStore((s) => s.clearPendingMate)
  const isMateMode = selectionMode === 'face'
  return (
    <button
      type="button"
      onClick={() => {
        if (isMateMode) {
          if (pendingMate.ref1 || pendingMate.ref2) clearPendingMate()
          setSelectionMode('object')
        } else {
          setSelectionMode('face')
        }
      }}
      title={
        isMateMode
          ? 'Exit mate mode — return to component drag/select'
          : 'Enter mate mode — click two faces to create a mate'
      }
      className={[
        'cad-focus px-2 py-1 text-[11px] font-medium rounded border transition-colors',
        isMateMode
          ? 'border-primary bg-primary/15 text-foreground'
          : 'border-border hover:bg-accent/40',
      ].join(' ')}
    >
      {isMateMode ? 'Mate ✓' : 'Mate'}
    </button>
  )
}
