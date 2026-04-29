# Viewport Debug Bridge

Gives Claude (or any dev tool) eyes into the live Three.js viewport so the
kernel-debug loop can be visual instead of triangle-counting.

## Architecture

```
Claude / curl ──HTTP POST──► api-server ──WS push──► CADViewport.tsx
                                  ▲                        │
                                  └──── WS reply ──────────┘
                                  │
                                  ▼
                          target/snapshots/*.png
```

A single WebSocket lives at `/ws/viewport-bridge`. The frontend
(`ViewportBridge` component, mounted in `App.tsx`) connects on load. Each
REST endpoint under `/api/viewport/*` generates a request id, pushes a
JSON command over the socket, awaits the matching reply, and surfaces the
result as JSON (snapshot endpoints additionally write a PNG to disk).

## Enabling

Both sides must be enabled:

* **Backend**: `ROSHERA_DEV_BRIDGE=1` in `roshera-backend/.env` (or in the
  shell). Routes are absent otherwise — production builds cannot
  accidentally expose this surface.
* **Frontend**: `VITE_ENABLE_VIEWPORT_BRIDGE=1` in `roshera-app/.env.local`.
  The component renders nothing when the flag is off.

Optional overrides:

* `VITE_VIEWPORT_BRIDGE_URL=ws://localhost:8081/ws/viewport-bridge` if the
  Vite dev server runs on a different host than the api-server.

## Endpoints

All `application/json`. Defaults assume `localhost:8081`.

### `GET /api/viewport/status`

```
{ "connected": true, "pending_requests": 0 }
```

### `POST /api/viewport/snapshot`

Body (all fields optional):

```jsonc
{
  "name": "sphere_iso",            // → target/snapshots/sphere_iso.png
  "out_dir": "target/snapshots",   // override output dir
  "width": 1024, "height": 768     // hints; renderer respects its canvas size
}
```

Response:

```json
{ "path": "C:/.../target/snapshots/sphere_iso.png",
  "width": 1024, "height": 768, "size_bytes": 91234 }
```

### `POST /api/viewport/camera`

```json
{ "position": [20, 15, 20],
  "target":   [0, 0, 0],
  "up":       [0, 1, 0] }
```

Internally this drives the existing `CameraController` animation pipeline
(0.6 s ease-out cubic). The endpoint waits ~700 ms before acking so the
caller can immediately follow up with `snapshot`.

### `POST /api/viewport/load_stl`

```json
{ "path": "C:/path/to/sphere.stl",
  "name": "sphere",
  "replace_scene": true }
```

The server reads the file from its own filesystem, base64-encodes it, and
streams it to the frontend over the WebSocket. The frontend parses it via
`STLLoader` and pushes a `CADObject` into the scene store, so the result
appears alongside the user's normal scene.

### `POST /api/viewport/shading`

```json
{ "mode": "lit" }   // or "wireframe", "normals"
```

### `POST /api/viewport/clear`

Empty body. Drops every object in the scene store.

## Suggested workflow (Claude)

```bash
# Confirm the frontend is connected.
curl -s localhost:8081/api/viewport/status

# Drop the sphere.stl into the scene.
curl -s -X POST localhost:8081/api/viewport/load_stl \
  -H 'Content-Type: application/json' \
  -d '{"path": "../roshera-app/public/demos/primitives/sphere.stl"}'

# Frame an isometric view + snapshot.
curl -s -X POST localhost:8081/api/viewport/camera \
  -H 'Content-Type: application/json' \
  -d '{"position":[60,45,60],"target":[0,0,0],"up":[0,1,0]}'

curl -s -X POST localhost:8081/api/viewport/snapshot \
  -H 'Content-Type: application/json' \
  -d '{"name":"sphere_iso"}'
# → { "path": ".../target/snapshots/sphere_iso.png", ... }
# Read that PNG with the Read tool — Claude is multimodal.

# Orbit + re-snap.
curl -s -X POST localhost:8081/api/viewport/camera \
  -d '{"position":[0,0,80],"target":[0,0,0],"up":[0,1,0]}' \
  -H 'Content-Type: application/json'
curl -s -X POST localhost:8081/api/viewport/snapshot \
  -d '{"name":"sphere_front"}' -H 'Content-Type: application/json'
```

## Errors

| HTTP | Meaning |
|------|---------|
| 503  | No viewport client connected (frontend not running or flag off). |
| 504  | Frontend didn't reply within 10 s. |
| 502  | Frontend reported an error (payload in body). |

## Limits

* One frontend connection at a time. A second connection replaces the first.
* `set_camera` resolves *after* the 0.6 s animation; chained calls
  serialize naturally because each REST handler awaits its own reply.
* PNG payloads travel base64 over WebSocket — fine up to a few MB; for
  4K screenshots, consider raising `tokio-tungstenite`'s default frame
  cap if needed (currently 16 MB / 64 MB which is plenty).
