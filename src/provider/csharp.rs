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
                let new_results = deduplicate_results(&res);
                info!("found {} results for search: {:?}", res.len(), &condition);
                let mut i: Vec<IncidentContext> = new_results.into_iter().map(Into::into).collect();
                i.sort_by_key(|i| format!("{}-{:?}", i.file_uri, i.line_number()));

                // Log detailed results for debugging non-determinism
                if i.len() > 0 {
                    info!(
                        "Returning {} incidents for pattern '{:?}':",
                        i.len(),
                        &condition
                    );
                    for (idx, incident) in i.iter().enumerate() {
                        debug!(
                            "  Incident[{}]: {} line {}",
                            idx,
                            incident.file_uri,
                            incident.line_number.unwrap_or(0)
                        );
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

/// Deduplicate results by grouping by (file_uri, line_number) and keeping the result
/// with the smallest span. When spans are equal, prefer earlier start character and
/// earlier end character for deterministic selection.
fn deduplicate_results<'a>(results: &'a [ResultNode]) -> Vec<&'a ResultNode> {
    use std::collections::BTreeMap;
    let mut best_by_location: BTreeMap<(String, usize), &ResultNode> = BTreeMap::new();

    for r in results {
        let key = (r.file_uri.clone(), r.line_number);
        best_by_location
            .entry(key)
            .and_modify(|current| {
                // Only replace if new result has smaller/better span
                let r_span =
                    r.code_location.end_position.line - r.code_location.start_position.line;
                let r_start = r.code_location.start_position.character;
                let r_end = r.code_location.end_position.character;
                let r_line = r.line_number;

                let current_span = current.code_location.end_position.line
                    - current.code_location.start_position.line;
                let current_start = current.code_location.start_position.character;
                let current_end = current.code_location.end_position.character;
                let current_line = current.line_number;

                if (r_line, r_span, r_start, r_end)
                    < (current_line, current_span, current_start, current_end)
                {
                    *current = r;
                }
            })
            .or_insert(r);
    }

    best_by_location.values().copied().collect()
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
        let deduplicated = super::deduplicate_results(&results);

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
            let deduplicated = super::deduplicate_results(&results);
            assert_eq!(deduplicated.len(), 1, "Should deduplicate to 1 result");
            char_positions.push(deduplicated[0].code_location.start_position.character);
        }

        // All runs should produce the same character position
        assert_eq!(char_positions[0], char_positions[1]);
        assert_eq!(char_positions[1], char_positions[2]);
        assert_eq!(
            char_positions[0], 5,
            "Should consistently pick character position 5"
        );
    }

    #[test]
    fn test_deduplication_prefers_earlier_character_when_same_span() {
        let results = vec![
            create_result_node("file1.cs", 10, 10, 10, 12, 0), // span=2, char=10
            create_result_node("file1.cs", 10, 10, 5, 12, 0),  // span=2, char=5 (should be kept)
            create_result_node("file1.cs", 10, 10, 15, 12, 0), // span=2, char=15
        ];

        let deduplicated = super::deduplicate_results(&results);

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
            create_result_node("file1.cs", 10, 10, 0, 15, 0), // Large span
            create_result_node("file1.cs", 10, 10, 5, 12, 0), // Small span, char=5 (winner)
            create_result_node("file1.cs", 10, 10, 0, 20, 0), // Huge span
            create_result_node("file2.cs", 20, 20, 0, 21, 0), // Different location
        ];

        let order2 = vec![
            create_result_node("file2.cs", 20, 20, 0, 21, 0), // Different location
            create_result_node("file1.cs", 10, 10, 0, 20, 0), // Huge span
            create_result_node("file1.cs", 10, 10, 5, 12, 0), // Small span, char=5 (winner)
            create_result_node("file1.cs", 10, 10, 0, 15, 0), // Large span
        ];

        let order3 = vec![
            create_result_node("file1.cs", 10, 10, 0, 20, 0), // Huge span
            create_result_node("file2.cs", 20, 20, 0, 21, 0), // Different location
            create_result_node("file1.cs", 10, 10, 0, 15, 0), // Large span
            create_result_node("file1.cs", 10, 10, 5, 12, 0), // Small span, char=5 (winner)
        ];

        // Process all three orderings
        let mut results_from_orders = vec![];
        for results in vec![&order1, &order2, &order3] {
            let deduplicated = super::deduplicate_results(results);

            // Extract key properties for comparison
            let mut props: Vec<(String, usize, usize, usize)> = deduplicated
                .iter()
                .map(|r| {
                    (
                        r.file_uri.clone(),
                        r.line_number,
                        r.code_location.end_position.line - r.code_location.start_position.line,
                        r.code_location.start_position.character,
                    )
                })
                .collect();
            props.sort(); // Sort for consistent comparison
            results_from_orders.push(props);
        }

        // All orderings should produce identical results
        assert_eq!(
            results_from_orders[0], results_from_orders[1],
            "Order 1 and Order 2 should produce identical results"
        );
        assert_eq!(
            results_from_orders[1], results_from_orders[2],
            "Order 2 and Order 3 should produce identical results"
        );

        // Verify the actual chosen values
        let file1_result = &results_from_orders[0]
            .iter()
            .find(|r| r.0 == "file1.cs")
            .unwrap();
        assert_eq!(file1_result.2, 2, "Should choose span of 2 lines");
        assert_eq!(file1_result.3, 5, "Should choose character position 5");
    }

    #[test]
    fn test_deduplication_adjacent_lines_tree_sitter_scenario() {
        // Simulates the System.Web.Mvc scenario where tree-sitter might create multiple nodes
        // for the SAME line_number with different spans due to parsing ambiguities.
        //
        // Real scenario from CI: The same line (e.g., 240) might get reported multiple times
        // with different span information because tree-sitter creates nodes with ambiguous
        // boundaries. The deduplication should keep the tightest/smallest span.
        //
        // Note: Results on DIFFERENT line_numbers (240 vs 241) are NOT deduplicated - they're
        // kept as separate results because the grouping key is (file_uri, line_number).
        let results = vec![
            // Scenario 1: Multiple nodes reported for line 179 with different spans
            create_result_node("AccountController.cs", 179, 179, 16, 179, 26), // Tight span on line 179
            create_result_node("AccountController.cs", 179, 179, 16, 181, 17), // Span crossing to line 181
            create_result_node("AccountController.cs", 179, 177, 0, 179, 26),  // Span starting earlier
            // Scenario 2: Multiple nodes reported for line 240 with different spans
            create_result_node("AccountController.cs", 240, 240, 0, 240, 94), // Tight span on line 240 (WINNER)
            create_result_node("AccountController.cs", 240, 240, 0, 241, 20), // Span crossing to line 241
            create_result_node("AccountController.cs", 240, 239, 0, 240, 94), // Span starting earlier
            // Scenario 3: Line 241 has its own references (separate from 240)
            create_result_node("AccountController.cs", 241, 241, 16, 241, 23), // ViewBag on line 241
            create_result_node("AccountController.cs", 241, 241, 0, 242, 10),  // Wider span for line 241
            // Scenario 4: Different file, should not be affected
            create_result_node("DinnerController.cs", 100, 100, 0, 100, 10),
        ];

        let mut deduplicated = super::deduplicate_results(&results);
        deduplicated.sort_by_key(|r| (&r.file_uri, r.line_number));

        // Should have 4 results after deduplication:
        // - AccountController.cs:179 (best of 3 results for line 179)
        // - AccountController.cs:240 (best of 3 results for line 240)
        // - AccountController.cs:241 (best of 2 results for line 241)
        // - DinnerController.cs:100
        assert_eq!(
            deduplicated.len(),
            4,
            "Should deduplicate same-line entries but keep distinct line numbers"
        );

        // Check line 179: should keep the one with smallest span
        let line_179 = deduplicated
            .iter()
            .find(|r| r.file_uri == "AccountController.cs" && r.line_number == 179)
            .expect("Should have result for line 179");
        let span_179 =
            line_179.code_location.end_position.line - line_179.code_location.start_position.line;
        assert_eq!(
            span_179, 0,
            "Line 179 should keep single-line span (smallest)"
        );
        assert_eq!(line_179.code_location.start_position.line, 179);
        assert_eq!(line_179.code_location.start_position.character, 16);
        assert_eq!(line_179.code_location.end_position.character, 26);

        // Check line 240: should keep the one with smallest span
        let line_240 = deduplicated
            .iter()
            .find(|r| r.file_uri == "AccountController.cs" && r.line_number == 240)
            .expect("Should have result for line 240");
        let span_240 =
            line_240.code_location.end_position.line - line_240.code_location.start_position.line;
        assert_eq!(span_240, 0, "Line 240 should keep single-line span");
        assert_eq!(line_240.code_location.start_position.line, 240);
        assert_eq!(line_240.code_location.end_position.character, 94);

        // Check line 241: should keep the one with smallest span (separate from 240)
        let line_241 = deduplicated
            .iter()
            .find(|r| r.file_uri == "AccountController.cs" && r.line_number == 241)
            .expect("Should have result for line 241");
        let span_241 =
            line_241.code_location.end_position.line - line_241.code_location.start_position.line;
        assert_eq!(span_241, 0, "Line 241 should keep single-line span");
        assert_eq!(line_241.code_location.start_position.line, 241);
        assert_eq!(line_241.code_location.start_position.character, 16);

        // Verify all AccountController.cs results
        let account_controller_results: Vec<&&ResultNode> = deduplicated
            .iter()
            .filter(|r| r.file_uri == "AccountController.cs")
            .collect();
        assert_eq!(
            account_controller_results.len(),
            3,
            "AccountController.cs should have exactly 3 results (lines 179, 240, 241)"
        );
    }

    #[test]
    fn test_deduplication_does_not_merge_different_lines() {
        // Ensure that the deduplication logic does NOT merge results on different line numbers
        // even if they're adjacent. Each line number should be treated as a separate key.
        let results = vec![
            create_result_node("file.cs", 179, 179, 0, 179, 10), // Line 179
            create_result_node("file.cs", 180, 180, 0, 180, 10), // Line 180
            create_result_node("file.cs", 181, 181, 0, 181, 10), // Line 181
        ];

        let deduplicated = super::deduplicate_results(&results);

        // Should keep all 3 results since they're on different lines
        assert_eq!(
            deduplicated.len(),
            3,
            "Adjacent line numbers should not be merged - each line is a separate key"
        );

        // Verify we have results for all three lines
        assert!(deduplicated.iter().any(|r| r.line_number == 179));
        assert!(deduplicated.iter().any(|r| r.line_number == 180));
        assert!(deduplicated.iter().any(|r| r.line_number == 181));
    }
}
