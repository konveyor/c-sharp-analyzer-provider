# Development Guide

## Getting Started

### Prerequisites

**Required:**
- Rust 1.70+ (2021 edition)
- Protocol Buffers compiler (protoc)
- .NET SDK 9.x or higher
- Git

**Optional (for testing):**
- grpcurl (for manual testing)
- ilspycmd (for dependency analysis)
- paket (for dependency resolution)

### Initial Setup

1. **Clone the repository**
   ```bash
   git clone <repository-url>
   cd c-sharp-analyzer-provider-cli
   ```

2. **Install Rust dependencies**
   ```bash
   cargo build
   ```
   This will also compile the protobuf definitions via `build.rs`.

3. **Install .NET tools (for full testing)**
   ```bash
   dotnet tool install --global ilspycmd
   dotnet tool install --global paket
   ```

4. **Install grpcurl (optional, for manual testing)**
   ```bash
   # macOS
   brew install grpcurl

   # Linux
   go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest

   # Or download from https://github.com/fullstorydev/grpcurl/releases
   ```

5. **Verify installation**
   ```bash
   cargo build
   cargo test -- --nocapture
   ```

## Development Workflow

### Building

```bash
# Debug build (faster, for development)
cargo build

# Release build (optimized, for production)
cargo build --release

# Build and run
cargo run -- --port 9000 --name c-sharp --db-path test.db
```

### Running the Server

```bash
# Run with default settings
cargo run -- --port 9000 --name c-sharp

# Run with custom database path
cargo run -- --port 9000 --name c-sharp --db-path my-test.db

# Run with debug logging
RUST_LOG=debug cargo run -- --port 9000 --name c-sharp

# Run on Unix socket (Unix-like systems)
cargo run -- --socket /tmp/analyzer.sock --name c-sharp

# Run on named pipe (Windows)
cargo run -- --socket \\.\pipe\analyzer --name c-sharp
```

### Linting

```bash
# Run Clippy (catches common mistakes)
cargo clippy

# Auto-fix some issues
cargo clippy --fix

# Run format check
cargo fmt --check

# Auto-format code
cargo fmt
```

### Testing

See [testing.md](testing.md) for comprehensive testing documentation.

Quick reference:
```bash
# Run all tests
cargo test -- --nocapture

# Run with demo server
make run-demo

# Run specific test
cargo test integration_tests -- --nocapture
```

## Project Structure

```
c-sharp-analyzer-provider-cli/
├── src/
│   ├── main.rs                      # Entry point, server setup
│   ├── lib.rs                       # Library exports
│   ├── analyzer_service/            # gRPC service definitions
│   │   ├── mod.rs                   # Generated proto code
│   │   └── provider.rs              # Generated from .proto
│   ├── provider/                    # Provider implementation
│   │   ├── mod.rs                   # Module exports
│   │   ├── csharp.rs               # CSharpProvider service impl
│   │   ├── project.rs              # Project state management
│   │   └── dependency_resolution.rs # Dependency handling
│   ├── c_sharp_graph/              # Stack graph query engine
│   │   ├── mod.rs                   # Module exports
│   │   ├── loader.rs               # Graph building from source
│   │   ├── query.rs                # Query trait and core logic
│   │   ├── results.rs              # Result formatting
│   │   ├── language_config.rs      # Tree-sitter config
│   │   ├── namespace_query.rs      # Namespace queries
│   │   ├── class_query.rs          # Class queries
│   │   ├── method_query.rs         # Method queries
│   │   └── field_query.rs          # Field queries
│   └── pipe_stream/                # Windows named pipe support
│       ├── mod.rs
│       └── server.rs
├── tests/
│   ├── integration_test.rs         # Integration test runner
│   └── demos/                       # Test cases
├── testdata/                        # Test projects
├── docs/                            # Documentation
├── build.rs                         # Build script (protoc)
├── Cargo.toml                       # Dependencies
└── Makefile                         # Common tasks
```

## Adding New Features

### Adding a New Query Capability

Example: Add support for querying property references.

#### 1. Define the Location Type

Edit `src/provider/csharp.rs`:

```rust
#[derive(Clone, ToSchema, Deserialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
enum Locations {
    #[default]
    All,
    Method,
    Field,
    Class,
    Property,  // <-- Add this
}
```

#### 2. Create Query Implementation

Create `src/c_sharp_graph/property_query.rs`:

```rust
use super::query::{Query, SyntaxType};
use super::results::ResultNode;
use stack_graphs::graph::StackGraph;
use anyhow::{Error, Result};

pub struct PropertySymbolsGetter<'a> {
    pub graph: &'a StackGraph,
    pub source_type_filter: Option<String>,
}

impl<'a> Query for PropertySymbolsGetter<'a> {
    fn query(self, pattern: String) -> Result<Vec<ResultNode>, Error> {
        // Find all nodes with syntax type "property_name"
        let nodes = self.graph.iter_nodes()
            .filter(|node| {
                // Check if node is a property
                matches!(
                    get_syntax_type(node, self.graph),
                    Some(SyntaxType::PropertyName)
                )
            })
            .collect();

        // Match against pattern, resolve FQDN, etc.
        // See method_query.rs for full example

        Ok(result_nodes)
    }
}
```

#### 3. Add Syntax Type

Edit `src/c_sharp_graph/query.rs`:

```rust
#[derive(Debug)]
pub enum SyntaxType {
    Import,
    CompUnit,
    NamespaceDeclaration,
    ClassDef,
    MethodName,
    FieldName,
    PropertyName,  // <-- Add this
    LocalVar,
    Argument,
    Name,
}

impl SyntaxType {
    pub(crate) fn get(syntax_type_string: &str) -> Self {
        match syntax_type_string {
            "import" => Self::Import,
            "comp_unit" => Self::CompUnit,
            "namespace_declaration" => Self::NamespaceDeclaration,
            "class_def" => Self::ClassDef,
            "method_name" => Self::MethodName,
            "field_name" => Self::FieldName,
            "property_name" => Self::PropertyName,  // <-- Add this
            "local_var" => Self::LocalVar,
            "argument" => Self::Argument,
            "name" => Self::Name,
            &_ => Self::Name,
        }
    }
}
```

#### 4. Export the Module

Edit `src/c_sharp_graph/mod.rs`:

```rust
mod class_query;
mod field_query;
pub mod language_config;
pub mod loader;
mod method_query;
mod namespace_query;
mod property_query;  // <-- Add this
pub mod query;
pub mod results;
```

#### 5. Wire Up in Provider

Edit `src/provider/csharp.rs` in the `evaluate()` method:

```rust
use crate::c_sharp_graph::property_query::PropertySymbolsGetter;

// In evaluate() method:
let result_nodes = match reference_condition.location {
    Locations::All => {
        let query = NamespaceSymbolsGetter { ... };
        query.query(reference_condition.pattern)?
    }
    Locations::Method => {
        let query = MethodSymbolsGetter { ... };
        query.query(reference_condition.pattern)?
    }
    Locations::Field => {
        let query = FieldSymbolsGetter { ... };
        query.query(reference_condition.pattern)?
    }
    Locations::Class => {
        let query = ClassSymbolsGetter { ... };
        query.query(reference_condition.pattern)?
    }
    Locations::Property => {  // <-- Add this
        let query = PropertySymbolsGetter {
            graph: &graph,
            source_type_filter: Some(source_type_symbol.clone()),
        };
        query.query(reference_condition.pattern)?
    }
};
```

#### 6. Update Language Configuration

If the tree-sitter C# grammar doesn't tag property nodes correctly, you may need to update `src/c_sharp_graph/language_config.rs` to add rules for property detection.

#### 7. Add Tests

Create test case in `tests/demos/property_search/`:

```yaml
# request.yaml
id: 1
cap: referenced
condition_info: '{"referenced": {"pattern": "MyApp\\..*", "location": "property"}}'
```

Run test to generate expected output, then verify.

### Adding a New Capability (Beyond "referenced")

Example: Add a "defined" capability to find where symbols are defined (not just referenced).

#### 1. Update Capabilities Response

Edit `src/provider/csharp.rs`:

```rust
async fn capabilities(&self, _: Request<()>) -> Result<Response<CapabilitiesResponse>, Status> {
    Ok(Response::new(CapabilitiesResponse {
        capabilities: vec![
            Capability {
                name: "referenced".to_string(),
                template_context: None,
            },
            Capability {  // <-- Add this
                name: "defined".to_string(),
                template_context: None,
            },
        ],
    }))
}
```

#### 2. Define Condition Schema

```rust
#[derive(ToSchema, Deserialize, Debug)]
struct DefinedCondition {
    pattern: String,
    #[serde(default)]
    location: Locations,
}

#[derive(ToSchema, Deserialize, Debug)]
struct CSharpCondition {
    #[serde(skip_serializing_if = "Option::is_none")]
    referenced: Option<ReferenceCondition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    defined: Option<DefinedCondition>,  // <-- Add this
}
```

#### 3. Handle in Evaluate

```rust
async fn evaluate(&self, r: Request<EvaluateRequest>) -> Result<Response<EvaluateResponse>, Status> {
    let req = r.get_ref();

    match req.cap.as_str() {
        "referenced" => {
            // Existing logic
        }
        "defined" => {  // <-- Add this
            let condition: CSharpCondition = serde_json::from_str(&req.condition_info)
                .map_err(|e| Status::invalid_argument(format!("Invalid condition: {}", e)))?;

            let defined_condition = condition.defined
                .ok_or_else(|| Status::invalid_argument("Missing 'defined' condition"))?;

            // Query logic for definitions
        }
        _ => return Err(Status::invalid_argument("Unknown capability")),
    }
}
```

#### 4. Implement Query Logic

For definitions, you might want to:
- Filter to only definition nodes (not references)
- Look for specific syntax types (class_def, method_def, etc.)
- Return where symbols are declared

## Debugging

### Logging

Use the `tracing` crate for structured logging:

```rust
use tracing::{debug, info, warn, error, trace};

debug!("Processing query: {}", pattern);
info!("Stack graph built with {} nodes", graph.iter_nodes().count());
warn!("Large result set: {} matches", results.len());
error!("Failed to parse: {}", err);
trace!("Node details: {:?}", node);
```

Set log level via environment:
```bash
# All debug logs
RUST_LOG=debug cargo run -- --port 9000

# Specific module
RUST_LOG=c_sharp_analyzer_provider_cli::c_sharp_graph=trace cargo run -- --port 9000

# Multiple levels
RUST_LOG=c_sharp_analyzer_provider_cli=debug,tower=info cargo run -- --port 9000
```

### Debugging with VS Code

Create `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "lldb",
      "request": "launch",
      "name": "Debug Server",
      "cargo": {
        "args": [
          "build",
          "--bin=c-sharp-analyzer-provider-cli",
          "--package=c-sharp-analyzer-provider-cli"
        ],
        "filter": {
          "name": "c-sharp-analyzer-provider-cli",
          "kind": "bin"
        }
      },
      "args": ["--port", "9000", "--name", "c-sharp", "--db-path", "debug.db"],
      "cwd": "${workspaceFolder}",
      "env": {
        "RUST_LOG": "debug"
      }
    }
  ]
}
```

Set breakpoints and press F5 to debug.

### Inspecting the Stack Graph

Add debug code to dump the graph:

```rust
use stack_graphs::serde::StackGraph as SerializableStackGraph;

let serializable = SerializableStackGraph::from(&graph);
let json = serde_json::to_string_pretty(&serializable)?;
std::fs::write("graph.json", json)?;
```

Then inspect `graph.json` to see the structure.

### Using a Debugger REPL

```bash
# Run with rust-lldb
rust-lldb target/debug/c-sharp-analyzer-provider-cli -- --port 9000

# Set breakpoint
(lldb) breakpoint set --file csharp.rs --line 150

# Run
(lldb) run

# When hit:
(lldb) frame variable
(lldb) continue
```

## Updating Dependencies

### Cargo Dependencies

```bash
# Check for outdated dependencies
cargo outdated

# Update within semver constraints
cargo update

# Update to latest (may break)
cargo upgrade
```

### Protocol Buffer Definition

The proto file comes from upstream analyzer-lsp:

```bash
# Download latest
make download_proto

# Rebuild
cargo build
```

This updates `src/build/proto/provider.proto` and regenerates the Rust code.

### Tree-sitter Grammar

The C# grammar comes from `tree-sitter-c-sharp` crate. To update:

```bash
# Edit Cargo.toml
tree-sitter-c-sharp = "0.24"  # Update version

# Rebuild
cargo build

# Test
cargo test
```

You may need to update language configuration if the grammar changes.

## Performance Optimization

### Profiling

```bash
# Install profiler
cargo install flamegraph

# Profile the server
cargo flamegraph -- --port 9000 --name c-sharp

# Run workload in another terminal
make run-grpc-init-http
make run-grpc-ref-http

# Ctrl+C server
# Open flamegraph.svg
```

### Benchmarking

Create `benches/query_benchmark.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn query_benchmark(c: &mut Criterion) {
    // Setup graph
    let graph = setup_test_graph();

    c.bench_function("method query", |b| {
        b.iter(|| {
            let query = MethodSymbolsGetter { ... };
            query.query(black_box("System\\..*".to_string()))
        });
    });
}

criterion_group!(benches, query_benchmark);
criterion_main!(benches);
```

Run:
```bash
cargo bench
```

## Common Pitfalls

### 1. Stack Graph Not Rebuilding

The stack graph is cached in SQLite. To force rebuild:

```bash
rm test.db  # or whatever --db-path you used
cargo run -- --port 9000 --name c-sharp --db-path test.db
```

### 2. Regex Escaping in JSON

When patterns are in JSON, you need double escaping:

```json
{
  "pattern": "System\\\\.Web.*"
}
```

One backslash for JSON, one for regex.

### 3. Source Type Tagging

Always tag nodes with source type in loader:

```rust
graph.add_node_tag(node, source_type.get_symbol_handle());
```

Otherwise filtering won't work.

### 4. Async Mutex Deadlocks

Use `tokio::sync::Mutex` for async code, `std::sync::Mutex` for sync:

```rust
// Async context
let guard = self.project.lock().await;

// Sync context
let guard = self.graph.lock().unwrap();
```

Don't mix them or you'll get deadlocks.

### 5. Proto File Changes

After updating the proto file, rebuild:

```bash
cargo clean
cargo build
```

The build script runs during compilation.

## Contributing

### Code Style

- Follow Rust standard style (`cargo fmt`)
- Use `clippy` and fix warnings
- Write doc comments for public APIs
- Keep functions under 100 lines when possible
- Use meaningful variable names

### Pull Request Process

1. Create a feature branch
2. Make changes with clear commit messages
3. Run tests: `make run-demo`
4. Run clippy: `cargo clippy`
5. Format code: `cargo fmt`
6. Create PR with description of changes
7. Ensure CI passes

### Commit Messages

Use conventional commits:

```
feat: Add property query support
fix: Correct FQDN resolution for nested classes
docs: Update testing guide
test: Add integration test for field queries
refactor: Extract common query logic
```

## Useful Resources

- [Tree-sitter](https://tree-sitter.github.io/tree-sitter/)
- [Stack Graphs](https://github.com/github/stack-graphs)
- [Tonic (gRPC)](https://docs.rs/tonic/)
- [Tokio (async runtime)](https://tokio.rs/)
- [C# Grammar](https://github.com/tree-sitter/tree-sitter-c-sharp)
