import { createContext } from 'react'

/** The viewport's overlay slots (see overlay-slots.tsx). */
export type SlotName = 'top' | 'left' | 'right' | 'bottom'

export type Slots = Partial<Record<SlotName, HTMLElement | null>>

export const OverlaySlotsContext = createContext<Slots>({})
