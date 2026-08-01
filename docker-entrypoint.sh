#!/bin/sh
set -eu

mkdir -p tmp/mcp-sandboxes tmp/deno-cache

if [ -n "${SERVICE_URL_MYMCPS_3333:-}" ]; then
  # Coolify's assigned domain is authoritative for redirects, links, and
  # OAuth callbacks. Keep APP_URL overrides for non-Coolify deployments.
  APP_URL="${SERVICE_URL_MYMCPS_3333%/}"
elif [ -z "${APP_URL:-}" ]; then
  APP_URL="http://localhost:${PORT:-3333}"
fi
export APP_URL

APP_KEY_FILE=/app/tmp/app.key

if [ -z "${APP_KEY:-}" ]; then
  if [ -s "$APP_KEY_FILE" ]; then
    APP_KEY=$(cat "$APP_KEY_FILE")
  else
    umask 077
    APP_KEY="base64:$(node -e "process.stdout.write(require('node:crypto').randomBytes(32).toString('base64'))")"
    printf '%s\n' "$APP_KEY" > "$APP_KEY_FILE"
  fi

  export APP_KEY
fi

# Production migrations require an explicit force flag.
node ace migration:run --force

exec "$@"
