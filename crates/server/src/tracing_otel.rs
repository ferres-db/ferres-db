//! # OpenTelemetry tracing (feature `otel`)
//!
//! Quando a feature `otel` está habilitada, os spans do `tracing` são exportados
//! para um backend OTLP (Jaeger, Collector, etc.). Configure `OTEL_EXPORTER_OTLP_ENDPOINT`
//! (ex.: `http://localhost:4317`) para ativar o envio.
//!
//! Hierarquia de spans: `http_request` → `search_points` / `search_hybrid` →
//! `validate_query`, `hnsw_search` / `hybrid_search`, `hydrate_results`.

#[cfg(feature = "otel")]
use opentelemetry::global;
#[cfg(feature = "otel")]
use opentelemetry::trace::TracerProvider as _;
#[cfg(feature = "otel")]
use opentelemetry_sdk::{
    trace::{BatchSpanProcessor, SdkTracerProvider},
    Resource,
};
#[cfg(feature = "otel")]
use tracing_subscriber::Registry;

/// Inicializa o tracer OTLP e retorna o layer para o subscriber e o provider (manter vivo).
/// Endpoint: env `OTEL_EXPORTER_OTLP_ENDPOINT` (ex.: `http://localhost:4317`).
#[cfg(feature = "otel")]
pub fn init_otel_tracing() -> Result<(impl tracing_subscriber::Layer<Registry> + Send + Sync, SdkTracerProvider), Box<dyn std::error::Error + Send + Sync>> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_span_processor(BatchSpanProcessor::builder(exporter).build())
        .with_resource(
            Resource::builder()
                .with_service_name("ferres-db-server")
                .build(),
        )
        .build();

    global::set_tracer_provider(provider.clone());
    let tracer = provider.tracer("ferres-db-server");

    let layer = tracing_opentelemetry::layer().with_tracer(tracer);
    Ok((layer, provider))
}
