import { createContext } from 'react'

/**
 * Whether newly rendered content should play the chalk-draw reveal.
 * Provided by the owning BlackboardLine (true only for agent content that
 * just arrived — never for persisted history) and by the fixtures harness.
 * Kept out of ChalkReveal.tsx so component files export only components
 * (react-refresh constraint).
 */
export const RevealContext = createContext<{ animate: boolean }>({ animate: false })
