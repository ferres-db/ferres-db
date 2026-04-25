# Stage 1: cargo-chef planner — gera recipe.json com o grafo de dependências
FROM rust:bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: builder — compila deps (layer cacheável) e depois o binário final
FROM chef AS builder

# Dependências do sistema para compilação
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# 1) Compila apenas as dependências (camada cached enquanto Cargo.toml/lock não mudar)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --bin ferres-db-server --recipe-path recipe.json

# 2) Copia o código-fonte e compila o binário final
COPY . .
RUN cargo build --release --bin ferres-db-server

# Stage 3: imagem de runtime mínima
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    gosu \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1000 ferres

WORKDIR /app

COPY --from=builder /app/target/release/ferres-db-server /app/ferres-db-server

COPY entrypoint.sh /entrypoint.sh
RUN sed -i 's/\r$//' /entrypoint.sh && chmod +x /entrypoint.sh

EXPOSE 8080
VOLUME ["/data"]

ENV HOST=0.0.0.0
ENV PORT=8080
ENV STORAGE_PATH=/data
ENV LOG_LEVEL=info
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1
ENV FERRESDB_ENABLE_MCP=false

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENTRYPOINT ["/entrypoint.sh"]
CMD ["/app/ferres-db-server"]
