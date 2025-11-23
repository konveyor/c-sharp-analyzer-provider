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
        if code_snip_request.code_location.is_none() {
            return Err(Status::invalid_argument("no code location sent"));
        }
        let code_location = code_snip_request.code_location.unwrap();
        if code_location.start_position.is_none() {
            return Err(Status::invalid_argument(
                "no code location start position sent",
            ));
        }
        let start_position = code_location.start_position.unwrap();
        if code_location.end_position.is_none() {
            return Err(Status::invalid_argument(
                "no code location end position sent",
            ));
        }
        let end_position = code_location.end_position.unwrap();

        info!(file=%code_snip_request.uri, "getting code snip for {:?}", code_location);
        let file_uri = Url::parse(code_snip_request.uri.clone().as_str());
        if let Err(e) = file_uri {
            return Err(Status::invalid_argument(format!(
                "could not find file requested: {} -- {}",
                e,
                code_snip_request.uri.clone()
            )));
        }

        let file_uri = file_uri.unwrap();
        if file_uri.path().is_empty() {
            return Err(Status::invalid_argument(format!(
                "could not find file requested: {}",
                file_uri
            )));
        }

        let file_path = file_uri.to_file_path();
        if file_path.is_err() {
            return Err(Status::invalid_argument(format!(
                "could not find file requested: {:?}",
                &file_path
            )));
        }
        let file_path = file_path.unwrap();

        let file = File::open(&file_path);
        if file.is_err() {
            return Err(Status::invalid_argument(format!(
                "could not find file requested: {:?}",
                &file_path
            )));
        }
        let file = file.unwrap();
        let file = BufReader::new(file);

        let mut skip_lines: usize = 0;
        if start_position.line as usize >= self.context_lines {
            skip_lines = start_position.line as usize - self.context_lines;
        }
        let take: usize = (end_position.line - start_position.line) as usize + self.context_lines;
        let code_snip_lines: String = file
            .lines()
            .skip(skip_lines)
            .take(take)
            .enumerate()
            .map(|(index, s)| {
                if s.is_err() {
                    "".to_string()
                } else {
                    let s = s.unwrap();
                    format!("{} {}\n", skip_lines + index, s)
                }
            })
            .collect();
        Ok(Response::new(GetCodeSnipResponse {
            snip: code_snip_lines,
        }))
    }
}
