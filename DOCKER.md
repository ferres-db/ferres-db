# Docker Setup — FerresDB

This project uses Docker Compose to run the backend (Rust) and frontend (React) as separate services.

## Quick Start

### Development (with hot reload)

```bash
docker-compose -f docker-compose.dev.yml up --build
```

- **Backend**: http://localhost:8080
- **Frontend**: http://localhost:5173 (Vite dev server with hot reload)

### Production

```bash
docker-compose up --build
```

- **Backend**: http://localhost:8080
- **Frontend**: http://localhost:3000 (Nginx)

---

## Project Structure

```
ferres-db-core/
├── docker-compose.yml          # Production
├── docker-compose.dev.yml      # Development
├── Dockerfile                  # Backend (Rust)
└── dashboard/
    ├── Dockerfile              # Frontend React (production)
    ├── Dockerfile.dev          # Frontend React (dev)
    └── nginx.conf              # Nginx configuration
```

- **Backend (Rust)**: Port `8080` — REST API
- **Frontend (React)**:
  - Development: Port `5173` (Vite dev server)
  - Production: Port `3000` (Nginx)

---

## Local Development (without Docker)

**Backend:**
```bash
cargo run -p ferres-db-server
# API at http://localhost:8080
```

**Frontend:**
```bash
cd dashboard
npm run dev
# Frontend at http://localhost:5173
```

Configure `dashboard/.env`:
```env
VITE_API_BASE_URL=http://localhost:8080
VITE_API_KEY=sk-dev-abc123
```

---

## Environment Variables

### Backend

Configure via `config.toml` or environment variables:

| Variable       | Default     | Description                          |
| -------------- | ----------- | ------------------------------------ |
| `HOST`         | `0.0.0.0`   | Server bind host                     |
| `PORT`         | `8080`      | Server port                          |
| `STORAGE_PATH` | `/data`     | Path for persistent data             |
| `LOG_LEVEL`    | `info`      | Log level (`info`, `debug`, etc.)    |

### Frontend

Configure via `dashboard/.env` or as build-time args in the Dockerfile:

| Variable            | Default                  | Description             |
| ------------------- | ------------------------ | ----------------------- |
| `VITE_API_BASE_URL` | `http://localhost:8080`  | API URL                 |
| `VITE_API_KEY`      | —                        | API key (optional)      |

---

## Useful Commands

```bash
# Stop all services
docker-compose down

# Stop and remove volumes
docker-compose down -v

# View logs (all services)
docker-compose logs -f

# View logs of a specific service
docker-compose logs -f backend
docker-compose logs -f frontend

# Full rebuild without cache
docker-compose build --no-cache

# Run commands in a container
docker-compose exec backend /app/ferres-db-server --help
docker-compose exec frontend sh
```

---

## Publishing to Docker Hub (GitHub Actions)

The workflow `.github/workflows/docker-publish.yml` builds and pushes images to Docker Hub.

### When it runs

- **Push to `main` branch**: build + push images
- **Pull request to `main`**: build only (no push), to validate
- **Manual**: in **Actions** → **Docker Publish** → **Run workflow**

### Repository configuration

1. **Secrets** (Settings → Secrets and variables → Actions):
   - `DOCKERHUB_USERNAME`: your Docker Hub username
   - `DOCKERHUB_TOKEN`: access token ([Docker Hub security settings](https://hub.docker.com/settings/security))
   - `VITE_API_KEY`: (optional) key used by the frontend at build time

2. **Variable** (optional):
   - `VITE_API_BASE_URL`: API URL in the frontend build (default: `http://localhost:8080`). In production, use the public URL of your API.

### Published images

- `DOCKERHUB_USERNAME/ferres-db-core`
- `DOCKERHUB_USERNAME/ferres-db-frontend`

Tags: `latest` (only on push to `main`), branch name and commit SHA.

### Using published images

```yaml
# docker-compose using Hub images instead of local build
services:
  backend:
    image: YOUR_USER/ferres-db-core:latest
    # ...
  frontend:
    image: YOUR_USER/ferres-db-frontend:latest
    # ...
```

---

## Troubleshooting

### Frontend cannot connect to the API

1. Check if the backend is running: `curl http://localhost:8080/health`
2. Check the `VITE_API_BASE_URL` variable in the dashboard `.env` or build args
3. Check that CORS is correctly configured in the backend (must allow `Any` origin)

### Ports already in use

Change the ports in `docker-compose.yml`:

```yaml
ports:
  - "8081:8080"  # Backend on port 8081
  - "3001:80"    # Frontend on port 3001
```

### Rebuild required after code changes

```bash
docker-compose up --build
```
