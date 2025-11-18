use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};
use tracing::{debug, error, info};
use utoipa::{OpenApi, ToSchema};

use crate::c_sharp_graph::query::{Query, QueryType};
use crate::c_sharp_graph::results::ResultNode;
use crate::c_sharp_graph::NamespaceFQDNNotFoundError;
//use crate::c_sharp_graph::find_node::FindNode;
use crate::provider::AnalysisMode;
use crate::{
    analyzer_service::{
        provider_service_server::ProviderService, CapabilitiesResponse, Capability, Config,
        DependencyDagResponse, DependencyResponse, EvaluateRequest, EvaluateResponse,
        IncidentContext, InitResponse, NotifyFileChangesRequest, NotifyFileChangesResponse,
        ProviderEvaluateResponse, ServiceRequest,
    },
    provider::Project,
};

#[derive(Clone, ToSchema, Deserialize, Default, Debug)]
#[serde(rename_all = "UPPERCASE")]
enum Locations {
    #[default]
    All,
    Method,
    Field,
    Class,
}

#[derive(ToSchema, Deserialize, Debug)]
struct ReferenceCondition {
    pattern: String,
    #[serde(default)]
    location: Locations,
    #[allow(dead_code)]
    file_paths: Option<Vec<String>>,
}

#[derive(ToSchema, Deserialize, Debug)]
struct CSharpCondition {
    referenced: ReferenceCondition,
}

pub struct CSharpProvider {
    pub db_path: PathBuf,
    pub config: Arc<Mutex<Option<Config>>>,
    pub project: Arc<Mutex<Option<Arc<Project>>>>,
    pub context_lines: usize,
}

impl CSharpProvider {
    pub fn new(db_path: PathBuf, context_lines: usize) -> CSharpProvider {
        CSharpProvider {
            db_path,
            config: Arc::new(Mutex::new(None)),
            project: Arc::new(Mutex::new(None)),
            context_lines,
        }
    }
}

#[tonic::async_trait]
impl ProviderService for CSharpProvider {
    async fn capabilities(&self, _: Request<()>) -> Result<Response<CapabilitiesResponse>, Status> {
        // Add Referenced

        #[derive(OpenApi)]
        struct ApiDoc;

        let openapi = ApiDoc::openapi();
        let json = openapi.to_pretty_json();
        if json.is_err() {
            return Err(Status::from_error(Box::new(json.err().unwrap())));
        }

        debug!("returning refernced capability: {:?}", json.ok());

        return Ok(Response::new(CapabilitiesResponse {
            capabilities: vec![Capability {
                name: "referenced".to_string(),
                template_context: None,
            }],
        }));
    }

    async fn init(&self, r: Request<Config>) -> Result<Response<InitResponse>, Status> {
        let mut config_guard = self.config.lock().await;
        let saved_config = config_guard.insert(r.get_ref().clone());

        let analysis_mode = AnalysisMode::from(saved_config.analysis_mode.clone());
        let location = PathBuf::from(saved_config.location.clone());
        let tools = Project::get_tools(&saved_config.provider_specific_config)
            .map_err(|e| Status::invalid_argument(format!("unalble to find tools: {}", e)))?;
        let project = Arc::new(Project::new(
            location,
            self.db_path.clone(),
            analysis_mode,
            tools,
        ));
        let project_lock = self.project.clone();
        let mut project_guard = project_lock.lock().await;
        let _ = project_guard.replace(project.clone());
        drop(project_guard);
        drop(config_guard);

        let project_guard = project_lock.lock().await;
        let project = match project_guard.as_ref() {
            Some(x) => x,
            None => {
                return Err(Status::internal(
                    "unable to create language configuration for project",
                ));
            }
        };

        info!(
            "starting to load project for location: {:?}",
            project.location
        );
        if let Err(e) = project.validate_language_configuration().await {
            error!("unable to create language configuration: {}", e);
            return Err(Status::internal(
                "unable to create language configuration for project",
            ));
        }
        let stats = project.get_project_graph().await.map_err(|err| {
            error!("{:?}", err);
            Status::new(tonic::Code::Internal, "failed")
        })?;
        debug!("loaded files: {:?}", stats);
        let get_deps_handle = project.resolve();

        let res = match get_deps_handle.await {
            Ok(res) => res,
            Err(e) => {
                debug!("unable to get deps: {}", e);
                return Err(Status::internal("unable to resolve dependenies"));
            }
        };
        debug!("got task result: {:?} -- project: {:?}", res, project);
        info!("adding depdencies to stack graph database");
        let res = project.load_to_database().await;
        debug!(
            "loading project to database: {:?} -- project: {:?}",
            res, project
        );

        return Ok(Response::new(InitResponse {
            error: String::new(),
            successful: true,
            id: 4,
            builtin_config: None,
        }));
    }

    async fn evaluate(
        &self,
        r: Request<EvaluateRequest>,
    ) -> Result<Response<EvaluateResponse>, Status> {
        info!("request: {:?}", r);
        let evaluate_request = r.get_ref();
        debug!("evaluate request: {:?}", evaluate_request.condition_info);

        if evaluate_request.cap != "referenced" {
            return Ok(Response::new(EvaluateResponse {
                error: "unable to find referenced capability".to_string(),
                successful: false,
                response: None,
            }));
        }
        let condition: CSharpCondition =
            serde_yml::from_str(evaluate_request.condition_info.as_str()).map_err(|err| {
                error!("{:?}", err);
                Status::new(tonic::Code::Internal, "failed")
            })?;

        debug!("condition: {:?}", condition);
        let project_guard = self.project.lock().await;
        let project = match project_guard.as_ref() {
            Some(x) => x,
            None => {
                return Ok(Response::new(EvaluateResponse {
                    error: "project may not be initialized".to_string(),
                    successful: false,
                    response: None,
                }));
            }
        };
        let graph_guard = project.graph.clone();

        let source_type = match project.get_source_type().await {
            Some(s) => s,
            None => {
                return Ok(Response::new(EvaluateResponse {
                    error: "project may not be initialized".to_string(),
                    successful: false,
                    response: None,
                }));
            }
        };
        // Release the project lock, so other evaluate calls can continue
        drop(project_guard);
        let graph = graph_guard.lock();
        let graph_option = match graph {
            Ok(g) => g,
            Err(e) => {
                graph_guard.clear_poison();
                e.into_inner()
            }
        };

        let graph = graph_option.as_ref().unwrap();

        // As we are passing an unmutable reference, we can drop the guard here.

        let query = match condition.referenced.location {
            Locations::All => QueryType::All {
                graph,
                source_type: &source_type,
            },
            Locations::Method => QueryType::Method {
                graph,
                source_type: &source_type,
            },
            Locations::Field => QueryType::Field {
                graph,
                source_type: &source_type,
            },
            Locations::Class => QueryType::Class {
                graph,
                source_type: &source_type,
            },
        };
        let results = query.query(condition.referenced.pattern.clone());
        let results = match results {
            Err(e) => {
                if let Some(_e) = e.downcast_ref::<NamespaceFQDNNotFoundError>() {
                    EvaluateResponse {
                        error: String::new(),
                        successful: true,
                        response: Some(ProviderEvaluateResponse {
                            matched: false,
                            incident_contexts: vec![],
                            template_context: None,
                        }),
                    }
                } else {
                    EvaluateResponse {
                        error: e.to_string(),
                        successful: false,
                        response: None,
                    }
                }
            }
            Ok(res) => {
                // Deduplicate: group by file+line and keep the one with smallest span
                use std::collections::BTreeMap;
                let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

                for r in &res {
                    let key = (r.file_uri.clone(), r.line_number);
                    best_by_location
                        .entry(key)
                        .and_modify(|current| {
                            // Only replace if new result has smaller/better span
                            let r_span = r.code_location.end_position.line - r.code_location.start_position.line;
                            let r_start = r.code_location.start_position.character;
                            let r_end = r.code_location.end_position.character;

                            let current_span = current.code_location.end_position.line - current.code_location.start_position.line;
                            let current_start = current.code_location.start_position.character;
                            let current_end = current.code_location.end_position.character;

                            if (r_span, r_start, r_end) < (current_span, current_start, current_end) {
                                *current = r;
                            }
                        })
                        .or_insert(r);
                }

                let new_results: Vec<&ResultNode> = best_by_location.values().copied().collect();
                info!("found {} results for search: {:?}", res.len(), &condition);
                let mut i: Vec<IncidentContext> = new_results.into_iter().map(Into::into).collect();
                i.sort_by_key(|i| format!("{}-{:?}", i.file_uri, i.line_number()));

                // Log detailed results for debugging non-determinism
                if i.len() > 0 {
                    info!("Returning {} incidents for pattern '{:?}':", i.len(), &condition);
                    for (idx, incident) in i.iter().enumerate() {
                        debug!("  Incident[{}]: {} line {}", idx, incident.file_uri, incident.line_number.unwrap_or(0));
                    }
                }
                EvaluateResponse {
                    error: String::new(),
                    successful: true,
                    response: Some(ProviderEvaluateResponse {
                        matched: !i.is_empty(),
                        incident_contexts: i,
                        template_context: None,
                    }),
                }
            }
        };
        if results.response.is_some()
            && !results
                .response
                .as_ref()
                .unwrap()
                .incident_contexts
                .is_empty()
        {
            info!("returning results: {:?}", results);
        }
        return Ok(Response::new(results));
    }

    async fn stop(&self, _: Request<ServiceRequest>) -> Result<Response<()>, Status> {
        return Ok(Response::new(()));
    }

    async fn get_dependencies(
        &self,
        _: Request<ServiceRequest>,
    ) -> Result<Response<DependencyResponse>, Status> {
        return Ok(Response::new(DependencyResponse {
            successful: true,
            error: String::new(),
            file_dep: vec![],
        }));
    }

    async fn get_dependencies_dag(
        &self,
        _: Request<ServiceRequest>,
    ) -> Result<Response<DependencyDagResponse>, Status> {
        return Ok(Response::new(DependencyDagResponse {
            successful: true,
            error: String::new(),
            file_dag_dep: vec![],
        }));
    }

    async fn notify_file_changes(
        &self,
        _: Request<NotifyFileChangesRequest>,
    ) -> Result<Response<NotifyFileChangesResponse>, Status> {
        return Ok(Response::new(NotifyFileChangesResponse {
            error: String::new(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use crate::c_sharp_graph::results::{Location, Position, ResultNode};
    use std::collections::BTreeMap;

    fn create_result_node(
        file_uri: &str,
        line_number: usize,
        start_line: usize,
        start_char: usize,
        end_line: usize,
        end_char: usize,
    ) -> ResultNode {
        ResultNode {
            file_uri: file_uri.to_string(),
            line_number,
            variables: BTreeMap::new(),
            code_location: Location {
                start_position: Position {
                    line: start_line,
                    character: start_char,
                },
                end_position: Position {
                    line: end_line,
                    character: end_char,
                },
            },
        }
    }

    #[test]
    fn test_deduplication_keeps_smallest_span() {
        // Create test data with same file+line but different spans
        let results = vec![
            create_result_node("file1.cs", 10, 10, 0, 15, 0), // span=5 lines
            create_result_node("file1.cs", 10, 10, 5, 12, 0), // span=2 lines (should be kept)
            create_result_node("file1.cs", 10, 10, 0, 20, 0), // span=10 lines
            create_result_node("file2.cs", 20, 20, 0, 21, 0), // different location
        ];

        // Run deduplication logic
        use std::collections::BTreeMap;
        let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

        for r in &results {
            let key = (r.file_uri.clone(), r.line_number);
            best_by_location
                .entry(key)
                .and_modify(|current| {
                    let r_span = r.code_location.end_position.line - r.code_location.start_position.line;
                    let r_start = r.code_location.start_position.character;
                    let r_end = r.code_location.end_position.character;

                    let current_span = current.code_location.end_position.line - current.code_location.start_position.line;
                    let current_start = current.code_location.start_position.character;
                    let current_end = current.code_location.end_position.character;

                    if (r_span, r_start, r_end) < (current_span, current_start, current_end) {
                        *current = r;
                    }
                })
                .or_insert(r);
        }

        let deduplicated: Vec<&ResultNode> = best_by_location.values().copied().collect();

        // Should have 2 results (one for each unique file+line)
        assert_eq!(deduplicated.len(), 2);

        // Find the result for file1.cs:10
        let file1_result = deduplicated
            .iter()
            .find(|r| r.file_uri == "file1.cs" && r.line_number == 10)
            .expect("Should have result for file1.cs:10");

        // Should be the one with smallest span (2 lines)
        let span = file1_result.code_location.end_position.line
            - file1_result.code_location.start_position.line;
        assert_eq!(span, 2, "Should keep result with smallest span");
        assert_eq!(file1_result.code_location.start_position.character, 5);
    }

    #[test]
    fn test_deduplication_is_deterministic() {
        // Create test data - same input multiple times
        let create_test_data = || {
            vec![
                create_result_node("file1.cs", 10, 10, 0, 15, 0),
                create_result_node("file1.cs", 10, 10, 5, 12, 0),
                create_result_node("file1.cs", 10, 10, 0, 20, 0),
                create_result_node("file1.cs", 10, 10, 8, 13, 0), // Same span as second, different char
            ]
        };

        // Run deduplication 3 times and collect character positions
        let mut char_positions = vec![];
        for _ in 0..3 {
            let results = create_test_data();
            use std::collections::BTreeMap;
            let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

            for r in &results {
                let key = (r.file_uri.clone(), r.line_number);
                best_by_location
                    .entry(key)
                    .and_modify(|current| {
                        let r_span = r.code_location.end_position.line - r.code_location.start_position.line;
                        let r_start = r.code_location.start_position.character;
                        let r_end = r.code_location.end_position.character;

                        let current_span = current.code_location.end_position.line - current.code_location.start_position.line;
                        let current_start = current.code_location.start_position.character;
                        let current_end = current.code_location.end_position.character;

                        if (r_span, r_start, r_end) < (current_span, current_start, current_end) {
                            *current = r;
                        }
                    })
                    .or_insert(r);
            }

            let deduplicated: Vec<&ResultNode> = best_by_location.values().copied().collect();
            assert_eq!(deduplicated.len(), 1, "Should deduplicate to 1 result");
            char_positions.push(deduplicated[0].code_location.start_position.character);
        }

        // All runs should produce the same character position
        assert_eq!(char_positions[0], char_positions[1]);
        assert_eq!(char_positions[1], char_positions[2]);
        assert_eq!(char_positions[0], 5, "Should consistently pick character position 5");
    }

    #[test]
    fn test_deduplication_prefers_earlier_character_when_same_span() {
        let results = vec![
            create_result_node("file1.cs", 10, 10, 10, 12, 0), // span=2, char=10
            create_result_node("file1.cs", 10, 10, 5, 12, 0),  // span=2, char=5 (should be kept)
            create_result_node("file1.cs", 10, 10, 15, 12, 0), // span=2, char=15
        ];

        use std::collections::BTreeMap;
        let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

        for r in &results {
            let key = (r.file_uri.clone(), r.line_number);
            best_by_location
                .entry(key)
                .and_modify(|current| {
                    let r_span = r.code_location.end_position.line - r.code_location.start_position.line;
                    let r_start = r.code_location.start_position.character;
                    let r_end = r.code_location.end_position.character;

                    let current_span = current.code_location.end_position.line - current.code_location.start_position.line;
                    let current_start = current.code_location.start_position.character;
                    let current_end = current.code_location.end_position.character;

                    if (r_span, r_start, r_end) < (current_span, current_start, current_end) {
                        *current = r;
                    }
                })
                .or_insert(r);
        }

        let deduplicated: Vec<&ResultNode> = best_by_location.values().copied().collect();

        assert_eq!(deduplicated.len(), 1);
        assert_eq!(
            deduplicated[0].code_location.start_position.character, 5,
            "Should keep result with earliest character when spans are equal"
        );
    }

    #[test]
    fn test_deduplication_is_order_independent() {
        // Create same results in different orders
        let order1 = vec![
            create_result_node("file1.cs", 10, 10, 0, 15, 0),  // Large span
            create_result_node("file1.cs", 10, 10, 5, 12, 0),  // Small span, char=5 (winner)
            create_result_node("file1.cs", 10, 10, 0, 20, 0),  // Huge span
            create_result_node("file2.cs", 20, 20, 0, 21, 0),  // Different location
        ];

        let order2 = vec![
            create_result_node("file2.cs", 20, 20, 0, 21, 0),  // Different location
            create_result_node("file1.cs", 10, 10, 0, 20, 0),  // Huge span
            create_result_node("file1.cs", 10, 10, 5, 12, 0),  // Small span, char=5 (winner)
            create_result_node("file1.cs", 10, 10, 0, 15, 0),  // Large span
        ];

        let order3 = vec![
            create_result_node("file1.cs", 10, 10, 0, 20, 0),  // Huge span
            create_result_node("file2.cs", 20, 20, 0, 21, 0),  // Different location
            create_result_node("file1.cs", 10, 10, 0, 15, 0),  // Large span
            create_result_node("file1.cs", 10, 10, 5, 12, 0),  // Small span, char=5 (winner)
        ];

        // Process all three orderings
        let mut results_from_orders = vec![];
        for results in vec![&order1, &order2, &order3] {
            use std::collections::BTreeMap;
            let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

            for r in results {
                let key = (r.file_uri.clone(), r.line_number);
                best_by_location
                    .entry(key)
                    .and_modify(|current| {
                        let r_span = r.code_location.end_position.line - r.code_location.start_position.line;
                        let r_start = r.code_location.start_position.character;
                        let r_end = r.code_location.end_position.character;

                        let current_span = current.code_location.end_position.line - current.code_location.start_position.line;
                        let current_start = current.code_location.start_position.character;
                        let current_end = current.code_location.end_position.character;

                        if (r_span, r_start, r_end) < (current_span, current_start, current_end) {
                            *current = r;
                        }
                    })
                    .or_insert(r);
            }

            let deduplicated: Vec<&ResultNode> = best_by_location.values().copied().collect();

            // Extract key properties for comparison
            let mut props: Vec<(String, usize, usize, usize)> = deduplicated
                .iter()
                .map(|r| (
                    r.file_uri.clone(),
                    r.line_number,
                    r.code_location.end_position.line - r.code_location.start_position.line,
                    r.code_location.start_position.character,
                ))
                .collect();
            props.sort(); // Sort for consistent comparison
            results_from_orders.push(props);
        }

        // All orderings should produce identical results
        assert_eq!(results_from_orders[0], results_from_orders[1],
            "Order 1 and Order 2 should produce identical results");
        assert_eq!(results_from_orders[1], results_from_orders[2],
            "Order 2 and Order 3 should produce identical results");

        // Verify the actual chosen values
        let file1_result = &results_from_orders[0].iter().find(|r| r.0 == "file1.cs").unwrap();
        assert_eq!(file1_result.2, 2, "Should choose span of 2 lines");
        assert_eq!(file1_result.3, 5, "Should choose character position 5");
    }
}
