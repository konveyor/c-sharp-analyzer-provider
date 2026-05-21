# CSharpProvider Architecture

Roslyn-based C# analyzer provider for [konveyor/analyzer-lsp](https://github.com/konveyor/analyzer-lsp). Uses `Microsoft.CodeAnalysis` (the C# compiler platform) for symbol resolution. Drop-in replacement for the tree-sitter/stack-graphs engine - same `provider.proto` gRPC contract, same request/response format.

## Project Structure

```
CSharpProvider/
├── Program.cs                          # CLI parsing, ASP.NET Core/gRPC setup
├── Containerfile                       # Multi-stage build (SDK builder, UBI 10 + SDK runtime)
├── Services/
│   ├── ProviderService.cs              # Init, Evaluate, Stop, Capabilities RPCs
│   ├── CodeLocationService.cs          # GetCodeSnip RPC
│   └── ProjectStateHolder.cs           # Thread-safe singleton holding CSharpCompilation
└── Analysis/
    ├── ProjectLoader.cs                # Loads projects into CSharpCompilation
    ├── PackageResolver.cs              # NuGet package download and resolution
    ├── SymbolQuery.cs                  # CSharpSyntaxWalker + SemanticModel queries
    └── ResultBuilder.cs                # Deduplication + gRPC response building
```

## How It Fits in Konveyor

```
kantra (CLI) → spawns containers → analyzer-lsp (Go, gRPC client) → providers (gRPC servers)
```

Each provider is a container that analyzer-lsp talks to over TCP. kantra spawns them with `podman run`, runs analysis, then destroys them. No state persists between runs.

ASP.NET Core creates a new `ProviderService` instance per gRPC call. Shared state lives in the `ProjectStateHolder` singleton.

## Init - Loading Projects

`Init(location="/path/to/project")` turns a directory into a Roslyn `CSharpCompilation` - an in-memory representation of all source code with full type information. Two loading paths:

### MSBuildWorkspace (modern SDK-style projects)

1. Find `.sln` or SDK-style `.csproj` in the directory
2. Run `dotnet restore` to fetch NuGet packages
3. Open via `MSBuildWorkspace.OpenSolutionAsync()` / `OpenProjectAsync()`
4. Roslyn resolves all references (framework, NuGet, project-to-project)
5. Validate: if CS0246/CS0234 unresolved type errors are detected, fall back to ad-hoc

### Ad-hoc compilation (legacy .NET Framework projects)

For older projects (e.g., nerd-dinner MVC 4) where MSBuild can't restore on Linux without Mono:

1. Parse all `*.cs` files into `SyntaxTree`s
2. Detect target framework from `.csproj` (`<TargetFrameworkVersion>v4.5</TargetFrameworkVersion>` → `net45`)
3. Discover packages in priority order: `paket.lock` → `packages.config` → `<PackageReference>`
4. Download NuGet packages via the NuGet client SDK (`NuGet.Protocol`), cached in `/tmp/csharp-provider-cache/`
5. Select best TFM-compatible DLLs using NuGet's `DefaultCompatibilityProvider`
6. Add framework reference assemblies: `Microsoft.NETFramework.ReferenceAssemblies` for `net4*`, or SDK packs from `DOTNET_ROOT` for modern .NET
7. Scan `packages/` and `lib/` for on-disk DLLs
8. `CSharpCompilation.Create()` with trees + references

Best-effort: the goal is migration analysis, not compilation. Even with some unresolved types, Roslyn resolves far more than stack-graphs.

## Evaluate - Reference Resolution

`Evaluate(cap="referenced", conditionInfo="{\"referenced\":{\"pattern\":\"...\"}}")` queries for all references matching a regex pattern against fully-qualified symbol names.

### Syntax Walking

A `CSharpSyntaxWalker` visits every node in the AST. For each node, it resolves the symbol through `SemanticModel` and matches the FQDN against the query pattern:

| Visitor | What it catches | Result type |
|---|---|---|
| `VisitUsingDirective` | `using System.Web.Mvc;` | `import` |
| `VisitMemberAccessExpression` | `Request.Cookies`, `db.SaveChanges()` | `method_reference` / `field_reference` |
| `VisitInvocationExpression` | `Response.Write(...)` | `method_reference` |
| `VisitIdentifierName` | `User` (implicit `this.User`) | `method_reference` / `field_reference` |
| `VisitClassDeclaration` | `class Foo` + `: BaseClass` | `class_def` / `type_reference` |
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

### FQDN Construction

For a symbol like `Cookies` on `HttpRequest`, the FQDN is built from `Namespace.ContainingType.Name` → `System.Web.HttpRequest.Cookies`. Roslyn gives us this directly from the type system - no graph traversal needed.

### Inheritance-Aware Matching

When you write `this.User` in a class extending `Controller`, Roslyn resolves the symbol to `ControllerBase.User` (where it's declared), not `Controller.User`. A query for `System\.Web\.Mvc\.Controller\..*` would miss it.

`MatchesFqdnInHierarchy` fixes this: when the direct FQDN doesn't match, it walks the inheritance chain from the access type up to the declaring type, generating alternative FQDNs at each level. A guard check ensures the declaring type is actually in the hierarchy to prevent false positives (e.g., `IsolationLevel` from `TransactionOptions` being attributed to `Controller`).

### Implicit `this` Access

C# allows `User` instead of `this.User` for inherited members. These have no `.` in the syntax, so they don't go through `VisitMemberAccessExpression`. `VisitIdentifierName` catches these by resolving the symbol, checking it's a member (not a local/parameter/namespace/type), finding the enclosing type, and running hierarchy-aware matching.

### Invocation Argument Walking

`VisitInvocationExpression` calls `base.VisitInvocationExpression(node)` to walk argument subtrees. Without this, expressions inside arguments (like `UrlParameter.Optional` in `MapRoute(...)`) are silently skipped. `VisitMemberAccessExpression` coordinates to avoid double-reporting: if a member access is the direct expression of an invocation, it skips it (the invocation handler already resolved it).

### Value-Flow Tracking

Beyond direct references, we track where values of a queried type flow through arguments, return statements, and assignments. `GetTypeInfo` on the expression walks the base type chain and interfaces, so `SomeMethod(myController)` matches a query for `Controller` even when `myController` is typed as `MyCustomController : Controller`.

### Dynamic Member Access

`ViewBag` is typed as `dynamic` - `GetSymbolInfo()` returns null past it. The walker detects this by walking down the expression tree to the deepest resolvable part, checking if its return type is `dynamic`, and recording the hit against the resolved property (e.g., `System.Web.Mvc.Controller.ViewBag`).

### Span Accuracy

Results report the span of the specific syntax node, not the enclosing declaration. A method's return type reports the span of the type name, not the entire method (which would include attributes like `[HttpPost]`).

### Deduplication

Results are deduped by `(fileUri, startLine, startChar, endLine, endChar)` so multiple distinct references on the same line are preserved (e.g., `Dog x = new Dog()` has both a type reference and an object creation).

## Container

Multi-stage build: .NET SDK 9.0 for `dotnet publish`, UBI 10 minimal with `dotnet-sdk-9.0` for runtime. The SDK is needed at runtime for `dotnet restore` on mounted projects. `DOTNET_ROOT=/usr/lib64/dotnet` for RHEL's SDK path.

Runs as user 1001. Source projects should be mounted via a podman volume (not a direct bind mount) so the user has write access for `dotnet restore`'s `obj/` output. See the Makefile's `run-c-sharp-pod` target for the pattern.

## Other RPCs

- **`GetCodeSnip`** - reads a file from disk, returns lines around a target location with context padding
- **`Stop`** - clears `ProjectStateHolder` (drops compilation from memory)
- **`Capabilities`** - returns `["referenced"]`
- **`GetDependencies` / `GetDependenciesDAG`** - stubs (empty success)
