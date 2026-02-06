# Guia de Desenvolvimento

## 🚀 Executando o Projeto

### Opção 1: Docker Compose (Recomendado)

**Desenvolvimento** (com hot reload):
```bash
docker-compose -f docker-compose.dev.yml up --build
```
- Backend: http://localhost:8080
- Frontend: http://localhost:5173

**Produção**:
```bash
docker-compose up --build
```
- Backend: http://localhost:8080
- Frontend: http://localhost:3000

### Opção 2: Desenvolvimento Local (sem Docker)

**Backend**:
```bash
cargo run -p ferres-db-server
```
- API disponível em: http://localhost:8080

**Frontend** (em outro terminal):
```bash
cd dashboard
npm run dev
```
- Frontend disponível em: http://localhost:5173

**Configure o `.env` do dashboard**:
```env
VITE_API_BASE_URL=http://localhost:8080
VITE_API_KEY=sk-dev-abc123
```

### Opção 3: Scripts PowerShell/Batch

**Windows**:
```powershell
.\run-server.ps1  # Builda frontend e inicia backend
```

**Ou manualmente**:
```bash
# 1. Build do frontend
cd dashboard
npm run build
cd ..

# 2. Iniciar servidor Rust
cargo run -p ferres-db-server
```

## 📁 Estrutura do Projeto

- `crates/server/` - Servidor Rust (Axum) - Porta 8080
- `dashboard/` - Frontend React (Vite + TypeScript)
  - Desenvolvimento: Porta 5173 (Vite dev server)
  - Produção: Porta 3000 (Nginx)

## 🔧 Configuração

### Backend (Rust)

Configure via `config.toml` ou variáveis de ambiente:
- `HOST`: Host do servidor (padrão: `0.0.0.0`)
- `PORT`: Porta do servidor (padrão: `8080`)
- `STORAGE_PATH`: Caminho para dados
- `LOG_LEVEL`: Nível de log (`info`, `debug`, etc.)

### Frontend (React)

Configure via `dashboard/.env`:
- `VITE_API_BASE_URL`: URL da API (padrão: `http://localhost:8080`)
- `VITE_API_KEY`: Chave de API (opcional)

## 📝 Notas Importantes

- **Desenvolvimento**: Frontend e backend rodam separadamente em portas diferentes
- **Produção**: Frontend é servido via Nginx, backend continua na porta 8080
- **CORS**: Backend está configurado para aceitar requisições de qualquer origem
- **Hot Reload**: Disponível apenas em desenvolvimento (Vite dev server)
