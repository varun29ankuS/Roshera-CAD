// Orbit raycast gate.
//
// R3F triangle-raycasts every mesh carrying a pointer handler on EVERY
// pointermove to drive hover/enter/leave. With a full assembly visible (dozens
// of meshes, no BVH) that cost lands on the main thread during an orbit drag —
// which fires pointermove continuously — and the camera stutters ("the mouse
// feels held"). Hover is meaningless mid-orbit anyway, so we gate it off while
// the user is actively DRAGGING.
//
// The gate keys off actual pointer MOVEMENT during a press, NOT the press
// itself. A bare click must keep raycasting: R3F runs a fresh raycast on
// pointerdown (to record `initialHits`) and on the click event, and only fires
// onClick when the clicked object is in `initialHits`. Suppressing the raycast
// during a plain click would leave `initialHits` empty → the click is treated
// as a miss (deselect), and part selection / context-menu / sub-element picking
// break. At pointerdown there is definitionally no movement yet, so `moved` is
// false through down→click and the click always raycasts; only once the pointer
// has moved while pressed (a drag) do we suppress.
//
// Listeners are CAPTURE-phase on window so they update `pressed`/`moved` before
// R3F's and OrbitControls' bubble-phase handlers read the gate on the same
// event — making the flag state deterministic regardless of listener
// registration order. A non-reactive module flag (not zustand) keeps the
// CADMesh `raycast` override and these listeners off the React render path:
// routing it through state would re-render every mesh twice per orbit, a hitch
// at exactly the moment we want smooth.

let pressed = false
let moved = false

if (typeof window !== 'undefined') {
  window.addEventListener(
    'pointerdown',
    () => {
      pressed = true
      moved = false
    },
    true,
  )
  window.addEventListener(
    'pointermove',
    () => {
      if (pressed) moved = true
    },
    true,
  )
  const release = () => {
    pressed = false
    moved = false
  }
  // pointerup ends a normal drag; pointercancel covers the cases that would
  // otherwise strand the gate ON (pointer leaves the window, gesture preempted).
  window.addEventListener('pointerup', release, true)
  window.addEventListener('pointercancel', release, true)
}

/** Read by each CADMesh's raycast override; true → skip the triangle raycast. */
export function isRaycastSuppressed(): boolean {
  // Only during an actual drag (pressed AND moved) — never on a plain click.
  return pressed && moved
}
