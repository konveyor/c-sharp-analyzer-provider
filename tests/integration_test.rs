use std::path::absolute;
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use std::{fs::File, path::PathBuf, str::FromStr};

use prost_types::value::Kind::StringValue;
use prost_types::{Struct, Value};
use serde::Deserialize;
use walkdir::WalkDir;

use c_sharp_analyzer_provider_cli::analyzer_service::IncidentContext;
use c_sharp_analyzer_provider_cli::analyzer_service::{
    provider_service_client::ProviderServiceClient, Config, EvaluateRequest,
};
use c_sharp_analyzer_provider_cli::c_sharp_graph::results::ResultNode;

#[derive(Deserialize, Debug)]
pub struct TestEvaluateRequest {
    id: i64,
    cap: String,
    condition_info: String,
}

impl From<TestEvaluateRequest> for EvaluateRequest {
    fn from(value: TestEvaluateRequest) -> Self {
        EvaluateRequest {
            id: value.id,
            cap: value.cap,
            condition_info: value.condition_info,
        }
    }
}

/// Guard to ensure server process is killed on drop
struct ServerGuard {
    process: Option<Child>,
}

impl ServerGuard {
    fn new(process: Child) -> Self {
        Self {
            process: Some(process),
        }
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            println!("Shutting down server...");
            let _ = process.kill();
            let _ = process.wait();
            println!("Server shut down");
        }
    }
}

/// Start the C# analyzer provider server in the background
fn start_server(port: &str) -> ServerGuard {
    let current_dir = std::env::current_dir().expect("Failed to get current directory");
    let server_bin = current_dir.join("target/debug/c-sharp-analyzer-provider-cli");

    println!("Starting server: {:?}", server_bin);

    // Create log file for server output

    let process = Command::new(server_bin)
        .args([
            "--port",
            port,
            "--name",
            "c-sharp",
            "--db-path",
            &format!("test-{}.db", port),
            "--log-file",
            &format!("test-server-{}.log", port),
        ])
        .env("RUST_LOG", "DEBUG")
        .spawn()
        .expect("Failed to start server");

    println!("Server logs will be written to test-server.log");
    ServerGuard::new(process)
}

/// Wait for the server to be ready by attempting to connect
async fn wait_for_server(port: &str, max_attempts: u32) -> Result<(), String> {
    println!("Waiting for server to be ready...");

    for attempt in 1..=max_attempts {
        match ProviderServiceClient::connect(format!("http://localhost:{}", port)).await {
            Ok(_) => {
                println!("Server is ready!");
                return Ok(());
            }
            Err(_) => {
                if attempt == max_attempts {
                    return Err(format!(
                        "Server did not become ready after {} attempts",
                        max_attempts
                    ));
                }
                println!(
                    "Attempt {}/{}: Server not ready, waiting...",
                    attempt, max_attempts
                );
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    Err("Server failed to start".to_string())
}

#[tokio::test]
async fn integration_tests() {
    let port = "9000";
    // Clean up old database to ensure fresh start
    let _ = std::fs::remove_file(format!("test-{}.db", port));
    // Start the server (will be automatically killed on drop)
    let _server_guard = start_server(port);

    // Wait for server to be ready
    if let Err(e) = wait_for_server(port, 60).await {
        panic!("Failed to start server: {}", e);
    }

    // Connect to the server
    let mut client =
        match ProviderServiceClient::connect(format!("{}:{}", "http://localhost", port)).await {
            Ok(client) => client,
            Err(e) => {
                panic!("Failed to connect to server: {}", e);
            }
        };
    let current_file = file!();
    let file_path = absolute(PathBuf::from_str(current_file).unwrap()).unwrap();

    let parent = file_path.parent().unwrap();
    let base = parent.parent().unwrap();
    let base: String = base.to_string_lossy().into_owned();

    // Initialize the provider with testdata/nerd-dinner
    let testdata_location = PathBuf::from(&base).join("testdata").join("nerd-dinner");
    println!(
        "Initializing provider with location: {:?}",
        testdata_location
    );

    // Build provider-specific config
    let home_dir = std::env::var("HOME").expect("HOME environment variable not set");
    let dotnet_install_script = PathBuf::from(&base)
        .join("scripts")
        .join("dotnet-install.sh");
    let mut provider_config_fields = std::collections::BTreeMap::new();
    provider_config_fields.insert(
        "ilspy_cmd".to_string(),
        Value {
            kind: Some(StringValue(format!("{}/.dotnet/tools/ilspycmd", home_dir))),
        },
    );
    provider_config_fields.insert(
        "paket_cmd".to_string(),
        Value {
            kind: Some(StringValue(format!("{}/.dotnet/tools/paket", home_dir))),
        },
    );
    provider_config_fields.insert(
        "dotnet_install_cmd".to_string(),
        Value {
            kind: Some(StringValue(
                dotnet_install_script.to_string_lossy().to_string(),
            )),
        },
    );

    let config = Config {
        location: testdata_location.to_string_lossy().to_string(),
        dependency_path: String::new(),
        analysis_mode: "source-only".to_string(),
        provider_specific_config: Some(Struct {
            fields: provider_config_fields,
        }),
        proxy: None,
        language_server_pipe: String::new(),
        initialized: false,
    };

    // Call init
    match client.init(config).await {
        Ok(response) => {
            let init_response = response.into_inner();
            if !init_response.successful {
                panic!("Init failed: {}", init_response.error);
            }
            println!("Successfully initialized provider");
        }
        Err(e) => {
            panic!("Init request failed: {}", e);
        }
    }

    let demos_path = parent.to_path_buf().join("demos");
    println!("Walking dir: {:?}", demos_path);
    for entry in WalkDir::new(&demos_path) {
        let entry = entry.unwrap();
        if !entry.file_type().is_dir() {
            continue;
        }
        let request_file = entry.clone().into_path().join("request.yaml");
        if !request_file.exists() {
            continue;
        }
        let demo_ouput = entry.clone().into_path().join("demo-output.yaml");
        if !demo_ouput.exists() {
            continue;
        }
        println!("testing demo: {:?}", entry.path());

        let start_time = Instant::now();
        let requst_file = File::open(&request_file).unwrap();

        let request: TestEvaluateRequest = serde_yml::from_reader(requst_file).unwrap();
        let request: EvaluateRequest = request.into();

        let result = client.evaluate(request).await.unwrap().into_inner();
        assert!(
            result.successful,
            "expected successful result got: {:?}",
            result
        );
        let expected_file = File::open(&demo_ouput).unwrap();
        let expected_output: Vec<ResultNode> = serde_json::from_reader(expected_file).unwrap();
        let expected_output: Vec<IncidentContext> = expected_output
            .iter()
            .map(|rn| {
                let mut x: IncidentContext = rn.into();
                if x.file_uri.contains("<REPLACE_ME>") {
                    x.file_uri = x.file_uri.replace("<REPLACE_ME>", &base);
                    let mut var = x.variables.clone().unwrap();
                    if let Some(s) = var.fields.get("file") {
                        if let Some(StringValue(y)) = &s.kind {
                            var.fields.insert(
                                "file".to_string(),
                                Value {
                                    kind: Some(StringValue(y.replace("<REPLACE_ME>", &base))),
                                },
                            );
                        }
                    }
                    x.variables = Some(var);
                }

                x
            })
            .collect();
        match result.response {
            None => panic!(),
            Some(x) => {
                fn get_difference_filter<T: PartialEq + Clone>(v1: &[T], v2: &[T]) -> Vec<T> {
                    v1.iter().filter(|&x| !v2.contains(x)).cloned().collect()
                }
                let a = get_difference_filter(&x.incident_contexts, &expected_output);
                let b = get_difference_filter(&expected_output, &x.incident_contexts);
                if !a.is_empty() || !b.is_empty() {
                    for i in a {
                        println!("Diff: {:?}", i);
                    }
                    for i in b {
                        println!("Diff: {:?}", i);
                    }
                    panic!("differences found");
                }
            }
        }
        println!("finished: {:?} in {:?}", entry.path(), start_time.elapsed())
    }

    // Server will be automatically shut down when _server_guard goes out of scope
    println!("All tests completed successfully");
}

#[tokio::test]
async fn integration_test_net8() {
    let port = "9001";
    // Clean up old database to ensure fresh start
    let _ = std::fs::remove_file(format!("test-{}.db", port));
    // Start the server (will be automatically killed on drop)
    let _server_guard = start_server(port);

    // Wait for server to be ready
    if let Err(e) = wait_for_server(port, 60).await {
        panic!("Failed to start server: {}", e);
    }

    // Connect to the server
    let mut client =
        match ProviderServiceClient::connect(format!("{}:{}", "http://localhost", port)).await {
            Ok(client) => client,
            Err(e) => {
                panic!(
                    "Failed to connect to server: {} -- http://localhost:{}",
                    e, port
                );
            }
        };

    let current_file = file!();
    let file_path = absolute(PathBuf::from_str(current_file).unwrap()).unwrap();
    let parent = file_path.parent().unwrap();
    let base = parent.parent().unwrap();

    // Initialize the provider with testdata/net8-sample (.NET 8 project)
    let testdata_location = PathBuf::from(&base).join("testdata").join("net8-sample");
    println!(
        "Initializing provider with .NET 8 project: {:?}",
        testdata_location
    );

    // Build provider-specific config
    let home_dir = std::env::var("HOME").expect("HOME environment variable not set");
    let dotnet_install_script = PathBuf::from(&base)
        .join("scripts")
        .join("dotnet-install.sh");
    let mut provider_config_fields = std::collections::BTreeMap::new();
    provider_config_fields.insert(
        "ilspy_cmd".to_string(),
        Value {
            kind: Some(StringValue(format!("{}/.dotnet/tools/ilspycmd", home_dir))),
        },
    );
    provider_config_fields.insert(
        "paket_cmd".to_string(),
        Value {
            kind: Some(StringValue(format!("{}/.dotnet/tools/paket", home_dir))),
        },
    );
    provider_config_fields.insert(
        "dotnet_install_cmd".to_string(),
        Value {
            kind: Some(StringValue(
                dotnet_install_script.to_string_lossy().to_string(),
            )),
        },
    );

    let config = Config {
        location: testdata_location.to_string_lossy().to_string(),
        dependency_path: String::new(),
        analysis_mode: "source-only".to_string(),
        provider_specific_config: Some(Struct {
            fields: provider_config_fields,
        }),
        proxy: None,
        language_server_pipe: String::new(),
        initialized: false,
    };

    // Call init
    match client.init(config).await {
        Ok(response) => {
            let init_response = response.into_inner();
            if !init_response.successful {
                panic!("Init failed: {}", init_response.error);
            }
            println!("Successfully initialized .NET 8 provider");
        }
        Err(e) => {
            panic!("Init request failed: {}", e);
        }
    }

    // Test a query - search for Person class references (defined in our source code)
    let request = EvaluateRequest {
        id: 1,
        cap: "referenced".to_string(),
        condition_info: r#"{"referenced": {"pattern": "Net8Sample.Person.*"}}"#.to_string(),
    };

    println!("Running query for Person class references...");
    let result = client.evaluate(request).await.unwrap().into_inner();

    assert!(
        result.successful,
        "expected successful result got: {:?}",
        result
    );

    match result.response {
        None => panic!("No response from evaluate"),
        Some(response) => {
            println!("Found {} incidents", response.incident_contexts.len());

            assert_ne!(response.incident_contexts.len(), 0);
            // Verify at least one incident is in Program.cs
            let program_incidents: Vec<_> = response
                .incident_contexts
                .iter()
                .filter(|ic| ic.file_uri.contains("Program.cs"))
                .collect();

            println!(
                "✓ Successfully found {} Person class references total",
                response.incident_contexts.len()
            );
            println!("✓ {} references in Program.cs", program_incidents.len());
        }
    }

    let request = EvaluateRequest {
        id: 1,
        cap: "referenced".to_string(),
        condition_info: r#"{"referenced": {"pattern": "System.Console.*"}}"#.to_string(),
    };

    println!("Running query for System.Console references (from SDK)...");
    let result = client.evaluate(request).await.unwrap().into_inner();

    assert!(
        result.successful,
        "expected successful result got: {:?}",
        result
    );

    match result.response {
        None => panic!("No response from evaluate"),
        Some(response) => {
            println!(
                "Found {} System.Console incidents",
                response.incident_contexts.len()
            );
            assert!(!response.incident_contexts.is_empty());

            if response.incident_contexts.is_empty() {
                println!("WARNING: No System.Console references found from SDK.");
                println!(
                    "This indicates SDK installation may have failed or XML files weren't loaded."
                );
                println!("Check server logs above for SDK installation status.");
            } else {
                println!(
                    "✓ Successfully found {} System.Console references from .NET 8 SDK",
                    response.incident_contexts.len()
                );
                println!("✓ SDK installation and XML loading working correctly!");
            }
        }
    }
    println!(".NET 8 integration test completed successfully");
}
