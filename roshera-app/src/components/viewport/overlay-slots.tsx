import { useContext, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { OverlaySlotsContext, type SlotName } from './overlay-slots-context'

/**
 * Where a viewport overlay goes. Every overlay used to pin itself to a
 * corner of the viewport, and once the tree, view bar, view cube, history
 * and Blackboard took those corners, overlays hid behind them (Varun
 * 2026-11-14: "select chamfer ... the saying is behind the menu on the
 * top"). Now the app lays out three slots in the free part of the viewport,
 * each a column that STACKS what is put in it, so two overlays can never
 * land on one spot:
 *  - top:    under the view bar -- banners and prompts;
 *  - left:   under the tree -- a tool's settings (fillet, chamfer, shell),
 *            as CAD puts them, clear of the model being picked on;
 *  - right:  under the view cube -- properties, section, live readouts;
 *  - bottom: above the history -- coordinate readouts and hints.
 */

/** Render `children` in the named slot (nothing until the slot exists). */
export function InSlot({ name, children }: { name: SlotName; children: ReactNode }) {
  const el = useContext(OverlaySlotsContext)[name]
  return el ? createPortal(children, el) : null
}
