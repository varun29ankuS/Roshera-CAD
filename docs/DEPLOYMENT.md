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

## 5. What does NOT survive a restart (durability gap — be honest)

**The geometry model and the design timeline are in-memory only.** The kernel
model is an `Arc<RwLock<BRepModel>>` built fresh at boot (`main.rs:7998`), and
the timeline/branches live in in-memory `DashMap`s. Postgres persists **only**
the auth/session tables (`users`, `sessions` — `database.rs:290+`).

Consequence: **restarting the `backend` container loses all geometry and
timeline history.** User accounts and login sessions survive (they are in
Postgres); nothing you modeled does. The `roshera-data:/app/data` volume and the
`/app/data` directory in the image are **reserved mount points for the future
durability slice** (snapshot + event-log storage) — they exist so the storage
path is stable when that lands, but **nothing writes to them today.**

Treat a partner instance as ephemeral for geometry until the durability slice
ships. Do not promise a partner that their models persist across a restart.

## 6. Backup story

**There is no application-level backup today.** What you can back up:

- **Postgres** (auth/session only) via the `pgdata` volume, e.g.
  `docker compose -f docker-compose.partner.yml exec postgres \
   pg_dump -U roshera roshera > roshera-auth-$(date +%F).sql`.
- **Exports** written to the `exports` volume (`/app/exports`) — STL/OBJ/STEP/ROS
  files a user explicitly exported.

Geometry/timeline state cannot be backed up because it is not persisted. A real
backup story arrives with the durability slice; until then, this section is
deliberately short and honest rather than aspirational.

---

## 7. Images & CI

`main`-branch pushes build and push two images to GHCR
(`.github/workflows/ci.yml`, `deploy` job), each tagged with the commit SHA and
`latest`, using the built-in `GITHUB_TOKEN` with `packages: write`:

- `ghcr.io/varun29ankus/roshera-cad` — backend.
- `ghcr.io/varun29ankus/roshera-cad-frontend` — frontend.

To pin a partner to a specific build, replace `:latest` in
`docker-compose.partner.yml` with the SHA tag.
