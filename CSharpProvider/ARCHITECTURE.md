# CSharpProvider Architecture

Roslyn-based C# analyzer provider for
[konveyor/analyzer-lsp](https://github.com/konveyor/analyzer-lsp). Uses
`Microsoft.CodeAnalysis` (the C# compiler platform) for symbol resolution.
Drop-in replacement for the tree-sitter/stack-graphs engine - same
`provider.proto` gRPC contract, same request/response format.

## How It Fits in Konveyor

```
kantra (CLI) -> spawns containers -> analyzer-lsp (Go, gRPC client) -> providers (gRPC servers)
```

Each provider is a container that analyzer-lsp talks to over TCP. kantra spawns
them with `podman run`, runs analysis, then destroys them. No state persists
between runs.

## Init - Loading Projects

`ProviderService.Init` -> `ProjectLoader.LoadAsync` turns a project directory
into a Roslyn `CSharpCompilation` (in-memory representation of all source with
full type information).

### 1. Try MSBuild first

Looks for `.sln`, then SDK-style `.csproj` files. Runs `dotnet restore`, opens
via `MSBuildWorkspace`, gets a `CSharpCompilation`. If the compilation has
unresolved type errors (CS0246/CS0234), it throws and falls back to ad-hoc.

### 2. Ad-hoc fallback

For legacy .NET Framework projects where MSBuild can't restore on Linux:

1. Collects all `.cs` files, parses them into syntax trees
2. Detects target framework from `.csproj`
   (`<TargetFrameworkVersion>v4.5</TargetFrameworkVersion>` -> `net45`)
3. Discovers references from three sources:
   - **Framework assemblies**: for .NET Framework targets (`net20`, `net35`,
     `net45`, `net472`, etc.), downloads the matching
     `Microsoft.NETFramework.ReferenceAssemblies` NuGet package. For .NET
     Core / .NET 5+ targets, uses on-disk SDK reference assemblies from
     `DOTNET_ROOT`.
   - **NuGet packages**: parses `paket.lock`, `packages.config`, or
     `<PackageReference>` from `.csproj` (in that priority order), resolves
     via `PackageResolver`.
   - **On-disk DLLs**: scans `packages/` and `lib/` directories.
4. Deduplicates references by filename
5. Builds a `CSharpCompilation` with trees + references

Best-effort: the goal is migration analysis, not compilation. Even with some
unresolved types, Roslyn resolves far more than stack-graphs.

## Evaluate - Reference Resolution

`ProviderService.Evaluate` -> `SymbolQuery.Execute` queries for all references
matching a regex pattern against fully-qualified symbol names.

### 1. Parse the condition

Parses the `conditionInfo` JSON to get a regex pattern, optional location
filter (`method`, `field`, `class`), and optional file path filter.

### 2. Walk the syntax tree

Uses a `CSharpSyntaxWalker` (visitor pattern) that overrides specific `Visit*`
methods. For each node, resolves the symbol via the `SemanticModel`, builds its
fully-qualified name, and regex-matches against the query pattern.

| Visitor | What it catches | Result type |
|---|---|---|
| `VisitUsingDirective` | `using System.Web.Mvc;` | `import` |
| `VisitMemberAccessExpression` | `Request.Cookies`, `db.SaveChanges()` | `method_reference` / `field_reference` |
| `VisitInvocationExpression` | `Response.Write(...)` | `method_reference` |
| `VisitIdentifierName` | `User` (implicit `this.User`) | `method_reference` / `field_reference` |
| `VisitClassDeclaration` | `class Foo` + `: BaseClass` | `class_def` / `type_reference` |
| `VisitStructDeclaration` | `struct Foo : IBar` | `type_reference` |
| `VisitInterfaceDeclaration` | `interface IFoo : IBar` | `type_reference` |
| `VisitObjectCreationExpression` | `new DbContext()` | `object_creation` |
| `VisitVariableDeclaration` | `DbContext db = ...` | `type_reference` |
| `VisitPropertyDeclaration` | `public DbContext Db { get; }` | `type_reference` |
| `VisitFieldDeclaration` | `private DbContext _db;` | `type_reference` |
| `VisitMethodDeclaration` | `public ActionResult Index()` | `type_reference` (return type) |
| `VisitParameter` | `void Foo(DbContext db)` | `type_reference` |
| `VisitCastExpression` | `(Controller)x` | `type_reference` |
| `VisitIsPatternExpression` | `x is Controller` | `type_reference` |
| `VisitTypeOfExpression` | `typeof(Controller)` | `type_reference` |
| `VisitAttribute` | `[Authorize]` | `annotation` |
| `VisitArgument` | `SomeMethod(dbContext)` | `type_usage` (value flow) |
| `VisitReturnStatement` | `return dbContext` | `type_usage` (value flow) |
| `VisitAssignmentExpression` | `x = dbContext` | `type_usage` (value flow) |

### 3. Inheritance-aware matching

When you write `this.User` in a class extending `Controller`, Roslyn resolves
the symbol to `ControllerBase.User` (where it's declared), not
`Controller.User`. A query for `System\.Web\.Mvc\.Controller\..*` would miss it.

`MatchesFqdnInHierarchy` fixes this: when the direct FQDN doesn't match, it
walks the inheritance chain from the access type up to the declaring type,
generating alternative FQDNs at each level.

### 4. Dynamic member access

`ViewBag` is typed as `dynamic` - `GetSymbolInfo()` returns null past it. The
walker detects this by walking down the expression tree to the deepest
resolvable part, checking if its return type is `dynamic`, and building the
FQDN from the resolved portion plus the unresolved member names
(e.g., `System.Web.Mvc.Controller.ViewBag.ReturnUrl`).

### 5. Value-flow tracking

Beyond direct references, the walker tracks where values of a queried type flow
through arguments, return statements, and assignments. `GetTypeInfo` on the
expression walks the base type chain and interfaces, so
`SomeMethod(myController)` matches a query for `Controller` even when
`myController` is typed as `MyCustomController : Controller`.

### 6. Deduplicate and respond

Results are deduped by `(fileUri, startLine, startChar, endLine, endChar)` so
multiple references on the same span collapse, but distinct references on the
same line are preserved (e.g., `Dog x = new Dog()` has both a type reference
and an object creation). Converted to `IncidentContext` protobuf messages.

## Other RPCs

- **`GetCodeSnip`** - reads a file from disk, returns lines around a target
  location with context padding
- **`Stop`** - clears `ProjectStateHolder` (drops compilation from memory)
- **`Capabilities`** - returns `["referenced"]`
- **`GetDependencies` / `GetDependenciesDAG`** - stubs (empty success)

## Container

Multi-stage build: .NET SDK 9.0 for `dotnet publish`, UBI 10 minimal with
`dotnet-sdk-9.0` for runtime. The SDK is needed at runtime for `dotnet restore`
on mounted projects. `DOTNET_ROOT=/usr/lib64/dotnet` for RHEL's SDK path.

Runs as user 1001. Source projects should be mounted via a podman volume (not a
direct bind mount) so the user has write access for `dotnet restore`'s `obj/`
output.

## Project Structure

```
CSharpProvider/
  Program.cs                          # CLI parsing, ASP.NET Core/gRPC setup
  Containerfile                       # Multi-stage build (SDK builder, UBI 10 + SDK runtime)
  Services/
    ProviderService.cs                # Init, Evaluate, Stop, Capabilities RPCs
    CodeLocationService.cs            # GetCodeSnip RPC
    ProjectStateHolder.cs             # Thread-safe singleton holding CSharpCompilation
  Analysis/
    ProjectLoader.cs                  # Loads projects into CSharpCompilation
    PackageResolver.cs                # NuGet package download and resolution
    SymbolQuery.cs                    # CSharpSyntaxWalker + SemanticModel queries
```

ASP.NET Core creates a new `ProviderService` instance per gRPC call. Shared
state lives in the `ProjectStateHolder` singleton.
