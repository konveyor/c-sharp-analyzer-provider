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
use tracing::{info, trace};
use url::Url;

#[async_trait]
impl ProviderCodeLocationService for CSharpProvider {
    async fn get_code_snip(
        &self,
        request: Request<GetCodeSnipRequest>,
    ) -> Result<Response<GetCodeSnipResponse>, Status> {
        trace!("request: {:#?}", request);
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

        let file = File::open(&file_path)
            .map_err(|_| Status::invalid_argument(format!(
                "could not open file: {:?}", file_path
            )))?;
        let file = BufReader::new(file);

        let skip_lines = (start_position.line as usize).saturating_sub(self.context_lines);
        let take = (end_position.line - start_position.line) as usize + self.context_lines;

        let code_snip_lines: String = file
            .lines()
            .skip(skip_lines)
            .take(take)
            .enumerate()
            .map(|(index, s)| match s {
                Ok(line) => format!("{} {}\n", skip_lines + index, line),
                Err(_) => String::new(),
            })
            .collect();

        Ok(Response::new(GetCodeSnipResponse {
            snip: code_snip_lines,
        }))
    }
}
