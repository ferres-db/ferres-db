//! # Metrics Handler — handler para endpoint de métricas Prometheus

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;

use crate::metrics::gather_metrics;

/// Handler para GET /metrics
///
/// Retorna métricas Prometheus em formato texto.
pub async fn get_metrics() -> Response<Body> {
    let metrics = gather_metrics();
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; version=0.0.4")
        .body(Body::from(metrics))
        .expect("response builder with valid StatusCode and Body::from(String) never fails")
}
