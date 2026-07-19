# Roshera CAD — frontend image.
#
# The Rust backend serves NO static files (there is no tower_http ServeDir in
# api-server); it is a pure REST + WebSocket surface. In production the built
# Vite SPA is therefore served by this nginx image, which also reverse-proxies
# /api and /ws to the backend so the browser talks to a single origin.
#
# Same-origin proxying means the app's REST calls work with an EMPTY
# VITE_API_URL (the code falls back to relative "/api" — see
# src/components/layout/ToolBar.tsx: `${import.meta.env.VITE_API_URL || ''}/api`).
# For the WebSocket, the client reads VITE_WS_URL and falls back to an absolute
# dev default (src/lib/ws-client.ts:
# `import.meta.env.VITE_WS_URL || 'ws://localhost:8081/ws'`). We bake a
# root-relative "/ws" instead: modern browsers resolve a relative WebSocket URL
# against the page origin, auto-selecting ws:// on http and wss:// on https, so
# a single image works on any host and behind TLS with no per-partner rebuild.
# nginx proxies that /ws path to the backend. An absolute ws(s):// URL can still
# be supplied via the build arg if a split origin is ever needed.

# ---------------------------------------------------------------------------
# Build stage
# ---------------------------------------------------------------------------
FROM node:20-bookworm-slim AS builder

WORKDIR /app

# Vite inlines VITE_* at build time, so these must be present before `build`.
ARG VITE_API_URL=""
ARG VITE_WS_URL="/ws"
ENV VITE_API_URL=${VITE_API_URL}
ENV VITE_WS_URL=${VITE_WS_URL}

COPY package.json package-lock.json ./
RUN npm ci

COPY . .
# `npm run build` == `tsc -b && vite build`; output lands in dist/.
RUN npm run build

# ---------------------------------------------------------------------------
# Runtime stage
# ---------------------------------------------------------------------------
FROM nginx:1.27-alpine

COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 80

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget -q --spider http://localhost/ || exit 1

CMD ["nginx", "-g", "daemon off;"]
