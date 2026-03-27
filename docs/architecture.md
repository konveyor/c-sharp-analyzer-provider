# Architecture Overview

## System Architecture

The C# Analyzer Provider is a gRPC service that analyzes C# codebases to find references to types, methods, fields, and classes. It uses tree-sitter for parsing and stack-graphs for semantic analysis.

```
┌─────────────────────────────────────────────────────────────┐
│                     gRPC Client                              │
│              (analyzer-lsp or grpcurl)                       │
└──────────────────────┬──────────────────────────────────────┘
                       │ gRPC/HTTP2 or Unix Socket
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                   Provider Service                           │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  capabilities() - Returns supported query types      │   │
│  │  init()        - Initializes project & builds graph  │   │
│  │  evaluate()    - Executes queries against graph      │   │
│  └──────────────────────────────────────────────────────┘   │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                    Project Manager                           │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  - Manages project state                             │   │
│  │  - Coordinates dependency resolution                 │   │
│  │  - Orchestrates stack graph building                 │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────┬──────────────────────────┬─────────────────────────┘
          │                          │
          ▼                          ▼
┌──────────────────────┐   ┌──────────────────────────────────┐
│ Dependency Resolver  │   │   Stack Graph Builder            │
│  - Uses Paket        │   │    - Tree-sitter parsing         │
│  - Downloads deps    │   │    - Stack graph construction    │
│  - ILSpy decompiler  │   │    - SQLite persistence          │
└──────────────────────┘   └──────────────────────────────────┘
                                     │
                                     ▼
                           ┌──────────────────────┐
                           │   Query Engine       │
                           │  - Pattern matching  │
                           │  - FQDN resolution   │
                           │  - Location filtering│
                           └──────────────────────┘
```

## Core Components

### 1. Main Entry Point (`src/main.rs`)

The main binary sets up the server infrastructure:

- **Multi-threaded Runtime**: Configures Tokio with dynamic worker threads (`available_parallelism()`, capped at 32)
- **Transport Layer**: Supports multiple transport modes:
  - HTTP/2 with gRPC (via `--port` flag)
  - Unix Domain Sockets on Unix-like systems (via `--socket` flag)
  - Named Pipes on Windows (via `--socket` flag)
- **Logging**: Environment-based log filtering with tracing
- **Reflection**: Includes gRPC reflection for service discovery
- **Telemetry**: Optional OpenTelemetry OTLP tracing layer (enabled via `OTEL_EXPORTER_OTLP_ENDPOINT`)
- **Metrics**: Optional Prometheus metrics HTTP server (enabled via `METRICS_PORT`)
- **Shutdown**: Graceful OTel tracer flush on exit

### 2. Provider Service (`src/analyzer_service/provider.rs`, `src/provider/csharp.rs`)

Implements the `ProviderService` gRPC interface:

#### Capabilities Endpoint
```rust
async fn capabilities(&self, _: Request<()>) -> Result<Response<CapabilitiesResponse>, Status>
```
- Returns supported capabilities (currently "referenced")
- Can be extended to support additional query types

#### Init Endpoint
```rust
async fn init(&self, r: Request<Config>) -> Result<Response<InitResponse>, Status>
```
- Receives configuration including:
  - Project location
  - Analysis mode (source-only or full)
  - Provider-specific config (ilspy_cmd, paket_cmd paths)
- Creates a `Project` instance
- Triggers dependency resolution (if full mode)
- Builds the stack graph
- Returns success/failure status

#### Evaluate Endpoint
```rust
async fn evaluate(&self, r: Request<EvaluateRequest>) -> Result<Response<EvaluateResponse>, Status>
```
- Receives query request with:
  - Capability name ("referenced")
  - Condition JSON (pattern, location, optional file_paths)
- Parses condition to extract query parameters
- Routes to appropriate query implementation based on location type
- Returns stream of incidents (matches) with file/line/column info

### 3. Project Management (`src/provider/project.rs`)

Manages the analyzed project's state:

```rust
pub struct Project {
    pub location: PathBuf,                    // Project root directory
    pub db_path: PathBuf,                     // SQLite database path
    pub dependencies: Arc<TokioMutex<...>>,   // Resolved dependencies
    pub graph: Arc<Mutex<Option<StackGraph>>>, // The stack graph
    pub source_language_config: Arc<...>,     // Tree-sitter config
    pub analysis_mode: AnalysisMode,          // Source-only or full
    pub tools: Tools,                         // External tool paths
}
```

**Analysis Modes:**
- `SourceOnly`: Only analyze the user's source code
- `Full`: Analyze source code + all resolved dependencies

**Initialization Flow:**
1. Validate external tools (ilspycmd, paket) exist
2. If full mode: resolve dependencies using Paket
3. If full mode: decompile dependencies using ILSpy
4. Walk directory tree to find all .cs files
5. Build stack graph from source and dependency files
6. Persist graph to SQLite database

### 4. Dependency Resolution (`src/provider/dependency_resolution.rs`)

Handles .NET dependency management:

**Process:**
1. Use Paket to resolve NuGet dependencies from .sln/.csproj files
2. Download dependency packages
3. Extract .dll files from packages
4. Use ILSpy to decompile .dll to C# source
5. Tag decompiled code as "dependency" type in stack graph

This allows querying both user code and framework/library code.

### 5. Stack Graph System (`src/c_sharp_graph/`)

The semantic analysis engine built on tree-sitter and stack-graphs.

#### Loader (`loader.rs`)

Builds the stack graph from C# source files:

```rust
pub fn init_stack_graph(
    graph: &mut StackGraph,
    db_path: &PathBuf,
    location: PathBuf,
    source_type: &SourceType,
) -> Result<SourceNodeLanguageConfiguration>
```

**Process:**
1. Configure tree-sitter with C# grammar
2. Walk directory tree for .cs files
3. Parse each file with tree-sitter
4. Run stack-graphs rules to build semantic graph
5. Tag nodes with source type (source vs dependency)
6. Write graph to SQLite database
7. Build forward partial paths for efficient querying

**Key Concept - Source Type Tagging:**
Every node in the graph is tagged with metadata:
- `konveyor.io/source_type=source`: User's code
- `konveyor.io/source_type=dependency`: External libraries

This enables filtering results by origin.

#### Query System (`query.rs`)

Provides the query interface:

```rust
pub trait Query {
    fn query(self, query: String) -> Result<Vec<ResultNode>, Error>;
}
```

**Implementations:**
- `NamespaceSymbolsGetter`: Query namespace declarations/usages
- `ClassSymbolsGetter`: Query class definitions
- `MethodSymbolsGetter`: Query method references
- `FieldSymbolsGetter`: Query field references

**FQDN Resolution:**

Each query implementation resolves fully-qualified domain names:

```
namespace: MyApp.Services
class: UserService
method: GetUser
field: _repository
```

The query engine:
1. Finds nodes matching the syntax type (e.g., method_name)
2. Traverses "FQDN" edges upward to build the full name
3. Matches against the query pattern (regex)
4. Returns matching nodes with location info

#### Results (`results.rs`)

Formats query results:

```rust
pub struct ResultNode {
    pub file_uri: String,           // file:// URI
    pub location: Location,         // Line/column range
    pub source_type: String,        // "source" or "dependency"
}
```

Converts stack graph nodes with source info into structured results that can be serialized to the gRPC response.

### 6. Language Configuration (`src/c_sharp_graph/language_config.rs`)

Defines the tree-sitter-stack-graphs configuration for C#:

- Maps C# syntax nodes to semantic symbols
- Defines scoping rules
- Specifies how to build the FQDN hierarchy
- Configures which nodes should be indexed

This is essentially the "rules" for how to interpret C# code semantically.

### 7. Telemetry (`src/provider/telemetry.rs`)

Opt-in observability infrastructure:

- **OTLP Traces**: Batch span exporter to any OTLP-compatible backend (Jaeger, Tempo, etc.)
- **Prometheus Metrics**: Lightweight HTTP server serving `/metrics` in text format
- **W3C TraceContext**: Extracts `traceparent` from gRPC metadata for cross-service tracing
- **Span Propagation**: Captures and re-enters tracing spans in `spawn_blocking` closures

### 8. SDK Detection (`src/provider/sdk_detection.rs`)

Locates existing .NET SDK installations before falling back to download:
- Checks configured `dotnet_sdk_path`
- Probes system dotnet installations
- Returns `SdkSource::Found` or `SdkSource::NotFound`

### 9. Target Framework (`src/provider/target_framework.rs`)

Manages .NET target framework detection and SDK installation:
- Parses TFMs from `.csproj` files
- Installs SDK versions via official `dotnet-install.sh`
- Discovers SDK XML reference files for dependency analysis

### 10. Code Snippet Service (`src/provider/code_snip.rs`)

Implements `ProviderCodeLocationService` gRPC interface for returning source code
snippets at given file locations with configurable context lines.

## Data Flow

### Initialization Flow

```
Client Init Request
    ↓
Validate Tools (ilspycmd, paket)
    ↓
Detect Target Framework (.csproj parsing)
    ↓
[If Modern .NET] Find/Install SDK (spawn_blocking)
    ↓
[If Full Mode] Resolve Dependencies
    ↓           ↓
    ↓       Run Paket
    ↓           ↓
    ↓       Download NuGet packages
    ↓           ↓
    ↓       Extract DLLs
    ↓           ↓
    ↓       Decompile with ILSpy
    ↓           ↓
    └───────────┘
         ↓
Walk Source Directory (.cs files)
    ↓
Parse with Tree-sitter
    ↓
Build Stack Graph
    ↓
Tag Nodes (source vs dependency)
    ↓
Persist to SQLite
    ↓
Build Forward Paths
    ↓
Return InitResponse (success/error)
```

### Query Flow

```
Client Evaluate Request
    ↓
Parse Condition JSON
    ↓
Extract: pattern, location, file_paths
    ↓
Select Query Implementation
    ↓           ↓             ↓           ↓
Namespace  Class Query  Method Query  Field Query
    ↓           ↓             ↓           ↓
    └───────────┴─────────────┴───────────┘
                ↓
        Find Matching Nodes by Syntax Type
                ↓
        Resolve FQDN for Each Node
                ↓
        Match Pattern (Regex)
                ↓
        Filter by Source Type
                ↓
        Filter by File Paths (if specified)
                ↓
        Format as ResultNode
                ↓
        Convert to IncidentContext
                ↓
        Stream to Client
```

## Threading Model

The system uses two thread pools managed by the Tokio runtime:

### Async Worker Pool
- **Size**: Dynamic via `std::thread::available_parallelism()`, capped at 32
- **Purpose**: Handles gRPC requests, async I/O, channel operations, task coordination
- **Contract**: Code between `.await` points must complete quickly (< ~10ms)

### Blocking Thread Pool
- **Size**: Auto-scaling (default max: 512), managed by `tokio::task::spawn_blocking`
- **Purpose**: Runs heavy blocking work that would starve the async pool
- **Operations offloaded**:
  - Graph queries (regex matching + graph traversal) in `evaluate()`
  - Stack graph initialization (WalkDir + tree-sitter parsing + SQLite) in `get_project_graph()`
  - Per-dependency graph builds in `load_to_database_source_only()` / `load_to_database_full_analysis()`
  - SDK XML file processing in `load_sdk_xml_files_to_database()`
  - File I/O in `get_code_snip()`
  - SDK installation subprocesses in `init()`

### Span Propagation Across Thread Pools

`spawn_blocking` closures run on the blocking pool and don't inherit the caller's tracing span.
The codebase uses explicit span capture to maintain trace continuity:

```rust
let span = tracing::Span::current();
tokio::task::spawn_blocking(move || {
    let _guard = span.enter();
    // blocking work -- spans created here are children of the captured span
})
```

When blocking work needs data from a `tokio::sync::RwLock`, `read_owned()` produces
a `'static + Send` guard that can be moved into the closure:

```rust
let guard = Arc::clone(&rw_lock).read_owned().await;
tokio::task::spawn_blocking(move || {
    let data = guard.as_ref().unwrap();
    // use data inside the blocking task
})
```

### Thread Safety
- `Arc<Mutex<...>>` for stack graph (std::sync -- cannot be held across `.await`)
- `Arc<TokioMutex<...>>` for async-accessible state (dependency list)
- `Arc<RwLock<...>>` for read-heavy structures (language config, project)
- Mutex poison recovery via `unwrap_or_else(|e| e.into_inner())` to avoid cascading panics

## Storage

### SQLite Database

The stack graph is persisted to SQLite for performance:

```
Default: /tmp/c_sharp_provider.db
Custom: --db-path flag
```

**Schema:**
- Stores serialized stack graph nodes
- Stores serialized partial paths
- Enables fast startup for subsequent runs

**Rebuild Triggers:**
- Project location changes
- Source files modified
- Dependencies changed

## Extension Points

### Adding a New Capability

1. Define the condition schema in `src/provider/csharp.rs`
2. Add capability to `capabilities()` response
3. Add match arm in `evaluate()` to handle the new capability
4. Implement query logic in `src/c_sharp_graph/`
5. Update tests in `tests/demos/`

### Adding a New Location Type

1. Add enum variant to `Locations` in `src/provider/csharp.rs`
2. Create new query file in `src/c_sharp_graph/` (e.g., `property_query.rs`)
3. Implement `Query` trait with appropriate syntax type filtering
4. Add match arm in `evaluate()` to route to new query
5. Update language configuration if needed

### Supporting Additional Languages

The architecture is designed around tree-sitter, so adding new languages requires:

1. Create new language module (e.g., `src/java_graph/`)
2. Configure tree-sitter for that language
3. Implement language-specific query types
4. Create provider implementation
5. Update main.rs to support multiple providers

## Performance Considerations

- **SQLite caching**: Avoids re-parsing on every startup
- **Streaming responses**: Large result sets don't require buffering
- **Regex compilation**: Patterns are compiled once per query
- **Forward paths**: Pre-computed for faster graph traversal
- **Worker threads**: Concurrent request handling

## Security Considerations

- No authentication/authorization (intended for local/trusted use)
- File system access limited to configured project location
- External tools (ilspycmd, paket) must be explicitly configured
- No arbitrary code execution (only parsing and decompilation)
