#!/bin/sh
set -e

# Cria diretório de dados se STORAGE_PATH for definido (ex.: docker run -e STORAGE_PATH=/data)
if [ -n "$STORAGE_PATH" ]; then
  mkdir -p "$STORAGE_PATH"
fi

exec /app/ferres-db-server "$@"
