# CSharpProvider (Roslyn)

Roslyn-based replacement for the tree-sitter/stack-graphs C# analyzer. Uses `CSharpCompilation` and `SemanticModel` for full symbol resolution.

## Running

```bash
dotnet build CSharpProvider.csproj
dotnet run --project . -- --port 9876
```

## How it works

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

## Testing

See [`tests/README.md`](tests/README.md) for the full test infrastructure.

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
