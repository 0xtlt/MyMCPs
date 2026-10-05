#!/bin/sh
set -eu

# Everything written from here on is private to the app user: the generated
# key, the SQLite database with its encrypted secrets, and the MCP sandboxes.
# The server and its Deno children inherit this.
umask 077

mkdir -p tmp/mcp-sandboxes tmp/deno-cache

APP_KEY_FILE=/app/tmp/app.key

if [ -z "${APP_KEY:-}" ]; then
  if [ -s "$APP_KEY_FILE" ]; then
    APP_KEY=$(cat "$APP_KEY_FILE")
  else
    APP_KEY="base64:$(node -e "process.stdout.write(require('node:crypto').randomBytes(32).toString('base64'))")"
    printf '%s\n' "$APP_KEY" > "$APP_KEY_FILE"
  fi

  export APP_KEY
fi

# Production migrations require an explicit force flag.
node ace migration:run --force

exec "$@"
