# Roshera CAD — Partner Instance Deployment

This is the runbook for standing up a **single-tenant partner instance**: one
container stack per design partner, no shared database, no tenancy code. Every
value below has been verified against the source; env vars cite `file:line`.

The stack is three services (`docker-compose.partner.yml`):

| Service    | Image                                          | Role                                             |
|------------|------------------------------------------------|--------------------------------------------------|
| `postgres` | `postgres:16-alpine`                           | Auth/session persistence (users, sessions).      |
| `backend`  | `ghcr.io/varun29ankus/roshera-cad`             | `api-server` — REST + WebSocket, geometry kernel.|
| `frontend` | `ghcr.io/varun29ankus/roshera-cad-frontend`    | nginx: serves the SPA, proxies `/api` and `/ws`. |

The backend serves **no** static files (there is no `ServeDir` in `api-server`);
the frontend nginx image serves the built SPA and reverse-proxies the two
backend surfaces so the browser talks to a single origin.

---

## 1. Prerequisites

- Docker Engine 24+ with Compose v2 (`docker compose`, not `docker-compose`).
- Outbound network to `ghcr.io` to pull images (or build locally from source).
- If pulling private images: a GitHub token with `read:packages`, then
  `echo $TOKEN | docker login ghcr.io -u <user> --password-stdin`.
- A host port for the UI (default `8080`) and, if agents call REST directly,
  the API port (default `8081`).

---

## 2. Environment variables (verified against code)

Copy `.env.partner.example` to `.env` beside the compose file and fill it in.

### Required

| Variable | Read at | Purpose / failure mode |
|----------|---------|------------------------|
| `POSTGRES_PASSWORD` | compose (postgres image + `DATABASE_URL`) | Postgres superuser password; also embedded in `DATABASE_URL`. Compose refuses to start if unset. |
| `DATABASE_URL` | `api-server/src/main.rs:8005` (`std::env::var("DATABASE_URL")`) | Postgres DSN. Compose sets it to `postgresql://roshera:$POSTGRES_PASSWORD@postgres:5432/roshera`. The backend opens the pool and **runs migrations at boot** (`session-manager/src/database.rs:283` → `run_migrations`, `CREATE TABLE IF NOT EXISTS`); if the DB is unreachable the process exits (`main.rs:8017` `PostgresDatabase::new(..).await?`). |
| `ROSHERA_JWT_SECRET` | `session-manager/src/manager.rs:632` | JWT signing key. **Must be set and stable.** Unset/empty ⇒ a random per-process secret is generated (`manager.rs:637-647`) and every issued token is invalidated on restart. Generate with `openssl rand -hex 32`. |

### Optional

| Variable | Read at | Default / effect |
|----------|---------|------------------|
| `ANTHROPIC_API_KEY` | `api-server/src/main.rs:8069` | Unset ⇒ AI command routes return `503 ai_not_configured` (loud, never mock output). |
| `RUST_LOG` | tracing `EnvFilter` | `info`. |
| `ROSHERA_CORS_ALLOWED_ORIGINS` | `api-server/src/main.rs:9392` | Empty. Not needed same-origin (nginx proxy). Comma-separated origins; `*` opens CORS to any origin and logs a warning (`main.rs:9424`). |
| `PUBLIC_WS_URL` (build arg `VITE_WS_URL`) | `roshera-app/src/lib/ws-client.ts:185` | `/ws` (root-relative; the browser resolves it to `ws://`/`wss://` same-origin). Set an absolute `ws(s)://host/ws` only for a split-origin deployment. |
| `HTTP_PORT` / `API_PORT` | compose port maps | `8080` (UI) / `8081` (direct REST/WS). |
| `ROSHERA_OP_TIMEOUT_SECS` (+ per-class `ROSHERA_OP_TIMEOUT_{BOOLEAN,BLEND,OTHER}_SECS`) | `api-server/src/bounded_exec.rs` (`OpBudgets::from_env`, resolved once at startup) | Wall-clock budget for heavy mutating kernel ops (Task #41). Defaults **ON** and generous: boolean/blend 60 s, other 120 s. An op that exceeds its budget runs on a discarded clone and the request returns `504 op_timeout` (non-retryable) while the live model and its write lock stay untouched — a runaway corefinement can no longer pin the instance. Precedence per class: per-class var → global `ROSHERA_OP_TIMEOUT_SECS` → compiled default. A value of `0`/unparseable is ignored (the guard cannot be disabled). |
| `ROSHERA_DURABILITY` | `api-server/src/durability.rs` (`durability_enabled`) | Durability escape hatch (Task #39). Defaults **ON**: when `DATABASE_URL` is present (it always is — boot-critical), every recorded timeline event is persisted to the `timeline_events` table and replayed into the model on boot. Set `ROSHERA_DURABILITY=off` (case-insensitive) to disable persistence + boot replay for a throwaway dev instance that behaves like the pre-durability server (blank on every start). Any other value leaves durability on. Boot outcome is exposed at `GET /api/durability/status` (typed: `active` / `empty` / `quarantined` / `disabled`). |

### Auth posture — secure by default (important)

Authentication is **enforced by default**. `AuthPosture::from_env()`
(`api-server/src/auth_middleware.rs:96`) resolves to `Required` on an empty
environment (`from_env_with`, `auth_middleware.rs:113-125`); every non-exempt
request must carry a valid credential. The public allowlist is `/`, `/health`,
`/api/auth/{login,register,refresh}`, and the `/ws` upgrade
(`auth_middleware.rs:187-192`).

- The **only** opt-out is `ROSHERA_DEV_INSECURE=1` (`auth_middleware.rs:117`),
  which grants every request full permissions without a credential. The partner
  stack **does not set it** — that omission is what keeps the instance secure.
- `ROSHERA_REQUIRE_AUTH` appears in stale code comments but is **dead**: it is
  never read via `std::env::var`. Do not rely on it to gate anything.

---

## 3. First boot

```bash
cp .env.partner.example .env
# edit .env: set POSTGRES_PASSWORD and ROSHERA_JWT_SECRET (openssl rand -hex 32)

docker compose -f docker-compose.partner.yml up -d
```

Startup order is enforced: `backend` waits for `postgres` to pass its
`pg_isready` healthcheck (`depends_on: condition: service_healthy`) because the
backend fails to start if the DB is down. The backend creates its own tables on
first boot (no manual migration step).

Create the first user via the public registration route (auth is enforced, so
you need a credential to do anything else):

```bash
curl -sf -X POST http://<host>:${API_PORT:-8081}/api/auth/register \
  -H 'content-type: application/json' \
  -d '{"email":"partner@example.com","name":"Partner","password":"<strong-password>"}'
```

Then open the UI at `http://<host>:${HTTP_PORT:-8080}/` and log in.

---

## 4. Health verification

```bash
# Backend liveness (auth-exempt route on the real bind port 8081):
curl -sf http://<host>:${API_PORT:-8081}/health && echo OK

# Container-level health (the Dockerfile HEALTHCHECK probes GET /health:8081):
docker compose -f docker-compose.partner.yml ps
#   backend + postgres + frontend should all show "healthy" / "running".

# Frontend is up:
curl -sfI http://<host>:${HTTP_PORT:-8080}/ | head -1
```

If `backend` stays `unhealthy`: check `docker compose logs backend`. The usual
cause is an unreachable DB (it will have exited) or a bad `DATABASE_URL`.

---

## 5. What survives a restart, and what does NOT (durability — be honest)

**Durability Slice 1 (Task #39) is live: the event log is persisted, and the
geometry model is rebuilt by replaying it on boot.** Every recorded kernel
operation (create/extrude/revolve/boolean/fillet/chamfer/transform, assembly
`*` events, and `drawing.create_from_part`) is appended to the Postgres
`timeline_events` table transactionally as it happens (`durability.rs`,
`DatabaseEventSink`), and on boot the log is loaded and replayed into a fresh
`BRepModel` before the server serves (`durability::boot_replay`).

**Survives a restart now:**
- The **geometry** — solids come back with the same shape (re-derived by
  replaying the log; the boolean/fillet/etc. are re-executed, so a bored solid
  is watertight exactly as it was live).
- The **timeline history** — event ids, sequence numbers, and kinds are
  byte-identical after a restart (`GET /api/timeline/history/{branch}`).
- User accounts and login sessions (unchanged — Postgres `users`/`sessions`).

**Does NOT survive yet (honest residual — spec `2026-07-19-durability-design.md`
slices 3–4):**
- **Solid names and colours.** These are written to `Solid::name` /
  `AppState.solid_colors` *outside* any recorded event (spec §2.3), so a
  replayed solid boots with a default name and no colour. **Slice 3** closes
  this by emitting rename/colour events with replay arms.
- **Public UUID identity.** The uuid↔solid mapping is not persisted this slice
  (spec §2.7 classes it derivable-on-replay), so a restored solid gets a *new*
  uuid. Addressing works (list the parts to get current uuids); a uuid held by
  an agent across a restart does not. Stable uuids are **Slice 3**.
- **Labels / GD&T annotations.** Recorded as `label.*`/`gdt.*` events but
  honestly skipped on replay (`NonGeometryStale`); re-materialising them is
  **Slice 3**.
- **Issued API keys / session tokens.** Held in memory (`session-manager`
  `AuthManager`); a restart invalidates them. Persisting them is **Slice 4**.
- **Blackboard notes.** In-memory only.

**Honesty gate.** If the log contains an event this kernel cannot faithfully
replay (an unknown kind, a `sweep_profile`/`loft_profiles` — spec §2.2, or a
corrupt row), the affected document is **quarantined**: the clean prefix up to
the first break is served, the break is named loudly in the logs and at
`GET /api/durability/status`, and the tail is refused — never served as a
subtly-wrong model. A fresh/empty database boots blank exactly as before.

**No snapshots yet** (spec §4.2): boot is a full replay of the log, so a very
large document boots slowly. Snapshots as a replay accelerator are **Slice 2**.
The `roshera-data:/app/data` volume remains reserved for those snapshot blobs.

## 6. Backup story

The durable document lives in Postgres, so a Postgres backup now captures the
geometry + timeline, not just auth:

- **Postgres** (auth/session **and the event log** — `timeline_events`,
  `durable_branches`) via the `pgdata` volume, e.g.
  `docker compose -f docker-compose.partner.yml exec postgres \
   pg_dump -U roshera roshera > roshera-$(date +%F).sql`. Restoring this dump
  into a fresh stack reproduces the document on the next boot (the server
  replays the restored log).
- **Exports** written to the `exports` volume (`/app/exports`) — STL/OBJ/STEP/ROS
  files a user explicitly exported.

Caveat (matches §5): a `pg_dump` restore brings back geometry + timeline, but
solid names/colours, labels/GD&T, and API keys are not yet in the log, so they
do not come back until Slices 3–4 land. A scripted backup/restore runbook with
a fingerprint-verified round-trip is **Slice 5**.

---

## 7. Images & CI

`main`-branch pushes build and push two images to GHCR
(`.github/workflows/ci.yml`, `deploy` job), each tagged with the commit SHA and
`latest`, using the built-in `GITHUB_TOKEN` with `packages: write`:

- `ghcr.io/varun29ankus/roshera-cad` — backend.
- `ghcr.io/varun29ankus/roshera-cad-frontend` — frontend.

To pin a partner to a specific build, replace `:latest` in
`docker-compose.partner.yml` with the SHA tag.
