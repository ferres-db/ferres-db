# Stage 1: Build
FROM node:20-alpine AS builder

WORKDIR /app

# Aceita build args para variáveis de ambiente
ARG VITE_API_BASE_URL=http://localhost:8080
ARG VITE_API_KEY=

# Define como variáveis de ambiente para o build
ENV VITE_API_BASE_URL=$VITE_API_BASE_URL
ENV VITE_API_KEY=$VITE_API_KEY

# Copia arquivos de dependências
COPY package*.json ./
RUN npm ci

# Copia código fonte
COPY . .

# Build da aplicação
RUN npm run build

# Stage 2: Production
FROM nginx:alpine

# Entrypoint gera config.js em runtime a partir de VITE_API_BASE_URL e VITE_API_KEY
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh

# Copia arquivos buildados
COPY --from=builder /app/dist /usr/share/nginx/html

# Copia configuração customizada do nginx
COPY nginx.conf /etc/nginx/conf.d/default.conf

# Garante que config.js existe (será sobrescrito pelo entrypoint em runtime)
COPY public/config.js /usr/share/nginx/html/config.js

EXPOSE 80

ENTRYPOINT ["/docker-entrypoint.sh"]
CMD ["nginx", "-g", "daemon off;"]
