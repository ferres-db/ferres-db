# Stage 1: Builder
# Rust: 1.78+ for Cargo.lock v4; recent stable for edition2024 (deps como time 0.3.x)
FROM rust:bookworm AS builder

# Instala dependências do sistema necessárias para compilar
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Define o diretório de trabalho
WORKDIR /app

# Copia todo o código (sem estágio de cache com stubs, para evitar lib.rs dummy no build)
COPY . .

# Build release do binário
RUN cargo build --release --bin ferres-db-server

# Stage 2: Runtime
FROM debian:bookworm-slim

# Instala dependências + gosu para rodar o servidor como usuário não-root após ajustar permissões do volume
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    gosu \
    && rm -rf /var/lib/apt/lists/*

# Usuário não-root para rodar o servidor
RUN useradd -m -u 1000 ferres

# Define o diretório de trabalho
WORKDIR /app

# Copia o binário compilado do stage builder
COPY --from=builder /app/target/release/ferres-db-server /app/ferres-db-server

# Entrypoint roda como root para criar/ajustar permissões de STORAGE_PATH, depois exec como ferres
COPY entrypoint.sh /entrypoint.sh
RUN sed -i 's/\r$//' /entrypoint.sh && chmod +x /entrypoint.sh

# Mantém root como USER padrão; o entrypoint faz chown e exec gosu ferres

EXPOSE 8080

VOLUME ["/data"]

ENV HOST=0.0.0.0
ENV PORT=8080
ENV STORAGE_PATH=/data
ENV LOG_LEVEL=info
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# CORS: origens permitidas (ex.: docker run -e CORS_ORIGINS=https://app.example.com,https://dashboard.example.com)
# Se não definido, usa localhost:3000 e localhost:5173.
# MCP: FERRESDB_ENABLE_MCP=true ativa o servidor MCP via STDIO (requer build com --features mcp).
ENV FERRESDB_ENABLE_MCP=false

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENTRYPOINT ["/entrypoint.sh"]
CMD ["/app/ferres-db-server"]
