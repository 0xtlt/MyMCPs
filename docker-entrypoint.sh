#!/bin/sh
set -eu

mkdir -p tmp/mcp-sandboxes tmp/deno-cache

if [ -z "${APP_KEY:-}" ]; then
  echo "error: APP_KEY is required (generate with: node ace generate:key)" >&2
  exit 1
fi

# Production migrations require an explicit force flag.
node ace migration:run --force

exec "$@"
