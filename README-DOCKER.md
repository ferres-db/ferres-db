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

## 📦 Publicar no Docker Hub (GitHub Actions)

O workflow `.github/workflows/docker-publish.yml` faz build e push das imagens para o Docker Hub.

### Quando roda

- **Push na branch `main`**: build + push das imagens
- **Pull request para `main`**: só build (sem push), para validar
- **Manual**: em **Actions** → **Docker Publish** → **Run workflow**

### Configurar no repositório

1. **Secrets** (Settings → Secrets and variables → Actions):
   - `DOCKERHUB_USERNAME`: seu usuário do Docker Hub
   - `DOCKERHUB_TOKEN`: token de acesso (Access tokens em [Docker Hub](https://hub.docker.com/settings/security))
   - `VITE_API_KEY`: (opcional) chave usada pelo frontend no build

2. **Variável** (opcional):
   - `VITE_API_BASE_URL`: URL da API no build do frontend (padrão: `http://localhost:8080`). Em produção, use a URL pública da sua API.

### Imagens publicadas

- `DOCKERHUB_USERNAME/ferres-db-core`
- `DOCKERHUB_USERNAME/ferres-db-frontend`

Tags: `latest` (apenas em push na `main`), nome da branch e SHA do commit.

### Usar as imagens publicadas

```yaml
# docker-compose usando imagens do Hub em vez de build local
services:
  backend:
    image: SEU_USER/ferres-db-core:latest
    # ...
  frontend:
    image: SEU_USER/ferres-db-frontend:latest
    # ...
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
