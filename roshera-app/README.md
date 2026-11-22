# roshera-app

The browser client for Roshera: React 19 + TypeScript + Vite, with a
Three.js viewport (`@react-three/fiber`). It is a thin display layer. The
kernel in `roshera-backend/` computes and certifies all geometry; the app
renders the meshes the server sends and calls the same REST + WebSocket
surface an agent uses.

Last verified: 2026-11-22 (against `package.json` and `vite.config.ts`).

## Run

```bash
npm install
npm run dev        # Vite dev server on http://localhost:5173
```

The dev server proxies to the api-server on `localhost:8081`:
`/api` (REST), `/acp` (Agent Client Protocol, SSE streamed unbuffered) and
`/ws` (WebSocket). Start the backend first (see the root `README.md`). The
server's default CORS origins are `localhost:5173` and `127.0.0.1:5173`
(`api-server/src/main.rs`), so use that port.

| Script | What it does |
|---|---|
| `npm run dev` | Vite dev server |
| `npm run build` | `tsc -b && vite build` (type-check, then bundle to `dist/`) |
| `npm run lint` | ESLint |
| `npm run preview` | Serve the built `dist/` |

Type-check on its own with `npx tsc -p tsconfig.app.json --noEmit`. A bare
`npx tsc --noEmit` checks no files, because the root `tsconfig.json` only
holds project references.

Unit tests are `node:test` files in `test/` that import the TypeScript
sources directly; run them with `node --test` on a Node version that strips
TypeScript types. There is no `npm test` script.

## Layout

```
src/
  App.tsx            top-level layout, workspaces, sign-in
  components/
    viewport/        Three.js scene, camera, lighting (Z-up world)
    panels/          Blackboard (agent conversation), model tree, timeline,
                     drawings workspace, DFM panel, sketch, assembly
    layout/, ui/     chrome and shared controls
  lib/               API clients, ACP bridge, WebSocket bridge,
                     viewport debug bridge (viewport-bridge.tsx)
  stores/            zustand stores
docs/screenshots/    images used by the root README
```

`Dockerfile` and `nginx.conf` build the production image: nginx serves the
built SPA and proxies `/api` and `/ws` to the backend (see
`docs/DEPLOYMENT.md`).
