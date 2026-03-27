use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use crate::{
    analyzer_service::{
        provider_code_location_service_server::ProviderCodeLocationService, GetCodeSnipRequest,
        GetCodeSnipResponse,
    },
    provider::CSharpProvider,
};
use tonic::{async_trait, Request, Response, Status};
use tracing::{info, instrument, trace};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use url::Url;

use crate::provider::telemetry::{self, METRICS};

#[async_trait]
impl ProviderCodeLocationService for CSharpProvider {
    #[instrument(skip_all, name = "grpc.get_code_snip")]
    async fn get_code_snip(
        &self,
        request: Request<GetCodeSnipRequest>,
    ) -> Result<Response<GetCodeSnipResponse>, Status> {
        tracing::Span::current().set_parent(telemetry::extract_context(request.metadata()));
        trace!("request: {:#?}", request);
        let _timer = METRICS.grpc_request_duration_seconds
            .with_label_values(&["get_code_snip"]).start_timer();
        let code_snip_request = request.into_inner();

        let code_location = code_snip_request
            .code_location
            .ok_or_else(|| Status::invalid_argument("no code location sent"))?;

        let start_position = code_location
            .start_position
            .ok_or_else(|| Status::invalid_argument("no code location start position sent"))?;

        let end_position = code_location
            .end_position
            .ok_or_else(|| Status::invalid_argument("no code location end position sent"))?;

        info!(file=%code_snip_request.uri, "getting code snip for {:?}", code_location);

        let file_uri = Url::parse(&code_snip_request.uri)
            .map_err(|e| Status::invalid_argument(format!(
                "could not parse file URI: {} -- {}", e, code_snip_request.uri
            )))?;

        if file_uri.path().is_empty() {
            return Err(Status::invalid_argument(format!(
                "could not find file requested: {}", file_uri
            )));
        }

        let file_path = file_uri.to_file_path()
            .map_err(|_| Status::invalid_argument(format!(
                "could not convert URI to file path: {}", file_uri
            )))?;

        let context_lines = self.context_lines;
        let skip_lines = (start_position.line as usize).saturating_sub(context_lines);
        let take = (end_position.line - start_position.line) as usize + context_lines;

        // Run blocking file I/O on a dedicated thread to avoid blocking the tokio runtime
        let span = tracing::Span::current();
        let code_snip_lines = tokio::task::spawn_blocking(move || -> Result<String, Status> {
            let _guard = span.enter();
            let file = File::open(&file_path)
                .map_err(|_| Status::invalid_argument(format!(
                    "could not open file: {:?}", file_path
                )))?;
            let reader = BufReader::new(file);

            let result: String = reader
                .lines()
                .skip(skip_lines)
                .take(take)
                .enumerate()
                .map(|(index, s)| match s {
                    Ok(line) => format!("{} {}\n", skip_lines + index, line),
                    Err(_) => String::new(),
                })
                .collect();

            Ok(result)
        })
        .await
        .map_err(|e| Status::internal(format!("code snip task panicked: {}", e)))??;

        METRICS.grpc_requests_total.with_label_values(&["get_code_snip", "ok"]).inc();

        Ok(Response::new(GetCodeSnipResponse {
            snip: code_snip_lines,
        }))
    }
}
