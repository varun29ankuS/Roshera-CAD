/**
 * The Map's two kernel routes: the feature graph (cached server-side under the
 * history's hash, so opening the Map is instant on an unchanged history) and
 * the rebuild check (the recipe replayed into a fresh model and measured
 * against the live part -- the one slow action, run only when asked).
 */
import type { FeatureGraph, RebuildCheck } from '@/lib/feature-graph'

const API_HOST: string =
  (import.meta as { env?: { VITE_API_URL?: string } }).env?.VITE_API_URL ?? ''

export async function fetchFeatureGraph(branch = 'main'): Promise<FeatureGraph> {
  const r = await fetch(`${API_HOST}/api/timeline/feature-graph/${encodeURIComponent(branch)}`)
  if (!r.ok) throw new Error(`The map could not be read (HTTP ${r.status}).`)
  return (await r.json()) as FeatureGraph
}

export async function rebuildCheck(branch = 'main'): Promise<RebuildCheck> {
  const r = await fetch(`${API_HOST}/api/timeline/rebuild-check/${encodeURIComponent(branch)}`, {
    method: 'POST',
  })
  if (!r.ok) throw new Error(`The rebuild check could not run (HTTP ${r.status}).`)
  return (await r.json()) as RebuildCheck
}
