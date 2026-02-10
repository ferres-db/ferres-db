.PHONY: help build run test bench docker-build docker-run docker-stop docker-logs clean

# Variáveis
BINARY_NAME=ferres-db-server
DOCKER_IMAGE=ferres-db-server
DOCKER_CONTAINER=ferres-db

help: ## Mostra esta mensagem de ajuda
	@echo "Comandos disponíveis:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}'

build: ## Compila o projeto em modo release
	@echo "Building $(BINARY_NAME)..."
	cargo build --release --bin $(BINARY_NAME)
	@echo "Build completo! Binário em: target/release/$(BINARY_NAME)"

run: ## Executa o servidor Rust localmente (modo dev)
	@echo "Running $(BINARY_NAME)..."
	@echo "Backend API will be available at http://localhost:8080"
	@echo "To run the frontend separately, use: cd dashboard && npm run dev"
	cargo run --bin $(BINARY_NAME)

test: ## Executa os testes
	@echo "Running tests..."
	cargo test --workspace

bench: ## Compila e executa benchmark padrão (ingest 10k + search 15s)
	@echo "Building ferres-bench..."
	cargo build --release -p ferres-bench
	@echo "Running standard benchmark (ensure server is up at http://localhost:8080)..."
	$(MAKE) bench-ingest bench-search

bench-ingest: ## Apenas ingest: 10k vetores, dim 768, concorrência 20
	cargo run --release -p ferres-bench -- ingest --vectors 10000 --dim 768 --concurrency 20

bench-search: ## Apenas search: 15s, concorrência 50
	cargo run --release -p ferres-bench -- search --duration 15s --concurrency 50

bench-chaos: ## Chaos: 30s com writers/readers mistos
	cargo run --release -p ferres-bench -- chaos --duration 30s --writers 20 --readers 50

docker-build: ## Constrói a imagem Docker
	@echo "Building Docker image $(DOCKER_IMAGE)..."
	docker build -t $(DOCKER_IMAGE):latest .
	@echo "Docker image construída: $(DOCKER_IMAGE):latest"

docker-run: ## Executa os containers Docker em modo produção
	@echo "Starting Docker containers (production)..."
	docker-compose up -d
	@echo "Containers iniciados. Backend: http://localhost:8080, Frontend: http://localhost:3000"
	@echo "Use 'make docker-logs' para ver os logs."

docker-dev: ## Executa os containers Docker em modo desenvolvimento
	@echo "Starting Docker containers (development)..."
	docker-compose -f docker-compose.dev.yml up --build
	@echo "Containers iniciados. Backend: http://localhost:8080, Frontend: http://localhost:5173"

docker-stop: ## Para o container Docker
	@echo "Stopping Docker container..."
	docker-compose down
	@echo "Container parado."

docker-logs: ## Mostra os logs dos containers Docker
	docker-compose logs -f

docker-logs-backend: ## Mostra os logs do backend
	docker-compose logs -f backend

docker-logs-frontend: ## Mostra os logs do frontend
	docker-compose logs -f frontend

docker-shell: ## Abre um shell no container Docker
	docker-compose exec vector-db /bin/bash

clean: ## Limpa arquivos de build
	@echo "Cleaning build artifacts..."
	cargo clean
	@echo "Limpeza concluída."

docker-clean: ## Remove imagens e containers Docker
	@echo "Cleaning Docker artifacts..."
	docker-compose down -v
	docker rmi $(DOCKER_IMAGE):latest 2>/dev/null || true
	@echo "Limpeza Docker concluída."
