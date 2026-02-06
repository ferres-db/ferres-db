#!/bin/sh
set -e

# Config em runtime: gera config.js a partir de variáveis de ambiente
# para que o frontend use a API correta sem rebuild.
API_BASE_URL="${VITE_API_BASE_URL:-http://localhost:8080}"
API_KEY="${VITE_API_KEY:-}"

cat > /usr/share/nginx/html/config.js << EOF
window.__RUNTIME_CONFIG__ = {
  apiBaseUrl: "${API_BASE_URL}",
  apiKey: "${API_KEY}"
};
EOF

exec "$@"
