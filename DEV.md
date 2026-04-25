# Development Guide

## Running the Project

### Option 1: Docker Compose (Recommended)

**Development** (with hot reload):
```bash
docker-compose -f docker-compose.dev.yml up --build
```
- Backend: http://localhost:8080
- Frontend: http://localhost:5173

**Production**:
```bash
docker-compose up --build
```
- Backend: http://localhost:8080
- Frontend: http://localhost:3000

### Option 2: Local Development (without Docker)

**Backend**:
```bash
cargo run -p ferres-db-server
```
- API available at: http://localhost:8080

**Frontend** (in another terminal):
```bash
cd dashboard
npm run dev
```
- Frontend available at: http://localhost:5173

**Configure the dashboard `.env`**:
```env
VITE_API_BASE_URL=http://localhost:8080
VITE_API_KEY=sk-dev-abc123
```

### Option 3: PowerShell/Batch Scripts

**Windows**:
```powershell
.\run-server.ps1  # Builds frontend and starts backend
```

**Or manually**:
```bash
# 1. Build the frontend
cd dashboard
npm run build
cd ..

# 2. Start the Rust server
cargo run -p ferres-db-server
```

## Project Structure

- `crates/server/` — Rust server (Axum) — Port 8080
- `dashboard/` — React frontend (Vite + TypeScript)
  - Development: Port 5173 (Vite dev server)
  - Production: Port 3000 (Nginx)

## Configuration

### Backend (Rust)

Configure via `config.toml` or environment variables:
- `HOST`: Server host (default: `0.0.0.0`)
- `PORT`: Server port (default: `8080`)
- `STORAGE_PATH`: Data path
- `LOG_LEVEL`: Log level (`info`, `debug`, etc.)

### Frontend (React)

Configure via `dashboard/.env`:
- `VITE_API_BASE_URL`: API URL (default: `http://localhost:8080`)
- `VITE_API_KEY`: API key (optional)

## Important Notes

- **Development**: Frontend and backend run separately on different ports
- **Production**: Frontend is served via Nginx, backend stays on port 8080
- **CORS**: Backend is configured to accept requests from any origin
- **Hot Reload**: Available only in development (Vite dev server)
