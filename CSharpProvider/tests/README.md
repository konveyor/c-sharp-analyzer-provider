# Test Infrastructure

Structured gRPC test suite for the C# analyzer provider. Supports both the
C# (Roslyn) and Rust (stack-graphs) providers.

All commands use the unified `test_runner.py` script via `uv run`.

## Quick Start

```bash
# 1. Clone external test repos (only needed once)
uv run CSharpProvider/tests/test_runner.py setup
```

### C# provider (Roslyn)

```bash
# 2. Run tests (auto-starts the provider per project)
uv run CSharpProvider/tests/test_runner.py run --provider csharp \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"

# Or start the provider yourself and omit --cmd
dotnet run --project CSharpProvider -- --port 9876
uv run CSharpProvider/tests/test_runner.py run --provider csharp
```

### Rust provider (stack-graphs)

```bash
# 2. Run tests (auto-starts the provider per project)
uv run CSharpProvider/tests/test_runner.py run --provider rust --port 9000 \
  --cmd "cargo run -- --port 9000"

# Or start the provider yourself and omit --cmd
cargo run -- --port 9000
uv run CSharpProvider/tests/test_runner.py run --provider rust --port 9000
```

## Provider-Specific Query Overrides

Query files can have provider-specific overrides. For a step named
`system-web-mvc.json`, if `system-web-mvc.rust.json` exists, the Rust
provider will use that file instead. The expected output file is always
`system-web-mvc.expected.json` (shared across providers).

This is needed because the C# provider uses full regex patterns
(`^System\.Web\.Mvc(\..*)?$`) while the Rust provider uses glob-style
patterns (`System.Web.Mvc.*`).

## Raw grpcurl

For ad-hoc testing outside the test runner, you can call the provider directly
with `grpcurl`.

```bash
# Start the provider
dotnet run --project CSharpProvider -- --port 9876 &

# List available services
grpcurl -plaintext localhost:9876 list

# Init with a project
grpcurl -plaintext -d '{
  "location": "/absolute/path/to/project"
}' localhost:9876 provider.ProviderService/Init

# Evaluate a query
grpcurl -max-msg-sz 10485760 -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"^System\\\\.Web\\\\.Mvc(\\\\..*)?$\"}}",
  "id": 1
}' localhost:9876 provider.ProviderService/Evaluate

# Stop the provider
grpcurl -plaintext -d '{}' localhost:9876 provider.ProviderService/Stop
```

Note: `conditionInfo` is a JSON string containing a JSON object, so the inner
quotes and backslashes need double-escaping on the command line. The C# provider
also accepts YAML-formatted `conditionInfo` (as sent by analyzer-lsp).

## Debugging a Provider

Use `--pause` to pause before and after every gRPC request, giving you time
to attach a debugger and set breakpoints:

```bash
uv run CSharpProvider/tests/test_runner.py run --provider csharp --project nerd-dinner --pause
```

The test runner will prompt before each Init/Evaluate call and after each
result, so you can inspect state at every step.

### VS Code (C# provider)

1. Open the CSharpProvider project in VS Code
2. Start the provider with F5 (launch.json should have `--port 9876`)
3. In another terminal: `uv run CSharpProvider/tests/test_runner.py run --provider csharp --project nerd-dinner --pause`
4. When prompted, set breakpoints in the Evaluate handler
5. Press Enter to continue

### Rust provider

1. Start the Rust provider: `cargo run -- --port 9000`
2. In another terminal: `uv run CSharpProvider/tests/test_runner.py run --provider rust --port 9000 --project nerd-dinner --pause`
3. Attach your debugger to the running process
4. Press Enter to continue

## Adding a New Test Project

1. Create `suites/{project-name}/_.json` manifest:

   **For an external repo:**
   ```json
   {
     "repo": {
       "url": "https://github.com/org/repo",
       "commit": "abc123...",
       "path": "optional/subdirectory"
     },
     "steps": ["init.json", "my-query.json"]
   }
   ```

   **For an in-tree repo (committed under `repos/`):**
   ```json
   {
     "repo": {},
     "steps": ["init.json", "my-query.json"]
   }
   ```

2. Create `init.json` (Init request):
   ```json
   {
     "location": "",
     "analysisMode": "source-only"
   }
   ```
   Set `location` to a subdirectory if the project root differs from the
   repo root (e.g. `"mvc4"` for nerd-dinner).

3. Create evaluate request files (e.g. `my-query.json`):
   ```json
   {
     "cap": "referenced",
     "conditionInfo": {
       "referenced": {
         "pattern": "^System\\.Web\\.Mvc(\\..*)?"
       }
     }
   }
   ```
   The `conditionInfo` object is serialized to a JSON string by the runner.

4. Optionally create `my-query.rust.json` with the Rust provider's pattern:
   ```json
   {
     "cap": "referenced",
     "conditionInfo": {
       "referenced": {
         "pattern": "System.Web.Mvc.*"
       }
     }
   }
   ```

5. Run `test_runner.py setup` if the project has a `repo.url`

6. Generate golden files:
   ```bash
   uv run CSharpProvider/tests/test_runner.py run --provider csharp --project my-project --update \
     --cmd "dotnet run --project CSharpProvider -- --port 9876"
   ```

## Updating Golden Files

When provider behavior changes intentionally:

```bash
uv run CSharpProvider/tests/test_runner.py run --provider csharp --update \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"
```

This overwrites all `*.expected.json` files with actual results.

## Comparing Providers

Run tests against each provider separately, then diff:

```bash
# Run against both providers
uv run CSharpProvider/tests/test_runner.py run --provider csharp --no-check \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"

uv run CSharpProvider/tests/test_runner.py run --provider rust --port 9000 --no-check \
  --cmd "cargo run -- --port 9000"

# Compare results (--rust-compat relaxes matching for Rust provider quirks)
uv run CSharpProvider/tests/test_runner.py diff CSharpProvider/tests/results/csharp/latest CSharpProvider/tests/results/rust/latest \
  --rust-compat
```

## Container Testing

Build the container and run the full test suite against it:

```bash
podman build -t csharp-provider -f CSharpProvider/Containerfile CSharpProvider/

uv run CSharpProvider/tests/test_runner.py run --provider csharp \
  --repo-root /repos \
  --cmd "podman run --rm -p 9876:9876 -v $(realpath CSharpProvider/tests/repos):/repos:Z csharp-provider --port 9876"
```

`--repo-root /repos` tells the test runner to send `/repos/<project>` as the
init location instead of the host-side absolute path, matching the volume mount
inside the container.

## CI Usage

Golden file comparison is on by default -- the runner exits non-zero on any
mismatch:

```bash
uv run CSharpProvider/tests/test_runner.py run --provider csharp \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"
```

Use `--fail-fast` to stop on the first failure instead of running all tests.

## Subcommands Reference

| Command | Description |
|---------|-------------|
| `setup` | Clone external test repos defined by manifests |
| `run`   | Run gRPC tests against a provider |
| `diff`  | Compare two result directories |

### `run` flags

| Flag | Description |
|------|-------------|
| `--provider {csharp,rust}` | Provider label for result directory (default: csharp) |
| `--port PORT` | Provider gRPC port (default: 9876) |
| `--cmd CMD` | Command to start the provider (fresh instance per project) |
| `--project NAME [...]` | Run only named project(s) |
| `--update` | Overwrite golden files with actual results |
| `--no-check` | Skip golden file comparison |
| `--verbose` | Print full result JSON on failure |
| `--fail-fast` | Stop on first failure |
| `--pause` | Pause before and after each request (for debugging) |
| `--repo-root PATH` | Override repo path sent to the provider |

### `diff` flags

| Flag | Description |
|------|-------------|
| `LEFT` | Path to left result directory (positional) |
| `RIGHT` | Path to right result directory (positional) |
| `--output DIR` | Output directory for diff files |
| `--project NAME [...]` | Diff only named project(s) |
| `--rust-compat` | Relax matching for Rust provider quirks (e.g. missing character offsets) |
