# Quick Start: Roslyn C# Provider

Get the Roslyn-based C# provider running with analyzer-lsp in 5 minutes.

## Prerequisites

- .NET SDK 9.0+: `dotnet --version`
- For containers: podman or docker

## 1. Build the Provider

### Option A: Local Development

```bash
cd CSharpProvider
dotnet build -c Release
```

### Option B: Container Image

```bash
./scripts/build-container.sh
# Or: cd CSharpProvider && podman build -f Containerfile -t c-sharp-roslyn .
```

## 2. Test Standalone

### Local Binary

```bash
# Terminal 1: Start the provider
cd CSharpProvider
dotnet run -- --port 14651 --name c-sharp-roslyn

# Terminal 2: Test with grpcurl (install from https://github.com/fullstorydev/grpcurl)
grpcurl -plaintext localhost:14651 list
grpcurl -plaintext localhost:14651 provider.ProviderService.Capabilities
```

### Container

```bash
# Terminal 1: Start container
./scripts/test-container.sh
# Or: podman run --rm -p 14651:14651 c-sharp-roslyn

# Terminal 2: Test
grpcurl -plaintext localhost:14651 provider.ProviderService.Capabilities
```

## 3. Configure for analyzer-lsp

Create `provider-config.yaml`:

```yaml
# For local binary
- name: c-sharp-roslyn
  binaryPath: dotnet
  address: localhost:14651
  useSocket: false
  initConfig:
    - location: /path/to/your/csharp/project
      analysisMode: source-only

# For container (comment out the above, uncomment this)
# - name: c-sharp-roslyn
#   binaryPath: ""  # Empty for container
#   address: ""     # Managed by kantra
#   useSocket: true
#   initConfig:
#     - location: /analyzer/projects/myapp
#       analysisMode: source-only
```

## 4. Run Analysis

### With analyzer-lsp

```bash
# analyzer-lsp will start/stop the provider automatically
analyzer-lsp \
  --provider-settings provider-config.yaml \
  --rules /path/to/rulesets \
  --output /path/to/output
```

### With kantra

```bash
kantra analyze \
  --input /path/to/csharp/app \
  --output /path/to/output \
  --rules /path/to/rulesets \
  --provider-settings provider-config.yaml
```

kantra will automatically:
- Detect C# projects in the input
- Start the provider (or spawn the container)
- Run the analysis
- Generate the report

## 5. Example Rule

Rules query the provider using the `referenced` capability:

```yaml
- ruleID: dotnet-framework-to-core-001
  description: Find ASP.NET MVC usage
  when:
    dotnet.referenced:
      pattern: System\.Web\.Mvc\..*
  perform:
    message: "ASP.NET MVC detected - consider migrating to ASP.NET Core"
```

## Common Commands

```bash
# Build
cd CSharpProvider && dotnet build

# Run locally
dotnet run -- --port 14651

# Build container
./scripts/build-container.sh

# Test container
./scripts/test-container.sh

# Run tests
cd CSharpProvider && dotnet test

# Clean
dotnet clean
```

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Provider won't start | Check .NET SDK: `dotnet --version` (need 9.0+) |
| Port already in use | Change port: `--port 9876` |
| Container permission denied | Add `:Z` to volume mounts for SELinux |
| No incidents found | Check pattern syntax, ensure full namespace |
| Init fails | Verify project path has `.csproj` or `.sln` |

## Next Steps

- **[INTEGRATION.md](INTEGRATION.md)**: Complete integration guide
- **[CSharpProvider/ARCHITECTURE.md](CSharpProvider/ARCHITECTURE.md)**: How it works
- **[CSharpProvider/tests/README.md](CSharpProvider/tests/README.md)**: Testing guide

## Example Analysis Session

```bash
# 1. Start provider
cd CSharpProvider
dotnet run -- --port 14651 &

# 2. Init with a project
grpcurl -plaintext -d '{
  "location": "../testdata/nerd-dinner/mvc4",
  "analysisMode": "source-only"
}' localhost:14651 provider.ProviderService.Init

# 3. Find all System.Web.Mvc references
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Web\\\\.Mvc\\\\..*\"}}",
  "id": 1
}' localhost:14651 provider.ProviderService.Evaluate

# 4. Stop provider
grpcurl -plaintext -d '{"id": 1}' localhost:14651 provider.ProviderService.Stop
kill %1  # Stop background process
```

## Need Help?

- Check the logs: `--log-file /tmp/provider.log`
- Increase verbosity: set `DOTNET_LogLevel=Debug`
- Review [INTEGRATION.md](INTEGRATION.md) troubleshooting section
- Open an issue on GitHub
