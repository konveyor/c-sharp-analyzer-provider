# Testing Guide

## Overview

The project uses integration tests that validate the entire stack: gRPC service → query engine → stack graph → results. Tests run against a live server instance.

## Test Architecture

```
tests/
├── integration_test.rs         # Main test runner
└── demos/                       # Test cases
    ├── class_search/
    │   ├── request.yaml         # Query definition
    │   └── demo-output.yaml     # Expected results
    ├── field_search/
    ├── method_search/
    └── ...
```

Each test case is a directory containing:
- `request.yaml`: The `EvaluateRequest` to send
- `demo-output.yaml`: The expected `ResultNode[]` output

## Running Tests

### Quick Test Run

```bash
# Run all integration tests (requires server running on localhost:9000)
cargo test -- --nocapture
```

### Full Demo Test (Automated)

```bash
# This runs everything: build, start server, init, test, cleanup
make run-demo
```

**What it does:**
1. Resets test data (cleans up any previous state)
2. Builds the project with `cargo build`
3. Starts server in background on port 9000
4. Waits for server to be ready
5. Sends init request to configure the project
6. Runs all integration tests
7. Kills the server
8. Cleans up test data

### CI/CD Test (GitHub Actions)

```bash
# Same as run-demo but with logging to demo.log
make run-demo-github
```

This is used in `.github/workflows/demo-testing.yml`.

## Manual Testing

### 1. Start the Server

```bash
# Terminal 1: Start server with debug logging
RUST_LOG=c_sharp_analyzer_provider_cli=DEBUG,INFO cargo run -- --port 9000 --name c-sharp --db-path testing.db
```

### 2. Initialize the Project

```bash
# Terminal 2: Send init request
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

### 3. Run a Query

```bash
# Query for references to System.Web.Mvc.*
grpcurl -max-msg-sz 10485760 -max-time 30 -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System.Web.Mvc.*\"}}"
}' localhost:9000 provider.ProviderService.Evaluate > output.yaml
```

### 4. Check Results

```bash
# View the results
cat output.yaml
```

## Writing New Tests

### 1. Create Test Directory

```bash
mkdir -p tests/demos/my_new_test
```

### 2. Create Request Definition

Create `tests/demos/my_new_test/request.yaml`:

```yaml
id: 1
cap: referenced
condition_info: '{"referenced": {"pattern": "MyNamespace\\.MyClass", "location": "class"}}'
```

**Fields:**
- `id`: Unique identifier for this request
- `cap`: Capability to use (currently only "referenced")
- `condition_info`: JSON string with query parameters
  - `pattern`: Regex pattern to match against FQDNs
  - `location`: Filter by location type (optional)
    - `all` (default): Match anywhere
    - `method`: Only in method contexts
    - `field`: Only field declarations/usages
    - `class`: Only class definitions
  - `file_paths`: Filter by file paths (optional)

### 3. Run Test to Generate Output

```bash
# Start server and run your test
make run-demo

# Find your test's output
cat demo-output.yaml
```

### 4. Create Expected Output

Copy the actual output to your test directory:

```bash
# Create expected output from actual results
cp demo-output.yaml tests/demos/my_new_test/demo-output.yaml
```

Edit `demo-output.yaml` to verify it contains the expected matches:

```yaml
- file_uri: file:///path/to/file.cs
  location:
    start:
      line: 10
      column: 4
    end:
      line: 10
      column: 20
  source_type: source
```

### 5. Verify Test Passes

```bash
# Run all tests including your new one
make run-demo
```

The test runner will:
1. Find your test directory
2. Parse `request.yaml`
3. Send the request to the server
4. Compare actual output with `demo-output.yaml`
5. Report pass/fail

## Test Data

### Primary Test Project: nerd-dinner

Located in `testdata/nerd-dinner/`, this is a .NET MVC application used for testing.

**Resetting test data:**
```bash
make reset-nerd-dinner-demo  # Reset just nerd-dinner
make reset-demo-apps         # Reset all test apps
```

**What gets reset:**
- Removes `paket-files/` (downloaded dependencies)
- Removes `packages/` (NuGet packages)
- Runs `git clean -f` (removes untracked files)
- Runs `git stash push` (stashes changes)

### Adding New Test Projects

1. Add project to `testdata/`
2. Create reset target in Makefile if needed
3. Update test cases to use the new project
4. Update `run-demo-github` if needed for CI

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

When using `make run-demo-github`, logs go to `demo.log`:

```bash
# Tail logs during test run
tail -f demo.log

# Search for errors
grep ERROR demo.log
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

Compare actual vs expected:

```bash
# Run test to generate actual output
make run-demo

# Diff actual vs expected
diff demo-output.yaml tests/demos/my_test/demo-output.yaml
```

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

### Profile with Flamegraph

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Run server under profiling
cargo flamegraph -- --port 9000 --name c-sharp

# In another terminal, run your workload
make run-grpc-init-http
make run-grpc-ref-http

# Ctrl+C the server, then open flamegraph.svg
```

## CI/CD Integration

### GitHub Actions Workflow

Located in `.github/workflows/demo-testing.yml`:

**Steps:**
1. Checkout code
2. Install Protoc (for building gRPC)
3. Run Clippy (linting)
4. Install grpcurl (for testing)
5. Install .NET SDK 9.x
6. Install ilspycmd and paket tools
7. Run `make run-demo-github`

**When it runs:**
- On pull requests
- On pushes to main
- On pushes to release branches

### Adding Checks

To add new validation to CI:

1. Edit `.github/workflows/demo-testing.yml`
2. Add a new step after clippy:

```yaml
- name: "Run my check"
  run: |
    cargo test unit_tests
```

## Test Coverage

Currently the integration tests cover:

- ✅ Server startup and initialization
- ✅ Dependency resolution (paket)
- ✅ Stack graph building
- ✅ Pattern matching queries
- ✅ Location-based filtering (method, field, class)
- ✅ Source vs dependency filtering
- ✅ Result formatting

**Not covered:**
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
