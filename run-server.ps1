# Script para iniciar o servidor Rust
# Uso: .\run-server.ps1
# Nota: O frontend React roda separadamente (npm run dev no diretório dashboard)

Write-Host "Starting Rust server..." -ForegroundColor Cyan
Write-Host "Backend API will be available at http://localhost:8080" -ForegroundColor Green
Write-Host "To run the frontend separately, use: cd dashboard && npm run dev" -ForegroundColor Yellow
cargo run -p ferres-db-server
