# Docker Setup - FerresDB

Este projeto usa Docker Compose para rodar o backend (Rust) e frontend (React) em serviços separados.

## Estrutura

- **Backend (Rust)**: Porta `8080` - API REST
- **Frontend (React)**: 
  - Desenvolvimento: Porta `5173` (Vite dev server)
  - Produção: Porta `3000` (Nginx)

## Desenvolvimento

Para rodar em modo desenvolvimento (com hot reload):

```bash
docker-compose -f docker-compose.dev.yml up --build
```

Isso vai:
- Rodar o backend Rust na porta `8080`
- Rodar o frontend React com Vite na porta `5173` (hot reload habilitado)
- Frontend acessível em `http://localhost:5173`
- API acessível em `http://localhost:8080`

### Desenvolvimento Local (sem Docker)

**Backend:**
```bash
cargo run -p ferres-db-server
# API em http://localhost:8080
```

**Frontend:**
```bash
cd dashboard
npm run dev
# Frontend em http://localhost:5173
```

Configure o `.env` do dashboard:
```env
VITE_API_BASE_URL=http://localhost:8080
VITE_API_KEY=sk-dev-abc123
```

## Produção

Para rodar em modo produção:

```bash
docker-compose up --build
```

Isso vai:
- Rodar o backend Rust na porta `8080`
- Buildar e servir o frontend React via Nginx na porta `3000`
- Frontend acessível em `http://localhost:3000`
- API acessível em `http://localhost:8080`

## Variáveis de Ambiente

### Backend

Configure via `config.toml` ou variáveis de ambiente:
- `HOST`: Host do servidor (padrão: `0.0.0.0`)
- `PORT`: Porta do servidor (padrão: `8080`)
- `STORAGE_PATH`: Caminho para dados (padrão: `./data`)
- `LOG_LEVEL`: Nível de log (padrão: `info`)

### Frontend

Configure via `.env`:
- `VITE_API_BASE_URL`: URL da API (padrão: `http://localhost:8080`)
- `VITE_API_KEY`: Chave de API (opcional)

## Comandos Úteis

```bash
# Parar todos os serviços
docker-compose down

# Parar e remover volumes
docker-compose down -v

# Ver logs
docker-compose logs -f

# Ver logs de um serviço específico
docker-compose logs -f backend
docker-compose logs -f frontend

# Rebuild sem cache
docker-compose build --no-cache

# Executar comandos no container
docker-compose exec backend /app/ferres-db-server --help
docker-compose exec frontend npm run build
```

## Troubleshooting

### Frontend não consegue conectar na API

1. Verifique se o backend está rodando: `curl http://localhost:8080/health`
2. Verifique a variável `VITE_API_BASE_URL` no `.env` do dashboard
3. Verifique se o CORS está configurado corretamente no backend

### Portas já em uso

Altere as portas no `docker-compose.yml`:
```yaml
ports:
  - "8081:8080"  # Backend na porta 8081
  - "3001:80"    # Frontend na porta 3001
```

### Rebuild necessário

Se houver mudanças no código:
```bash
docker-compose up --build
```
