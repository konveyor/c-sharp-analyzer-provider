# Testing Guide

## Overview

The project has multiple layers of testing:

1. **Integration Tests**: Validate the entire stack (gRPC service → query engine → stack graph → results). Tests automatically manage server lifecycle.
2. **Analyzer Integration Tests (Local)**: Run the provider with the konveyor-analyzer CLI locally
3. **Analyzer Integration Tests (Container)**: Run the provider in containers as it would in CI/production

## Test Architecture

### Integration Tests
```
tests/
├── integration_test.rs         # Main test runner (auto-manages server)
└── demos/                       # Test cases
    ├── class_search/
    │   ├── request.yaml         # Query definition
    │   └── demo-output.yaml     # Expected results
    ├── field_search/
    ├── method_search/
    └── ...
```

### Analyzer Integration Tests
```
e2e-tests/
├── konveyor-analyzer           # Analyzer binary (downloaded)
├── provider_settings.json      # Provider configuration
├── demo-output.yaml            # Expected e2e output
└── analysis-output.yaml        # Actual e2e output (generated)
```

Each integration test case directory contains:
- `request.yaml`: The `EvaluateRequest` to send
- `demo-output.yaml`: The expected `ResultNode[]` output

## Requirements

### For Integration Tests
- Rust/Cargo (for building and running tests)
- .NET SDK 9.x or compatible version
- ilspycmd (.NET tool): `dotnet tool install -g ilspycmd`
- paket (.NET tool): `dotnet tool install -g paket`

### For Analyzer Integration Tests (Local)
- All integration test requirements above
- `jq` (for updating provider_settings.json)
- `yq` (optional, for sorted output verification)
- `gh` CLI (optional, for downloading konveyor-analyzer)

### For Container-Based Tests
- Container runtime: `podman` (default) or `docker`
- All build dependencies

### For Manual Testing
- `grpcurl` (for manual gRPC requests)

## Running Tests

### Integration Tests (Automated)

```bash
# Run all integration tests with automatic server lifecycle management
make run-tests
```

**What it does:**
1. Resets test data (cleans up any previous state)
2. Builds the project with `cargo build`
3. Runs integration tests (tests start/stop server automatically)
4. Cleans up test data

**Or run just the tests:**
```bash
# Tests manage server lifecycle internally
cargo test -- --nocapture
```

### Analyzer Integration Tests (Local)

Run the complete end-to-end test locally with konveyor-analyzer:

```bash
# Run complete local analyzer integration test
make run-analyzer-integration-local
```

**What it does:**
1. Downloads/locates konveyor-analyzer binary
2. Updates provider_settings.json with current paths
3. Runs analyzer with rulesets
4. Verifies output matches expected results

**Individual steps:**
```bash
# Download or locate konveyor-analyzer binary
make get-konveyor-analyzer-local

# Update provider_settings.json with current paths (requires jq)
make update-provider-settings-local

# Run analyzer with rulesets
make run-test-local

# Verify output matches expected (exact diff)
make verify-output

# Verify output with sorted comparison (requires yq)
make verify-e2e-results
```

**Override konveyor-analyzer branch:**
```bash
make get-konveyor-analyzer-local KONVEYOR_BRANCH=development
```

### Analyzer Integration Tests (Container)

Run the provider in containers as it would in CI:

```bash
# Run complete container-based integration test
make run-analyzer-integration
```

**What it does:**
1. Builds container image
2. Creates pod and runs provider container
3. Runs analyzer in container against provider
4. Stops and cleans up pod

**Individual steps:**
```bash
# Build container image
make build-image

# Create pod and run provider container
make run-c-sharp-pod

# Run analyzer in container against provider
make run-demo-c-sharp-pod

# Stop and clean up pod
make stop-c-sharp-pod
```

**Override container runtime and tags:**
```bash
make run-analyzer-integration CONTAINER_RUNTIME=docker TAG=v0.1.0
make run-analyzer-integration IMG_ANALYZER=quay.io/konveyor/analyzer-lsp:latest
```

## Manual Testing (Debugging/Legacy)

> **Note**: Manual server management is now legacy. Integration tests automatically manage the server lifecycle. Use this approach only for debugging or manual exploration.

### Manual Testing with grpcurl

For manual testing, you can use the provided Makefile targets:

```bash
# Terminal 1: Start server with debug logging
RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,INFO cargo run -- --port 9000 --name c-sharp --db-path testing.db
```

```bash
# Terminal 2: Initialize the project
make run-grpc-init-http

# Run a query for references
make run-grpc-ref-http
```

These targets use grpcurl with pre-configured requests. To customize:

**Initialize with custom config:**
```bash
grpcurl -max-time 1000 -plaintext -d '{
    "analysisMode": "source-only",
    "location": "'$(pwd)'/testdata/nerd-dinner",
    "providerSpecificConfig": {
      "ilspy_cmd": "'${HOME}'/.dotnet/tools/ilspycmd",
      "paket_cmd": "'${HOME}'/.dotnet/tools/paket",
      "dotnet_install_cmd": "'$(pwd)'/scripts/dotnet-install.sh"
    }
  }' localhost:9000 provider.ProviderService.Init
```

**Run custom query:**
```bash
grpcurl -max-msg-sz 10485760 -max-time 30 -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System.Web.Mvc.*\"}}"
}' localhost:9000 provider.ProviderService.Evaluate > output.yaml
```

## Test Data

### Test Projects

The project includes multiple test applications:

1. **nerd-dinner** (`testdata/nerd-dinner/`): .NET MVC 4 application used for primary testing
2. **net8-sample** (`testdata/net8-sample/`): .NET 8 sample application

**Resetting test data:**
```bash
make reset-nerd-dinner-demo  # Reset just nerd-dinner
make reset-net8-sample       # Reset just net8-sample
make reset-demo-apps         # Reset all test apps and output
make reset-demo-output       # Restore demo-output.yaml from backup
```

**What gets reset:**
- Removes `paket-files/` and `.paket/` (Paket artifacts)
- Removes `packages/` (NuGet packages)
- Removes `obj/` (build artifacts)
- Runs `git clean -f` (removes untracked files)
- Runs `git stash push` (stashes changes)
- Cleans up database files (`*.db`, `*.log`)

### Adding New Test Projects

1. Add project to `testdata/`
2. Create reset target in Makefile following the pattern of existing targets
3. Update `reset-demo-apps` target to include the new reset target
4. Create test cases in `tests/demos/` or `e2e-tests/` that use the new project

## Debugging Tests

### Enable Verbose Logging

```bash
# Run with full debug output
RUST_LOG=trace cargo test -- --nocapture
```

### Log Levels by Component

```bash
# Only debug logs from the CLI itself
RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG cargo test

# Debug for multiple components
RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,tree_sitter_stack_graphs=DEBUG cargo test
```

### Inspect Server Logs

When running tests, server logs are output to stdout/stderr:

```bash
# Run tests with all logging visible
RUST_LOG=trace make run-tests

# Or capture to a file
make run-tests 2>&1 | tee test-run.log
```

### Debug a Single Test

```bash
# Run specific test by filtering test name
cargo test integration_tests -- --nocapture
```

### Common Issues

#### Server Not Starting

```bash
# Check if port is already in use
lsof -i :9000

# Kill existing process
kill $(lsof -t -i :9000)
```

#### Init Request Failing

Check that required tools are installed:

```bash
# Verify ilspycmd
which ilspycmd
ilspycmd --version

# Verify paket
which paket
paket --version
```

#### Pattern Not Matching

Patterns are Rust regex. Test your pattern:

```bash
# Example: match pattern interactively
cargo run -- --port 9000 --name c-sharp --db-path test.db

# In another terminal, try different patterns
grpcurl ... -d '{"cap": "referenced", "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Web.*\"}}"}' ...
```

Common regex pitfalls:
- `.` matches any character; use `\\.` for literal dot
- `*` is a repetition operator; use `.*` to match "any characters"
- Use `\\` for backslash in JSON strings

#### Unexpected Results

The test framework automatically compares actual vs expected output and shows differences. For e2e tests:

```bash
# Run analyzer integration test and verify
make run-analyzer-integration-local

# For sorted comparison (when order differs)
make verify-e2e-results
```

For integration tests, the test output will show the difference between expected and actual results.

### Using grpcurl for Debugging

List available services:
```bash
grpcurl -plaintext localhost:9000 list
```

Describe a service:
```bash
grpcurl -plaintext localhost:9000 describe provider.ProviderService
```

Get capabilities:
```bash
grpcurl -plaintext localhost:9000 provider.ProviderService.Capabilities
```

## Performance Testing

### Measure Query Time

```bash
# Time a query
time grpcurl -plaintext -d '{"cap": "referenced", "conditionInfo": "..."}' localhost:9000 provider.ProviderService.Evaluate > /dev/null
```

### Measure Init Time

```bash
# Time initialization
time grpcurl -max-time 1000 -plaintext -d '{"analysisMode": "full", ...}' localhost:9000 provider.ProviderService.Init
```

## CI/CD Integration

### GitHub Actions Workflow

The project uses GitHub Actions for continuous integration. Check `.github/workflows/` for active workflows.

**Typical CI steps:**
1. Checkout code
2. Install Protoc (for building gRPC)
3. Run Clippy (linting)
4. Install .NET SDK and required tools
5. Install ilspycmd and paket tools
6. Run integration tests
7. Run analyzer integration tests (container-based)

**When tests run:**
- On pull requests
- On pushes to main
- On pushes to release branches

### Adding Checks to CI

To add new validation to CI:

1. Edit the appropriate workflow file in `.github/workflows/`
2. Add a new step:

```yaml
- name: "Run additional check"
  run: |
    make your-test-target
```

### Container Testing in CI

The `make run-analyzer-integration` target is designed for CI environments and tests the provider in containers as it would run in production.

## Test Coverage

### Integration Tests Cover:

- ✅ Server startup and initialization
- ✅ Dependency resolution (paket)
- ✅ Stack graph building
- ✅ Pattern matching queries
- ✅ Location-based filtering (method, field, class)
- ✅ Source vs dependency filtering
- ✅ Result formatting
- ✅ Automatic server lifecycle management

### Analyzer Integration Tests Cover:

- ✅ End-to-end provider integration with konveyor-analyzer
- ✅ Ruleset execution and violation detection
- ✅ Provider settings configuration
- ✅ Container-based deployment testing
- ✅ Output verification (exact and sorted)

### Not Covered:

- ❌ Unit tests for individual components
- ❌ Error handling edge cases
- ❌ Concurrent request handling
- ❌ Large codebases (performance)
- ❌ Malformed requests
- ❌ File watching / incremental updates

## Future Testing Improvements

1. **Unit Tests**: Add tests for individual modules
2. **Benchmark Suite**: Standardized performance benchmarks
3. **Fuzz Testing**: Random input generation for robustness
4. **Multiple Projects**: Test against various project structures
5. **Error Scenarios**: Explicit tests for error conditions
6. **Mock Tests**: Test without external dependencies
