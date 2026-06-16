# C# Analyzer Provider

Roslyn-based C# analyzer for analyzer-lsp and kantra. Uses `CSharpCompilation` and `SemanticModel` for full symbol resolution.

## Running

```bash
dotnet build src/CSharpProvider.csproj
dotnet run --project . -- --port 9876
```

## How it works

See [ARCHITECTURE.md](ARCHITECTURE.md) for a more thorough explanation.

1. **Project loading** (`Analysis/ProjectLoader.cs`): Tries MSBuild workspace first (`.sln` / SDK-style `.csproj`), falls back to ad-hoc compilation with NuGet package resolution.

2. **Package resolution** (`Analysis/PackageResolver.cs`): Downloads NuGet packages for the detected target framework. Supports `paket.lock`, `packages.config`, and `PackageReference` formats.

3. **Symbol querying** (`Analysis/SymbolQuery.cs`): Walks the syntax tree with a `CSharpSyntaxWalker`, resolving symbols via `SemanticModel`. Matches FQDNs against regex patterns.

### What it finds

- **Imports**: `using` directives
- **Type references**: variable types, return types, parameters, casts, `typeof`, generics, base classes
- **Member access**: method calls, property/field access (explicit and implicit `this`)
- **Object creation**: `new T()`
- **Attributes**: `[Authorize]`, etc.
- **Value flow**: arguments, return values, and assignments where the expression type matches
- **Dynamic access**: `ViewBag.X` and similar dynamic member chains
- **Inheritance-aware matching**: querying `Controller` also finds members accessed through `Controller`, even if declared on `ControllerBase`

And more!

## Testing

See [`tests/README.md`](tests/README.md) for the full test infrastructure.

## Container Deployment

### Building the Container Image

```bash
# Using the build script
cd src
../scripts/build-container.sh

# Or manually with podman
podman build -f Dockerfile -t quay.io/konveyor/c-sharp-roslyn-provider:latest .
```

### Testing the Container

```bash
# Using the test script (interactive)
../scripts/test-container.sh

# Or manually
podman run --rm -p 14651:14651 quay.io/konveyor/c-sharp-roslyn-provider:latest

# Test with a project volume
podman run --rm -p 14651:14651 \
  -v /path/to/your/project:/projects:Z \
  quay.io/konveyor/c-sharp-roslyn-provider:latest
```

### Container Runtime Permissions

The container:
- Runs as user 1001 (non-root)
- Has group 0 (root group) permissions for OpenShift compatibility
- Requires `:Z` flag on volume mounts for SELinux systems
- Includes the .NET SDK for runtime `dotnet restore` on analyzed projects

## Integration with analyzer-lsp

See [INTEGRATION.md](../INTEGRATION.md) for complete integration guide including:
- Provider configuration examples
- Testing with analyzer-lsp/kantra
- Troubleshooting tips
- Advanced configuration options

## Query format

Patterns are regex, matched against the fully-qualified symbol name:

```bash
# Exact type
grpcurl -plaintext -d '{"cap":"referenced","id":"1","conditionInfo":"{\"referenced\":{\"pattern\":\"^System\\\\.Web\\\\.Mvc\\\\.Controller$\"}}"}' \
  localhost:9876 provider.ProviderService/Evaluate

# Wildcard (type + all members)
grpcurl -plaintext -d '{"cap":"referenced","id":"1","conditionInfo":"{\"referenced\":{\"pattern\":\"^System\\\\.Web\\\\.Mvc\\\\.Controller(\\\\..*)$\"}}"}' \
  localhost:9876 provider.ProviderService/Evaluate
```
