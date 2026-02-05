//! # Dashboard Handler — serve a página HTML do dashboard

use axum::response::Html;

/// HTML do dashboard (single-file, embedado no binário).
const DASHBOARD_HTML: &str = include_str!("../../static/dashboard.html");

/// Handler para GET /dashboard
///
/// Retorna a página HTML do dashboard (Alpine.js, Tailwind, Chart.js).
pub async fn get_dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}
