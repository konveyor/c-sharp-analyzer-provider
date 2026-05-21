# Test Infrastructure

Structured gRPC test suite for the C# analyzer provider. Supports both the
C# (Roslyn) and Rust (stack-graphs) providers.

All commands use the unified `test_runner.py` script via `uv run`.

## Quick Start

```bash
# 1. Clone external test repos (only needed once)
uv run testdata/test_runner.py setup
```

### C# provider (Roslyn)

```bash
# 2. Run tests (auto-starts the provider per project)
uv run testdata/test_runner.py run --provider csharp \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"

# Or start the provider yourself and omit --cmd
dotnet run --project CSharpProvider -- --port 9876
uv run testdata/test_runner.py run --provider csharp
```

### Rust provider (stack-graphs)

```bash
# 2. Run tests (auto-starts the provider per project)
uv run testdata/test_runner.py run --provider rust --port 9000 \
  --cmd "cargo run --manifest-path Cargo.toml -- --port 9000"

# Or start the provider yourself and omit --cmd
cargo run --manifest-path Cargo.toml -- --port 9000
uv run testdata/test_runner.py run --provider rust --port 9000
```

## Provider-Specific Query Overrides

Query files can have provider-specific overrides. For a step named
`system-web-mvc.json`, if `system-web-mvc.rust.json` exists, the Rust
provider will use that file instead. The expected output file is always
`system-web-mvc.expected.json` (shared across providers).

This is needed because the C# provider uses full regex patterns
(`^System\.Web\.Mvc(\..*)?$`) while the Rust provider uses glob-style
patterns (`System.Web.Mvc.*`).

## Debugging a Provider

Use `--pause` to pause before and after every gRPC request, giving you time
to attach a debugger and set breakpoints:

```bash
uv run testdata/test_runner.py run --provider csharp --project nerd-dinner --pause
```

The test runner will prompt before each Init/Evaluate call and after each
result, so you can inspect state at every step.

### VS Code (C# provider)

1. Open the CSharpProvider project in VS Code
2. Start the provider with F5 (launch.json should have `--port 9876`)
3. In another terminal: `uv run testdata/test_runner.py run --provider csharp --project nerd-dinner --pause`
4. When prompted, set breakpoints in the Evaluate handler
5. Press Enter to continue

### Rust provider

1. Start the Rust provider: `cargo run --manifest-path src/Cargo.toml -- --port 9000`
2. In another terminal: `uv run testdata/test_runner.py run --provider rust --port 9000 --project nerd-dinner --pause`
3. Attach your debugger to the running process
4. Press Enter to continue

## Adding a New Test Project

1. Create `tests/{project-name}/_.json` manifest:

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
   uv run testdata/test_runner.py run --provider csharp --project my-project --update \
     --cmd "dotnet run --project CSharpProvider -- --port 9876"
   ```

## Updating Golden Files

When provider behavior changes intentionally:

```bash
uv run testdata/test_runner.py run --provider csharp --update \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"
```

This overwrites all `*.expected.json` files with actual results.

## Comparing Providers

Run tests against each provider separately, then diff:

```bash
# Run against both providers
uv run testdata/test_runner.py run --provider csharp --no-check \
  --cmd "dotnet run --project CSharpProvider -- --port 9876"

uv run testdata/test_runner.py run --provider rust --port 9000 --no-check \
  --cmd "cargo run --manifest-path src/Cargo.toml -- --port 9000"

# Compare results
uv run testdata/test_runner.py diff testdata/results/csharp/latest testdata/results/rust/latest
```

## CI Usage

Golden file comparison is on by default -- the runner exits non-zero on any
mismatch:

```bash
uv run testdata/test_runner.py run --provider csharp \
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

### `diff` flags

| Flag | Description |
|------|-------------|
| `LEFT` | Path to left result directory (positional) |
| `RIGHT` | Path to right result directory (positional) |
| `--output DIR` | Output directory for diff files |
| `--project NAME [...]` | Diff only named project(s) |
