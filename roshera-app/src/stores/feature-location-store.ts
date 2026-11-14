import { create } from 'zustand'
import type { FeatureLocation } from '@/hooks/use-feature-locations'

/** Where each feature is on the part, found once (FeatureHighlight) and read
 *  by the viewport callout and the tree's column alike. */
interface FeatureLocationState {
  located: Map<number, FeatureLocation>
  setLocated: (located: Map<number, FeatureLocation>) => void
}

export const useFeatureLocationStore = create<FeatureLocationState>((set) => ({
  located: new Map(),
  setLocated: (located) => set({ located }),
}))
