# C# Analyzer Provider CLI

A gRPC-based code analysis service for C# codebases using tree-sitter and stack-graphs. Part of the [Konveyor analyzer-lsp](https://github.com/konveyor/analyzer-lsp) ecosystem.

## Overview

This tool provides semantic code analysis for C# projects, enabling queries to find:
- Type references (classes, interfaces, structs)
- Method calls and definitions
- Field usages and declarations
- Namespace imports and usages

It builds a stack graph from C# source code and optionally decompiled dependencies, then provides a gRPC service for querying that graph.

## Quick Start

### Prerequisites

- Rust 1.70+ with cargo
- Protocol Buffers compiler (protoc)
- .NET SDK 9.x or higher
- Optional: ilspycmd and paket for dependency analysis

### Installation

```bash
# Clone the repository
git clone <repository-url>
cd c-sharp-analyzer-provider

# Build
cargo build

# Install .NET tools (optional, for full dependency analysis)
dotnet tool install --global ilspycmd
dotnet tool install --global paket
```

### Running

```bash
# Start the server
cargo run -- --port 9000 --name c-sharp

# In another terminal, initialize a project
grpcurl -plaintext -d '{
  "analysisMode": "source-only",
  "location": "/path/to/csharp/project"
}' localhost:9000 provider.ProviderService.Init

# Query for references
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Web\\\\.Mvc.*\"}}"
}' localhost:9000 provider.ProviderService.Evaluate
```

## Features

- **Semantic Analysis**: Uses tree-sitter for parsing and stack-graphs for semantic understanding
- **Dependency Analysis**: Optionally resolves and analyzes .NET dependencies
- **Pattern Matching**: Regex-based queries for flexible symbol search
- **Location Filtering**: Query by location type (method, field, class, or all)
- **gRPC Service**: Standard gRPC interface for integration
- **Multiple Transports**: HTTP/2, Unix domain sockets, or Windows named pipes
- **Persistent Caching**: SQLite-based stack graph storage for fast startup

## Documentation

### For Users

- [Quick Start Guide](#quick-start) - Get up and running quickly
- [CLAUDE.md](CLAUDE.md) - Guidance for AI assistants working with this codebase

### For Developers

- **[Architecture Overview](docs/architecture.md)** - System design, components, and data flow
- **[Development Guide](docs/development.md)** - Setup, workflows, and adding features
- **[Testing Guide](docs/testing.md)** - Running tests, debugging, and writing new tests

## Analysis Modes

### Source-Only Mode
Analyzes only your project's source code. Fast and lightweight.

```bash
cargo run -- --port 9000 --name c-sharp
# Then init with: "analysisMode": "source-only"
```

### Full Mode
Analyzes source code plus all resolved dependencies. Requires ilspycmd and paket.

```bash
# Install tools first
dotnet tool install --global ilspycmd paket

# Run server
cargo run -- --port 9000 --name c-sharp

# Init with: "analysisMode": "full"
```

## Query Examples

### Find All References to a Namespace

```bash
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Collections.*\"}}"
}' localhost:9000 provider.ProviderService.Evaluate
```

### Find Method References Only

```bash
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"MyApp\\\\.Services\\\\..*\", \"location\": \"method\"}}"
}' localhost:9000 provider.ProviderService.Evaluate
```

### Find Class Definitions

```bash
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \".*Controller\", \"location\": \"class\"}}"
}' localhost:9000 provider.ProviderService.Evaluate
```

## Development

### Building and Testing

```bash
# Build
cargo build

# Run linter
cargo clippy

# Run tests
make run-demo

# Run specific test
cargo test -- --nocapture
```

### Project Structure

```
src/
├── main.rs                  # Server entry point
├── analyzer_service/        # gRPC service definitions
├── provider/                # Provider implementation
├── c_sharp_graph/          # Stack graph query engine
└── pipe_stream/            # Named pipe support (Windows)

tests/
├── integration_test.rs     # Integration tests
└── demos/                  # Test cases

docs/                       # Developer documentation
```

See [Development Guide](docs/development.md) for detailed information.

## Contributing

Contributions are welcome! Please:

1. Read the [Development Guide](docs/development.md)
2. Check existing issues or create a new one
3. Fork the repository and create a feature branch
4. Make your changes with tests
5. Run `cargo clippy` and `cargo fmt`
6. Submit a pull request

## Testing

The project uses integration tests that run against a live server instance:

```bash
# Full test suite with server management
make run-demo

# Manual testing
cargo run -- --port 9000 --name c-sharp  # Terminal 1
cargo test -- --nocapture                 # Terminal 2
```

See [Testing Guide](docs/testing.md) for comprehensive testing documentation.

## Architecture

The system consists of several layers:

1. **gRPC Service Layer**: Handles client requests and responses
2. **Provider Layer**: Manages project state and coordinates analysis
3. **Stack Graph Engine**: Builds and queries semantic graphs
4. **Dependency Resolution**: Handles .NET dependencies via Paket and ILSpy

See [Architecture Overview](docs/architecture.md) for detailed design documentation.

## Requirements

### Runtime Dependencies

- Rust standard library
- SQLite (for graph caching)

### Optional Dependencies (Full Mode)

- **ilspycmd**: Decompiles .NET assemblies to C# source
  ```bash
  dotnet tool install --global ilspycmd
  ```

- **paket**: Resolves .NET dependencies
  ```bash
  dotnet tool install --global paket
  ```

## Configuration

### Command-line Options

```
Options:
  --port <PORT>           TCP port for gRPC over HTTP/2
  --socket <SOCKET>       Unix socket or named pipe path
  --name <NAME>           Service name
  --db-path <DB_PATH>     SQLite database path (default: temp dir)
  --log-file <LOG_FILE>   Log file path
  -v, --verbosity         Log verbosity level
```

### Environment Variables

- `RUST_LOG`: Set log level (debug, info, warn, error)
  ```bash
  RUST_LOG=debug cargo run -- --port 9000
  ```

## Performance

- **Caching**: Stack graphs are persisted to SQLite for fast restarts
- **Streaming**: Results are streamed to avoid buffering large result sets
- **Concurrency**: Multi-threaded async runtime handles concurrent requests
- **Incremental**: Reuses cached graphs when project hasn't changed

## Limitations

- No authentication or authorization (intended for local/trusted use)
- C# only (no other .NET languages yet)
- Regex patterns only (no AST-based queries)
- Limited incremental update support

## License

[Add your license here]

## Related Projects

- [analyzer-lsp](https://github.com/konveyor/analyzer-lsp) - Language Server Protocol implementation
- [tree-sitter](https://tree-sitter.github.io/) - Parser generator and incremental parsing library
- [stack-graphs](https://github.com/github/stack-graphs) - Code navigation using stack graphs
- [tree-sitter-c-sharp](https://github.com/tree-sitter/tree-sitter-c-sharp) - C# grammar for tree-sitter

## Support

- **Issues**: Report bugs or request features via GitHub issues
- **Documentation**: See [docs/](docs/) directory
- **CI/CD**: GitHub Actions workflow in `.github/workflows/`

## Acknowledgments

This project uses:
- [Tonic](https://github.com/hyperium/tonic) for gRPC
- [Tokio](https://tokio.rs/) for async runtime
- [Tree-sitter](https://tree-sitter.github.io/) for parsing
- [Stack Graphs](https://github.com/github/stack-graphs) for semantic analysis
