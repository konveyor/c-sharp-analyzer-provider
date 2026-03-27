//! Observability infrastructure: OpenTelemetry traces (OTLP) and Prometheus metrics.
//!
//! ## Configuration
//!
//! All telemetry is opt-in via environment variables:
//!
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`: Enable OTLP trace export (e.g. `http://localhost:4317`).
//!   When unset, no tracer is created and there is zero overhead.
//!   `OTEL_SERVICE_NAME` can override the default service name (`c-sharp-provider`).
//!
//! - `METRICS_PORT`: Enable Prometheus metrics HTTP server on this port
//!   (e.g. `9090` serves `GET /metrics` on `0.0.0.0:9090`).
//!   When unset, no HTTP server is started.

use once_cell::sync::Lazy;
use opentelemetry::propagation::TextMapPropagator;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::{propagation::TraceContextPropagator, trace::SdkTracerProvider, Resource};
use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts,
    Registry, TextEncoder,
};
use tracing::info;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

const DEFAULT_SERVICE_NAME: &str = "c-sharp-provider";

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Application metrics, registered on a shared Prometheus registry.
pub struct Metrics {
    /// Total gRPC requests, labelled by `method` and `status`.
    pub grpc_requests_total: IntCounterVec,
    /// gRPC request duration in seconds, labelled by `method`.
    pub grpc_request_duration_seconds: HistogramVec,
    /// Total number of evaluate result incidents returned.
    pub evaluate_results_total: IntCounter,
    /// Duration of project initialization in seconds.
    pub init_duration_seconds: Histogram,
    /// Duration of stack graph construction in seconds.
    pub graph_build_duration_seconds: Histogram,
    /// Number of files currently indexed in the stack graph.
    pub files_indexed: IntGauge,
    /// Number of resolved dependencies.
    pub dependency_count: IntGauge,
    /// Duration of a single assembly decompilation in seconds.
    pub decompile_duration_seconds: Histogram,
}

/// Global Prometheus registry shared by the metrics server and recording sites.
pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

/// Global application metrics instance.
pub static METRICS: Lazy<Metrics> = Lazy::new(|| {
    let m = Metrics {
        grpc_requests_total: IntCounterVec::new(
            Opts::new(
                "csharp_provider_grpc_requests_total",
                "Total number of gRPC requests",
            ),
            &["method", "status"],
        )
        .expect("metric can be created"),

        grpc_request_duration_seconds: HistogramVec::new(
            HistogramOpts::new(
                "csharp_provider_grpc_request_duration_seconds",
                "gRPC request duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0, 60.0, 300.0]),
            &["method"],
        )
        .expect("metric can be created"),

        evaluate_results_total: IntCounter::new(
            "csharp_provider_evaluate_results_total",
            "Total number of evaluate result incidents returned",
        )
        .expect("metric can be created"),

        init_duration_seconds: Histogram::with_opts(
            HistogramOpts::new(
                "csharp_provider_init_duration_seconds",
                "Duration of project initialization in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]),
        )
        .expect("metric can be created"),

        graph_build_duration_seconds: Histogram::with_opts(
            HistogramOpts::new(
                "csharp_provider_graph_build_duration_seconds",
                "Duration of stack graph construction in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0]),
        )
        .expect("metric can be created"),

        files_indexed: IntGauge::new(
            "csharp_provider_files_indexed",
            "Number of files currently indexed in the stack graph",
        )
        .expect("metric can be created"),

        dependency_count: IntGauge::new(
            "csharp_provider_dependency_count",
            "Number of resolved dependencies",
        )
        .expect("metric can be created"),

        decompile_duration_seconds: Histogram::with_opts(
            HistogramOpts::new(
                "csharp_provider_decompile_duration_seconds",
                "Duration of a single assembly decompilation in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0]),
        )
        .expect("metric can be created"),
    };

    REGISTRY
        .register(Box::new(m.grpc_requests_total.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.grpc_request_duration_seconds.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.evaluate_results_total.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.init_duration_seconds.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.graph_build_duration_seconds.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.files_indexed.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.dependency_count.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(m.decompile_duration_seconds.clone()))
        .expect("collector can be registered");

    m
});

// ---------------------------------------------------------------------------
// OpenTelemetry Tracer
// ---------------------------------------------------------------------------

/// Stored tracer provider handle so we can shut it down gracefully.
static TRACER_PROVIDER: Lazy<std::sync::Mutex<Option<SdkTracerProvider>>> =
    Lazy::new(|| std::sync::Mutex::new(None));

/// Initialize an OpenTelemetry OTLP tracer if `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
///
/// Returns `Some(layer)` that should be added to the `tracing_subscriber::Registry`.
/// Returns `None` when the env var is not set (zero overhead).
pub fn init_tracer_layer<S>() -> Option<OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
{
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok()?;

    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_string());

    info!(
        endpoint = %endpoint,
        service = %service_name,
        "Initializing OpenTelemetry OTLP trace exporter"
    );

    // Set the W3C TraceContext propagator so incoming `traceparent` headers
    // are extracted and outgoing requests propagate trace context.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = SpanExporter::builder()
        .with_tonic()
        .build()
        .expect("failed to create OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            Resource::builder()
                .with_service_name(service_name)
                .build(),
        )
        .build();

    let tracer = provider.tracer("c-sharp-provider");

    // Store the provider so we can shut it down later
    if let Ok(mut guard) = TRACER_PROVIDER.lock() {
        *guard = Some(provider);
    }

    Some(tracing_opentelemetry::layer().with_tracer(tracer))
}

/// Flush and shut down the OpenTelemetry tracer provider.
/// Call this before process exit to ensure all buffered spans are exported.
pub fn shutdown_tracer() {
    if let Ok(mut guard) = TRACER_PROVIDER.lock() {
        if let Some(provider) = guard.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("Error shutting down tracer provider: {}", e);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// gRPC Trace Context Propagation
// ---------------------------------------------------------------------------

/// Extractor that reads W3C trace context from tonic request metadata.
/// This allows spans created by the provider to be children of the caller's
/// trace (e.g., konveyor-analyzer), enabling cross-service tracing in Jaeger.
struct MetadataExtractor<'a>(&'a tonic::metadata::MetadataMap);

impl opentelemetry::propagation::Extractor for MetadataExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .filter_map(|k| match k {
                tonic::metadata::KeyRef::Ascii(key) => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Extract the OpenTelemetry context from gRPC request metadata.
///
/// Call this at the start of a gRPC handler to link the handler's spans
/// to the caller's trace. Returns an `OtelContext` that should be attached
/// with `tracing_opentelemetry::OpenTelemetrySpanExt::set_parent`:
///
/// ```ignore
/// use tracing_opentelemetry::OpenTelemetrySpanExt;
/// let cx = telemetry::extract_context(request.metadata());
/// tracing::Span::current().set_parent(cx);
/// ```
pub fn extract_context(metadata: &tonic::metadata::MetadataMap) -> opentelemetry::Context {
    let propagator = TraceContextPropagator::new();
    propagator.extract(&MetadataExtractor(metadata))
}

// ---------------------------------------------------------------------------
// Prometheus Metrics HTTP Server
// ---------------------------------------------------------------------------

/// Start a lightweight HTTP server serving `GET /metrics` in Prometheus text format.
///
/// Reads the `METRICS_PORT` environment variable. If unset, does nothing.
/// The server runs as a background tokio task on `0.0.0.0:{METRICS_PORT}`.
pub fn start_metrics_server() {
    let port = match std::env::var("METRICS_PORT") {
        Ok(p) => match p.parse::<u16>() {
            Ok(port) => port,
            Err(e) => {
                tracing::error!("Invalid METRICS_PORT value '{}': {}", p, e);
                return;
            }
        },
        Err(_) => return, // Not set, do nothing
    };

    // Force-initialize the metrics so they appear even before first use
    Lazy::force(&METRICS);

    info!(port = port, "Starting Prometheus metrics server");

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let addr = format!("0.0.0.0:{}", port);
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Failed to bind metrics server to {}: {}", addr, e);
                return;
            }
        };

        info!(addr = %addr, "Prometheus metrics server listening");

        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!("Metrics server accept error: {}", e);
                    continue;
                }
            };

            tokio::spawn(async move {
                // Read the request (we don't parse it -- any request gets metrics)
                let mut buf = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;

                let encoder = TextEncoder::new();
                let metric_families = REGISTRY.gather();
                let mut body = Vec::new();
                if let Err(e) = encoder.encode(&metric_families, &mut body) {
                    tracing::error!("Failed to encode metrics: {}", e);
                    return;
                }

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
                    encoder.format_type(),
                    body.len()
                );

                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.flush().await;
            });
        }
    });
}
