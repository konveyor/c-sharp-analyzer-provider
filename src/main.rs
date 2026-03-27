mod analyzer_service;
mod c_sharp_graph;
mod pipe_stream;
mod provider;

use std::{
    env::temp_dir,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use clap::{command, Parser};
use tokio::runtime;
use tonic::transport::Server;
use tracing::{debug, info, instrument::WithSubscriber};
use tracing_log::LogTracer;
use tracing_subscriber::{fmt, layer::SubscriberExt, EnvFilter, Layer};

use crate::analyzer_service::proto;
use crate::analyzer_service::{
    provider_code_location_service_server::ProviderCodeLocationServiceServer,
    provider_service_server::ProviderServiceServer,
};
use crate::provider::telemetry;
use crate::provider::CSharpProvider;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long)]
    port: Option<usize>,

    #[arg(long)]
    socket: Option<String>,

    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    log_file: Option<String>,
    #[command(flatten)]
    verbosity: clap_verbosity_flag::Verbosity,
    #[arg(long)]
    db_path: Option<PathBuf>,
    #[arg(long)]
    context_lines: Option<usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Build the Tokio runtime first -- the OTLP exporter needs a runtime context
    // during initialization (it connects to the collector via tonic/gRPC).
    let rt = runtime::Builder::new_multi_thread()
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
            let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
            format!("worker-{}", id)
        })
        // Use available parallelism, capped at 32 threads
        .worker_threads(std::thread::available_parallelism().map_or(4, |n| n.get().min(32)))
        .enable_all()
        .build()?;

    // Enter the runtime context so the OTLP exporter can use tokio
    let _rt_guard = rt.enter();

    // Use RUST_LOG if set, otherwise fall back to the CLI verbosity flag.
    let filter = if std::env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        let level = args
            .verbosity
            .tracing_level()
            .unwrap_or(tracing::Level::INFO);
        EnvFilter::new(level.to_string())
    };

    LogTracer::init_with_filter(tracing_log::log::LevelFilter::Trace)?;

    // Keep the guard alive for the duration of the program.
    // When it's dropped at the end of main(), it will flush remaining logs.
    let _guard;

    // Build the fmt layer (stdout or file)
    let fmt_layer = if let Some(log_file_path) = &args.log_file {
        let file_appender = tracing_appender::rolling::never(
            std::path::Path::new(log_file_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
            std::path::Path::new(log_file_path)
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("output.log")),
        );
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        _guard = Some(guard);
        fmt::layer()
            .with_writer(non_blocking)
            .with_thread_names(true)
            .boxed()
    } else {
        _guard = None;
        fmt::layer().with_thread_names(true).boxed()
    };

    // Build the subscriber with the fmt layer and optional OpenTelemetry layer.
    // Using `Option<Layer>` -- `None` adds zero overhead.
    let otel_layer = telemetry::init_tracer_layer();

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(otel_layer);

    tracing::subscriber::set_global_default(subscriber)?;

    let db_path = args
        .db_path
        .unwrap_or_else(|| temp_dir().join("c_sharp_provider.db"));
    let provider = Arc::new(CSharpProvider::new(
        db_path,
        args.context_lines.unwrap_or(10),
    ));
    let service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1alpha()
        .unwrap();

    // Start the Prometheus metrics HTTP server (no-op if METRICS_PORT is unset)
    rt.block_on(async { telemetry::start_metrics_server() });

    if let Some(port) = args.port {
        let s = format!("[::]:{}", port);
        info!("Using gRPC over HTTP/2 on port {}", s);

        let addr = s.parse()?;

        rt.block_on(async {
            if let Err(e) = Server::builder()
                .http2_max_pending_accept_reset_streams(Some(30))
                .add_service(ProviderServiceServer::from_arc(provider.clone()))
                .add_service(ProviderCodeLocationServiceServer::from_arc(
                    provider.clone(),
                ))
                .add_service(service)
                .serve(addr)
                .with_current_subscriber()
                .await
            {
                tracing::error!("gRPC server error: {}", e);
            }
        });
    } else {
        info!("using uds");
        #[cfg(not(windows))]
        {
            debug!("Running on Unix-like OS");

            rt.block_on(async {
                use tokio::net::UnixListener;
                use tokio_stream::wrappers::UnixListenerStream;

                let socket_path =
                    args.socket
                        .expect("either --port or --socket must be specified");
                let uds = match UnixListener::bind(socket_path) {
                    Ok(listener) => listener,
                    Err(err) => {
                        tracing::error!("unable to bind listener: {err}");
                        return;
                    }
                };
                let uds_stream = UnixListenerStream::new(uds);
                if let Err(e) = Server::builder()
                    .http2_keepalive_timeout(Some(Duration::new(20, 0)))
                    .http2_keepalive_interval(Some(Duration::new(7200, 0)))
                    .tcp_keepalive(Some(Duration::new(7200, 0)))
                    .add_service(ProviderServiceServer::from_arc(provider.clone()))
                    .add_service(ProviderCodeLocationServiceServer::from_arc(
                        provider.clone(),
                    ))
                    .add_service(service)
                    .serve_with_incoming(uds_stream)
                    .with_current_subscriber()
                    .await
                {
                    tracing::error!("gRPC server error: {}", e);
                }
            });
        }
        #[cfg(target_os = "windows")]
        {
            debug!("Using Windows OS");
            use crate::pipe_stream::get_named_pipe_connection_stream;
            rt.block_on(async {
                if let Err(e) = Server::builder()
                    .http2_keepalive_timeout(Some(Duration::new(20, 0)))
                    .http2_keepalive_interval(Some(Duration::new(7200, 0)))
                    .tcp_keepalive(Some(Duration::new(7200, 0)))
                    .add_service(ProviderServiceServer::from_arc(provider.clone()))
                    .add_service(ProviderCodeLocationServiceServer::from_arc(
                        provider.clone(),
                    ))
                    .add_service(service)
                    .serve_with_incoming(get_named_pipe_connection_stream(args.socket.unwrap()))
                    .with_current_subscriber()
                    .await
                {
                    tracing::error!("gRPC server error: {}", e);
                }
            });
        }
    }

    // Flush any buffered traces before exit
    telemetry::shutdown_tracer();

    Ok(())
}
