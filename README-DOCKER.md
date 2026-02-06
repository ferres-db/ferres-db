# Docker Setup - Guia Rápido

## 🚀 Início Rápido

### Desenvolvimento (com hot reload)

```bash
docker-compose -f docker-compose.dev.yml up --build
```

- **Backend**: http://localhost:8080
- **Frontend**: http://localhost:5173 (Vite dev server com hot reload)

### Produção

```bash
docker-compose up --build
```

- **Backend**: http://localhost:8080
- **Frontend**: http://localhost:3000 (Nginx)

## 📋 Estrutura

```
ferres-db-core/
├── docker-compose.yml          # Produção
├── docker-compose.dev.yml       # Desenvolvimento
├── Dockerfile                   # Backend Rust
└── dashboard/
    ├── Dockerfile              # Frontend React (produção)
    ├── Dockerfile.dev          # Frontend React (dev)
    └── nginx.conf              # Configuração Nginx
```

## 🔧 Configuração

### Variáveis de Ambiente

**Backend** (via `config.toml` ou env vars):
- `HOST`: Host do servidor (padrão: `0.0.0.0`)
- `PORT`: Porta do servidor (padrão: `8080`)
- `STORAGE_PATH`: Caminho para dados (padrão: `/data`)
- `LOG_LEVEL`: Nível de log (`info`, `debug`, etc.)

**Frontend** (build-time no Dockerfile):
- `VITE_API_BASE_URL`: URL da API (padrão: `http://localhost:8080`)
- `VITE_API_KEY`: Chave de API (opcional)

### Para desenvolvimento local (sem Docker)

1. **Backend**:
   ```bash
   cargo run -p ferres-db-server
   ```

2. **Frontend**:
   ```bash
   cd dashboard
   npm run dev
   ```

3. Configure `dashboard/.env`:
   ```env
   VITE_API_BASE_URL=http://localhost:8080
   VITE_API_KEY=sk-dev-abc123
   ```

## 📝 Comandos Úteis

```bash
# Ver logs
docker-compose logs -f

# Parar tudo
docker-compose down

# Rebuild completo
docker-compose build --no-cache

# Executar comandos
docker-compose exec backend /app/ferres-db-server --help
docker-compose exec frontend sh
```

## 🐛 Troubleshooting

### Frontend não conecta na API

1. Verifique se o backend está rodando: `curl http://localhost:8080/health`
2. Verifique o CORS no backend (deve permitir `Any` origem)
3. Verifique `VITE_API_BASE_URL` no `.env` ou build args

### Portas ocupadas

Altere no `docker-compose.yml`:
```yaml
ports:
  - "8081:8080"  # Backend
  - "3001:80"    # Frontend
```
