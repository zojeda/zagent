use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, Layer, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature = "otel")]
use opentelemetry::{KeyValue, trace::TracerProvider as _};
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "otel")]
use opentelemetry_sdk::{Resource, runtime, trace as sdktrace};

/// Guard that must be held alive for non-blocking log writer.
/// When dropped, buffered logs are flushed.
pub struct TracingGuard {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
    #[cfg(feature = "otel")]
    otel_enabled: bool,
}

#[cfg(feature = "otel")]
impl Drop for TracingGuard {
    fn drop(&mut self) {
        if self.otel_enabled {
            opentelemetry::global::shutdown_tracer_provider();
        }
    }
}

/// Initialize multi-layer tracing:
/// 1. Terminal layer: compact, colored, INFO+ (or TRACE in verbose mode)
/// 2. JSON file layer: structured, TRACE level, daily rolling
/// 3. Optional OTLP exporter layer (when `otel` feature enabled)
pub fn init_tracing(log_dir: &str, verbose: bool) -> TracingGuard {
    let default_info_filter = default_app_filter("info");
    let default_trace_filter = default_app_filter("trace");

    let terminal_filter = if verbose {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| default_trace_filter.clone())
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| default_info_filter.clone())
    };

    let terminal_layer = fmt::layer()
        .compact()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .with_filter(terminal_filter);

    let file_appender = rolling::daily(log_dir, "zagent-server.log");
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

    let json_filter = default_trace_filter.clone();
    let json_layer = fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_span_list(true)
        .with_filter(json_filter);

    #[cfg(feature = "otel")]
    {
        if let Some(tracer) = build_otel_tracer() {
            let otel_filter = default_trace_filter;
            tracing_subscriber::registry()
                .with(terminal_layer)
                .with(json_layer)
                .with(
                    tracing_opentelemetry::layer()
                        .with_tracer(tracer)
                        .with_filter(otel_filter),
                )
                .init();

            return TracingGuard {
                _file_guard: file_guard,
                otel_enabled: true,
            };
        }
    }

    tracing_subscriber::registry()
        .with(terminal_layer)
        .with(json_layer)
        .init();

    TracingGuard {
        _file_guard: file_guard,
        #[cfg(feature = "otel")]
        otel_enabled: false,
    }
}

fn default_app_filter(level: &str) -> EnvFilter {
    let directives = format!(
        "zagent={level},zagent_core={level},zagent_backend={level},zagent_server={level},hyper=off,h2=off,reqwest=off,tower=off,tonic=off,rustls=off"
    );
    EnvFilter::new(directives)
}

#[cfg(feature = "otel")]
fn build_otel_tracer() -> Option<sdktrace::Tracer> {
    let endpoint = match std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return None,
    };

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "zagent-server".to_string());

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .ok()?;

    let provider = sdktrace::TracerProvider::builder()
        .with_batch_exporter(exporter, runtime::Tokio)
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            service_name,
        )]))
        .build();

    opentelemetry::global::set_tracer_provider(provider.clone());
    Some(provider.tracer("zagent-server"))
}
