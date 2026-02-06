@echo off
REM Script para iniciar o servidor Rust
REM Uso: run-server.bat
REM Nota: O frontend React roda separadamente (npm run dev no diretório dashboard)

echo Starting Rust server...
echo Backend API will be available at http://localhost:8080
echo To run the frontend separately, use: cd dashboard ^&^& npm run dev
cargo run -p ferres-db-server
