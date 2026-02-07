#!/bin/sh
set -e

# Se STORAGE_PATH parecer path Windows (C:, C:\...) vindo do host, usar /data no container.
# Evita "mkdir: cannot create directory 'C:'" e path inválido no Linux.
case "$STORAGE_PATH" in
  ""|*\\*|*:*) export STORAGE_PATH=/data ;;
esac

mkdir -p "$STORAGE_PATH"

exec /app/ferres-db-server "$@"
