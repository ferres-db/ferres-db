# Stage 1: Builder
# Rust 1.78+ required for Cargo.lock v4
FROM rust:1.83 AS builder

# Instala dependências do sistema necessárias para compilar
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Define o diretório de trabalho
WORKDIR /app

# Copia arquivos de configuração do Cargo (para cache de dependências)
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml ./crates/core/
COPY crates/server/Cargo.toml ./crates/server/
COPY crates/sdk-rust/Cargo.toml ./crates/sdk-rust/

# Cria stubs dummy para compilar dependências (cache layer); core tem lib, bin e bench
RUN mkdir -p crates/core/src/bin crates/core/benches crates/server/src crates/sdk-rust/src && \
    echo "pub fn dummy() {}" > crates/core/src/lib.rs && \
    echo "fn main() {}" > crates/core/src/bin/cli.rs && \
    echo "fn main() {}" > crates/core/benches/performance.rs && \
    echo "fn main() {}" > crates/server/src/main.rs && \
    echo "pub fn dummy() {}" > crates/sdk-rust/src/lib.rs && \
    cargo build --release --bin ferres-db-server && \
    rm -rf crates

# Copia o código fonte real
COPY . .

# Build release otimizado do binário
RUN cargo build --release --bin ferres-db-server

# Stage 2: Runtime
FROM debian:bookworm-slim

# Instala apenas dependências mínimas necessárias para runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Cria usuário não-root para segurança
RUN useradd -m -u 1000 ferres && \
    mkdir -p /data && \
    chown -R ferres:ferres /data

# Define o diretório de trabalho
WORKDIR /app

# Copia o binário compilado do stage builder
COPY --from=builder /app/target/release/ferres-db-server /app/ferres-db-server

# Entrypoint: cria STORAGE_PATH se definido e repassa env ao executar o servidor
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Muda para o usuário não-root
USER ferres

EXPOSE 8080

VOLUME ["/data"]

ENV HOST=0.0.0.0
ENV PORT=8080
ENV STORAGE_PATH=/data
ENV LOG_LEVEL=info

# CORS: origens permitidas (ex.: docker run -e CORS_ORIGINS=https://app.example.com,https://dashboard.example.com)
# Se não definido, usa localhost:3000 e localhost:5173.

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENTRYPOINT ["/entrypoint.sh"]
CMD ["/app/ferres-db-server"]
