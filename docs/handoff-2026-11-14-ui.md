# Handoff 2026-11-14 — UI to the mockups (overnight)

Branch `wip/step-import`. **Nothing pushed.**

## First: the server is DOWN
It was stopped for a rebuild that auto mode then refused. Rebuild + start:

```
! cd roshera-backend && cargo build --bin api-server -j 2 && ./target/debug/api-server.exe
```

The rebuild also makes live: `GET /api/timeline/parameters/{branch}` (feature
sizes, tree size column, feature edit panel) and the bed-face overhang fix.

## Done tonight (frontend; each commit: tsc 0, eslint clean, node --test green)
| Commit | What |
|---|---|
| d6734e26 | Mockup palette (neutral greys / charcoal), neutral scene lighting |
| dab7319d | Tool rail = tools, not categories: MODEL / CHECK / MAKE, AI pinned at foot; "not wired yet" entries removed |
| 585661e2 | View cube (Z-up faces) + view bar (Shaded+edges · Shaded · Section · Iso Front Top Right · Fit); lights moved above the part (they were Y-up: key behind, fill below → part read dark) |
| 38ce3002 | Status bar: Pick Part/Face/Edge/Vertex, Snap, cursor X Y Z, under-cursor part + kernel face, ✓ sound / "history unread" |
| c6a49451 | Feature tree size column: Ø80 × 12, Ø30, 4× Ø8, R3, 0.5 × 45° |
| 8bf624d8 | Blackboard: hard-coded white tints → theme colours (invisible in light theme before); composer card; starter prompts (fill only, never send) |
| 898f58f8 | Fixes to own work: cursor readout from nearest part only; view cube edges/corners |

Checked by eye: light + dark theme, 1920 and 1366×768, lighting on a temporary
in-browser part (not saved anywhere).

## NOT seen working live (server down)
- Tree size column and feature edit panel (need the parameters route). The
  backend's chapter kinds and keys were checked by reading the source:
  cylinder/boss/cut/hole/pattern, fillet `radius`, chamfer `distance`.
- Cursor readout on a real kernel part (face ids come from the server's mesh).
- Lighting strengths: judged on a stand-in part only; tune on the flange.

## Not done
- **Drawing and Assembly workspaces to the mockups**: their content needs the
  server; not restyled blind.
- Properties panel restyle.
- **Composer selection chips**: the selection does NOT travel with a message
  today; a chip would claim something false. Wire the selection first.
- **Hole tool** (kernel route + RED router test + UI): needs `cargo test`,
  which the backend CLAUDE.md reserves for your ask. One small filtered
  probe (`cargo test -p timeline-engine --lib editable`, 0 tests ran) was
  run before that rule was read.
- Commit 898f58f8's message says 125 tests; the real count is 123.
