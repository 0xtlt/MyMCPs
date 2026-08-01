# Production image for self-hosted MyMCPs.
#
# Includes Deno so npm-transport MCP sandboxes work out of the box
# (see app/services/upstream/deno_runner.ts).
#
# Build:  docker build -t mymcps .
# Run:    docker run --rm -p 3333:3333 \
#           -e APP_KEY=... -e APP_URL=http://localhost:3333 \
#           -v mymcps-data:/app/tmp mymcps

ARG NODE_VERSION=24
ARG PNPM_VERSION=11.19.0
ARG DENO_VERSION=2.9.4

# Official Deno binary only (multi-arch).
FROM denoland/deno:bin-${DENO_VERSION} AS deno

# ---------------------------------------------------------------------------
# Dependencies + compile (TypeScript, Vite assets, better-sqlite3 native)
# ---------------------------------------------------------------------------
FROM node:${NODE_VERSION}-bookworm-slim AS build

ARG PNPM_VERSION
ENV PNPM_HOME=/pnpm
ENV PATH=$PNPM_HOME:$PATH
ENV CI=true

RUN apt-get update \
  && apt-get install -y --no-install-recommends python3 make g++ \
  && rm -rf /var/lib/apt/lists/* \
  && corepack enable \
  && corepack prepare "pnpm@${PNPM_VERSION}" --activate

WORKDIR /app

# ace build loads Env — provide placeholders (runtime APP_KEY must be set separately).
ENV NODE_ENV=production \
  PORT=3333 \
  HOST=0.0.0.0 \
  LOG_LEVEL=info \
  APP_KEY=build-only-not-for-runtime-use \
  APP_URL=http://localhost:3333 \
  SESSION_DRIVER=cookie

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile --config.minimumReleaseAge=360

COPY . .
# Assemble a standalone prod tree outside the workspace root so pnpm does not
# walk up to /app (which would skip creating node_modules next to the build).
RUN pnpm run build \
  && mkdir -p /prod \
  && cp -a build/. /prod/ \
  && cp pnpm-workspace.yaml /prod/ \
  && cd /prod \
  && pnpm install --frozen-lockfile --prod --config.minimumReleaseAge=360

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM node:${NODE_VERSION}-bookworm-slim AS runtime

ARG DENO_VERSION
ENV NODE_ENV=production \
  HOST=0.0.0.0 \
  PORT=3333 \
  DENO_PATH=/usr/local/bin/deno \
  DENO_DIR=/app/tmp/deno-cache \
  DENO_VERSION=${DENO_VERSION}

RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates dumb-init \
  && rm -rf /var/lib/apt/lists/*

COPY --from=deno /deno /usr/local/bin/deno

WORKDIR /app

COPY --from=build --chown=node:node /prod ./
COPY --chown=node:node docker-entrypoint.sh /app/docker-entrypoint.sh

RUN chmod +x /app/docker-entrypoint.sh \
  && mkdir -p /app/tmp/mcp-sandboxes /app/tmp/deno-cache \
  && chown -R node:node /app/tmp

USER node

EXPOSE 3333

# Node is already present in the runtime image, so no curl/wget dependency is
# needed for the liveness check.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:' + (process.env.PORT || '3333') + '/health').then((response) => process.exit(response.ok ? 0 : 1)).catch(() => process.exit(1))"

# Persist SQLite (tmp/db.sqlite3) and Deno MCP sandboxes across restarts.
VOLUME ["/app/tmp"]

# dumb-init reaps Deno MCP child processes.
ENTRYPOINT ["dumb-init", "--", "/app/docker-entrypoint.sh"]
CMD ["node", "bin/server.js"]
