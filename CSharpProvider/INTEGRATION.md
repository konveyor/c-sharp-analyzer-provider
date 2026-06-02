# Integrating CSharpProvider with analyzer-lsp

This guide explains how to configure and use the Roslyn-based C# provider with Konveyor's analyzer-lsp engine.

## Overview

The CSharpProvider implements the same gRPC protocol as other analyzer-lsp providers, making it a drop-in replacement for the tree-sitter-based C# provider. It uses Microsoft's Roslyn compiler platform for superior symbol resolution.

## Architecture

```
kantra (CLI) 
  └─> analyzer-lsp (Go, gRPC client)
       └─> CSharpProvider (C#, gRPC server)
            └─> Roslyn (Microsoft.CodeAnalysis)
```

## Prerequisites

- **.NET SDK 9.0 or higher** (required - check with `dotnet --version`)
  - The provider uses features from .NET 9
  - Download from: https://dotnet.microsoft.com/download/dotnet/9.0
- For container deployments: podman or docker

**Important:** If you only have .NET 6.0 SDK, you'll need to upgrade to .NET 9.0 SDK to build and run this provider.

## Configuration Options

### Option 1: Local Binary (Development)

Best for development and testing.

1. **Build the provider:**
   ```bash
   cd CSharpProvider
   dotnet build -c Release
   ```

2. **Create provider configuration** (`provider-settings.yaml`):
   ```yaml
   - name: c-sharp-roslyn
     binaryPath: dotnet
     address: localhost:14651
     useSocket: false
     initConfig:
       - location: /path/to/your/csharp/project
         analysisMode: source-only
   ```

3. **Run manually for testing:**
   ```bash
   cd CSharpProvider
   dotnet run -- --port 14651 --name c-sharp-roslyn
   ```

4. **Or let analyzer-lsp manage it:**
   ```bash
   # analyzer-lsp will start the provider based on the config
   analyzer-lsp --provider-settings provider-settings.yaml
   ```

### Option 2: Container Image (Production)

Best for production deployments with kantra.

1. **Build the container image:**
   ```bash
   podman build -f CSharpProvider/Containerfile -t quay.io/konveyor/c-sharp-roslyn-provider:latest .
   ```

2. **Test the container:**
   ```bash
   podman run --rm -p 14651:14651 \
     -v /path/to/project:/projects:Z \
     quay.io/konveyor/c-sharp-roslyn-provider:latest
   ```

3. **Configure kantra** to use the container:
   
   The provider can be registered in kantra's provider configuration. When kantra detects a C# project, it will automatically spawn this container.

### Option 3: Unix Socket

Best for local IPC without network exposure.

1. **Run with socket:**
   ```bash
   cd CSharpProvider
   dotnet run -- --socket /tmp/c-sharp-provider.sock --name c-sharp-roslyn
   ```

2. **Provider configuration:**
   ```yaml
   - name: c-sharp-roslyn
     address: /tmp/c-sharp-provider.sock
     useSocket: true
     initConfig:
       - location: /path/to/project
         analysisMode: source-only
   ```

## Provider Configuration Reference

### Required Fields

- **`name`**: Unique identifier for the provider (e.g., "c-sharp-roslyn")
- **`initConfig.location`**: Path to the C# project to analyze
- **`initConfig.analysisMode`**: Analysis mode (`"source-only"` or `"full"`)

### Optional Fields

- **`binaryPath`**: Path to provider executable (empty for containers)
- **`address`**: TCP address (e.g., "localhost:14651") or Unix socket path
- **`useSocket`**: `true` for Unix sockets, `false` for TCP (default: `false`)
- **`contextLines`**: Number of context lines in code snippets (default: 10)
- **`logLevel`**: Logging verbosity (0-5, higher = more verbose)

### Provider-Specific Config

The `providerSpecificConfig` section can include:

```yaml
providerSpecificConfig:
  contextLines: 15  # Override default context lines
```

## Command-Line Arguments

When running the provider directly:

| Argument | Description | Default |
|----------|-------------|---------|
| `--port <PORT>` | TCP port for gRPC | 14651 |
| `--socket <PATH>` | Unix socket path | (none) |
| `--name <NAME>` | Provider name | c-sharp |
| `--context-lines <N>` | Code snippet context lines | 10 |
| `--log-file <PATH>` | Log file path | (none) |

## Testing the Integration

### 1. Test gRPC Connectivity

```bash
# Start the provider
cd CSharpProvider
dotnet run -- --port 14651

# List available services (requires grpcurl)
grpcurl -plaintext localhost:14651 list

# Expected output:
# grpc.reflection.v1.ServerReflection
# grpc.reflection.v1alpha.ServerReflection
# provider.ProviderCodeLocationService
# provider.ProviderService
```

### 2. Test Init RPC

```bash
grpcurl -plaintext -d '{
  "location": "/path/to/csharp/project",
  "analysisMode": "source-only"
}' localhost:14651 provider.ProviderService.Init

# Expected output:
# {
#   "successful": true,
#   "id": "1",
#   "builtinConfig": {
#     "location": "/current/directory",
#     "analysisMode": "source-only"
#   }
# }
```

### 3. Test Evaluate RPC

```bash
grpcurl -plaintext -d '{
  "cap": "referenced",
  "conditionInfo": "{\"referenced\": {\"pattern\": \"System\\\\.Console.*\"}}",
  "id": 1
}' localhost:14651 provider.ProviderService.Evaluate

# Expected output:
# {
#   "successful": true,
#   "response": {
#     "matched": true,
#     "incidentContexts": [
#       {
#         "fileURI": "file:///path/to/file.cs",
#         "codeLocation": { ... },
#         ...
#       }
#     ]
#   }
# }
```

## Using with kantra

Once configured, use kantra normally:

```bash
kantra analyze \
  --input /path/to/csharp/app \
  --output /path/to/output \
  --rules /path/to/rulesets \
  --provider-settings provider-config-roslyn.yaml
```

kantra will:
1. Detect C# projects in the input
2. Start the CSharpProvider (or spawn the container)
3. Initialize the provider with the project location
4. Execute rules that use the "referenced" capability
5. Collect incidents and generate the analysis report

## Capabilities

The CSharpProvider currently supports:

- **`referenced`**: Find references to types, methods, fields, etc. using regex patterns

Query format:
```json
{
  "referenced": {
    "pattern": "Fully\\.Qualified\\.Name\\.Pattern.*",
    "location": "method|field|class|annotation|import|type_reference|object_creation|type_usage"
  }
}
```

## Volume Mounts (Container Mode)

When running in a container, ensure proper volume mounts:

```bash
podman run --rm \
  -v /path/to/analyzed/project:/projects:Z \
  -v /path/to/output:/output:Z \
  quay.io/konveyor/c-sharp-roslyn-provider:latest
```

The `:Z` flag ensures SELinux labeling for container access.

## Troubleshooting

### Provider doesn't start

- **Check .NET SDK**: `dotnet --version` (requires 9.0+)
- **Check port availability**: `lsof -i :14651`
- **Check logs**: Use `--log-file /tmp/provider.log`

### Init fails

- **Project path**: Ensure the location exists and contains `.csproj` or `.sln` files
- **Permissions**: Container user (1001) needs read/write access to the project directory
- **Dependencies**: If using .NET Framework projects, ensure reference assemblies are available

### No incidents found

- **Pattern syntax**: Ensure regex pattern is properly escaped in JSON
- **Case sensitivity**: Patterns are case-sensitive
- **Namespace**: Include full namespace in pattern (e.g., `System\\.Web\\.Mvc.*`)

### Container permission errors

- **SELinux**: Use `:Z` flag on volume mounts
- **User ID**: Container runs as user 1001; ensure files are accessible

## Advanced Configuration

### Log Levels

Set logging verbosity:

```yaml
initConfig:
  - location: /path/to/project
    analysisMode: source-only
    logLevel: 3  # 0=Critical, 1=Error, 2=Warning, 3=Info, 4=Debug, 5=Trace
```

Or via command line:
```bash
dotnet run -- --port 14651 --log-file /tmp/provider.log
# Then check DOTNET_LogLevel environment variable
```

### Custom Context Lines

Adjust code snippet context:

```bash
dotnet run -- --port 14651 --context-lines 20
```

## Differences from tree-sitter Provider

| Feature | tree-sitter Provider | Roslyn Provider |
|---------|---------------------|-----------------|
| Symbol Resolution | Stack graphs | Semantic model |
| Project Files | Not required | Recommended for best results |
| Dependencies | Decompile with ILSpy | Resolved via NuGet/MSBuild |
| Incremental Updates | Limited | Supported via NotifyFileChanges |
| .NET Framework Support | Limited | Full (v2.0 - v4.8) |
| Performance | Fast startup, cached graph | Slower startup, faster queries |

## Next Steps

- See [ARCHITECTURE.md](CSharpProvider/ARCHITECTURE.md) for implementation details
- See [README.md](README.md) for general provider information
- Report issues at the Konveyor GitHub repository
