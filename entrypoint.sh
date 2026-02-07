#!/bin/sh
set -e

case "$STORAGE_PATH" in
  ""|*\\*|*:*) export STORAGE_PATH=/data ;;
esac

mkdir -p "$STORAGE_PATH"
chown -R ferres:ferres "$STORAGE_PATH" 2>/dev/null || true

echo "[entrypoint] STORAGE_PATH=$STORAGE_PATH"
echo "[entrypoint] Starting ferres-db-server..."

# exec para que o servidor seja PID 1 e receba SIGTERM do docker stop
exec /app/ferres-db-server
