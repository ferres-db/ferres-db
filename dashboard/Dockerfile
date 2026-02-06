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

# Copia arquivos buildados
COPY --from=builder /app/dist /usr/share/nginx/html

# Copia configuração customizada do nginx
COPY nginx.conf /etc/nginx/conf.d/default.conf

# Expõe porta 80
EXPOSE 80

# Comando padrão do nginx
CMD ["nginx", "-g", "daemon off;"]
