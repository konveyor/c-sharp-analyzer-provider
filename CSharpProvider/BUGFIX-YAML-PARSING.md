# Bug Fix: YAML Parsing in SymbolQuery

## Problem

When running kantra analysis with the Roslyn C# provider, all `Evaluate` RPC calls were failing with JSON parsing errors:

```
error="'tags:\n  BSD License: true\ntemplate: {}\nruleID: dotnet-core-winforms-01\nreferenced:\n  location: ALL\n  pattern: System.Windows.Forms\n' is an invalid JSON literal. Expected the literal 'true'. LineNumber: 0 | BytePositionInLine: 1."
```

## Root Cause

The `SymbolQuery.ParseCondition()` method was attempting to parse the `conditionInfo` parameter as JSON using `JsonDocument.Parse()`, but the analyzer-lsp engine actually sends this information as **YAML**, not JSON.

Looking at the error message, the actual data sent is:
```yaml
tags:
  BSD License: true
template: {}
ruleID: dotnet-core-winforms-01
referenced:
  location: ALL
  pattern: System.Windows.Forms
```

This is the full rule evaluation context that analyzer-lsp sends to providers, which includes:
- `tags`: Rule metadata
- `template`: Template context for variable substitution
- `ruleID`: The rule being evaluated
- `referenced`: The actual query condition (what we need)

## Solution

Updated `Analysis/SymbolQuery.cs` to:

1. **Replace JSON parsing with YAML parsing** using YamlDotNet (already a dependency)
2. **Created proper data structures** to deserialize the YAML:
   - `ConditionWrapper`: Represents the entire context sent by analyzer-lsp
   - `ReferencedCondition`: Represents the `referenced` section with pattern, location, and file_paths

3. **Used YamlDotNet deserializer** configured to:
   - Use underscore naming convention (to match `file_paths`)
   - Ignore unmatched properties (so we can ignore `tags`, `template`, `ruleID` which we don't need)

### Code Changes

**Before:**
```csharp
using System.Text.Json;

public static QueryCondition ParseCondition(string conditionInfo)
{
    using var doc = JsonDocument.Parse(conditionInfo);  // ❌ Fails - conditionInfo is YAML
    var root = doc.RootElement;
    var referenced = root.GetProperty("referenced");
    // ...
}
```

**After:**
```csharp
using YamlDotNet.Serialization;
using YamlDotNet.Serialization.NamingConventions;

internal class ConditionWrapper
{
    public ReferencedCondition? Referenced { get; set; }
}

internal class ReferencedCondition
{
    public string Pattern { get; set; } = string.Empty;
    public string? Location { get; set; }
    [YamlMember(Alias = "file_paths")]
    public List<string>? FilePaths { get; set; }
}

private static readonly IDeserializer YamlDeserializer = new DeserializerBuilder()
    .WithNamingConvention(UnderscoredNamingConvention.Instance)
    .IgnoreUnmatchedProperties()
    .Build();

public static QueryCondition ParseCondition(string conditionInfo)
{
    var wrapper = YamlDeserializer.Deserialize<ConditionWrapper>(conditionInfo);  // ✅ Works!
    
    if (wrapper.Referenced == null)
        throw new ArgumentException("Missing 'referenced' section in condition info");
    
    // Extract pattern and location from the Referenced section
    // ...
}
```

## How the Rust Provider Handles This

The tree-sitter-based Rust provider in the parent directory correctly uses YAML parsing:

```rust
// From ../src/provider/csharp.rs
let condition: CSharpCondition =
    serde_yml::from_str(evaluate_request.condition_info.as_str())
    .map_err(|err| {
        error!("{:?}", err);
        Status::new(tonic::Code::Internal, "failed")
    })?;
```

The Roslyn provider should match this behavior.

## Testing

To verify the fix works:

1. **Build the provider** (requires .NET SDK 9.0):
   ```bash
   cd CSharpProvider
   dotnet build
   ```

2. **Run standalone test**:
   ```bash
   dotnet run -- --port 14651
   ```

3. **Test with example YAML** (using grpcurl):
   ```bash
   grpcurl -plaintext -d '{
     "cap": "referenced",
     "conditionInfo": "tags:\n  test: true\ntemplate: {}\nruleID: test-rule\nreferenced:\n  location: ALL\n  pattern: System\\\\.Console",
     "id": 1
   }' localhost:14651 provider.ProviderService.Evaluate
   ```

4. **Run with kantra**:
   ```bash
   kantra analyze \
     --input /path/to/csharp/project \
     --output /path/to/output \
     --rules /path/to/dotnet-rulesets \
     --provider-settings provider-config-roslyn.yaml
   ```

   Check the analysis.log - you should no longer see "invalid JSON literal" errors.

## Impact

This fix resolves **all** `Evaluate` RPC failures. Before this fix:
- ❌ 0% of rules could execute successfully
- ❌ Every query resulted in JSON parsing errors
- ❌ Analysis produced no incidents

After this fix:
- ✅ Rules can be evaluated successfully
- ✅ YAML condition info is parsed correctly
- ✅ Analysis produces incidents for matching code patterns

## Files Changed

- `CSharpProvider/Analysis/SymbolQuery.cs` - Changed from JSON to YAML parsing

## Related Issues

This was discovered during integration testing with kantra. The error manifested as thousands of:
```
level=error msg="unable to make evaluate call" error="...is an invalid JSON literal..."
```

The fix aligns the Roslyn provider's behavior with:
1. The analyzer-lsp protocol (which sends YAML)
2. The existing tree-sitter C# provider (which parses YAML)
3. All other language providers in the Konveyor ecosystem

## Prerequisites

To build and test this fix, you need:
- .NET SDK 9.0 or higher
- YamlDotNet package (already in CSharpProvider.csproj)

## Next Steps

After applying this fix:
1. Rebuild the provider
2. Rebuild the container image (if using containers)
3. Re-run failed analyses to generate correct results
