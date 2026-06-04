# Bug Fix: Code Snippet Formatting

## Problem

When viewing analysis results in the Konveyor frontend, code snippets were showing as **"Code snippet unavailable"** even though the `codeSnip` field was populated in `output.yaml`.

## Root Cause

The `GetCodeSnip` RPC handler in `CodeLocationService.cs` was returning code snippets without line numbers. The analyzer-lsp and its frontend expect code snippets in a specific format where each line is prefixed with its line number.

### Expected Format

```
 36  \t\t\t<id>demo-config</id>
 37  \t\t\t<name>Azure DevOps</name>
 38  \t\t\t<url>https://pkgs.dev.azure.com/...</url>
```

Each line follows the pattern: `{lineNumber,3}  {content}`
- Line number right-aligned to 3 digits
- Two spaces after the line number
- Then the actual line content

### What We Were Returning

```
line content without numbers
more content
even more content
```

This caused the frontend to reject the snippet as invalid and display "Code snippet unavailable".

## Solution

Updated `Services/CodeLocationService.cs` to format each line with its line number:

### Before
```csharp
var snippet = string.Join("\n",
    lines.Skip(contextStart).Take(contextEnd - contextStart + 1));
```

### After
```csharp
var snippetLines = new List<string>();
for (var i = contextStart; i <= contextEnd; i++)
{
    // Line numbers are 1-based for display, right-aligned to 3 digits
    var lineNum = i + 1;
    var lineContent = i < lines.Length ? lines[i] : "";
    snippetLines.Add($"{lineNum,3}  {lineContent}");
}

var snippet = string.Join("\n", snippetLines);
```

## Format Specification

The line number formatting uses C#'s string interpolation with alignment:
- `{lineNum,3}` - Right-aligns the line number to 3 characters
- Two spaces separate the number from content
- Line numbers are **1-based** (first line is `  1`, not `  0`)

Examples:
```
  1  first line          // Single digit with 2 leading spaces
 42  middle line         // Two digits with 1 leading space
100  hundredth line      // Three digits with 0 leading spaces
```

## Testing

The fix can be verified by:

1. **Unit tests** (see `tests/CodeSnipFormatTest.cs`):
   ```bash
   cd CSharpProvider
   dotnet test --filter CodeSnipFormatTest
   ```

2. **Integration test** with grpcurl:
   ```bash
   # Start provider
   dotnet run -- --port 14651
   
   # In another terminal, first init a project
   grpcurl -plaintext -d '{
     "location": "/path/to/project",
     "analysisMode": "source-only"
   }' localhost:14651 provider.ProviderService.Init
   
   # Then request a code snippet
   grpcurl -plaintext -d '{
     "uri": "file:///path/to/file.cs",
     "codeLocation": {
       "startPosition": {"line": 10, "character": 0},
       "endPosition": {"line": 10, "character": 50}
     }
   }' localhost:14651 provider.ProviderCodeLocationService.GetCodeSnip
   
   # Should return formatted snippet with line numbers:
   # {
   #   "snip": "  1  using System;\n  2  using System.Linq;\n ..."
   # }
   ```

3. **Full analysis** with kantra:
   ```bash
   kantra analyze \
     --input /path/to/csharp/project \
     --output /path/to/output \
     --rules /path/to/rulesets \
     --provider-settings provider-config-roslyn.yaml
   
   # Check output.yaml - snippets should have line numbers:
   cat /path/to/output/output.yaml | grep -A 20 "codeSnip:"
   ```

4. **Frontend verification**:
   - Open the Konveyor UI
   - View an incident with code snippet
   - Should now display the code with line numbers instead of "Code snippet unavailable"

## Impact

**Before the fix:**
- ❌ Frontend showed "Code snippet unavailable"
- ❌ Code snippets in `output.yaml` had no line numbers
- ❌ Difficult to locate issues in source files

**After the fix:**
- ✅ Frontend displays code snippets correctly
- ✅ Snippets include line numbers for easy reference
- ✅ Matches the format used by other providers (Java, Go, etc.)

## Comparison with Other Providers

This fix aligns the Roslyn provider's code snippet format with all other Konveyor providers:

| Provider | Line Number Format | Example |
|----------|-------------------|---------|
| Java | `{lineNum,3}  {content}` | ` 42  public void main()` |
| Go | `{lineNum,3}  {content}` | ` 42  func main() {` |
| Python | `{lineNum,3}  {content}` | ` 42  def main():` |
| **C# (Roslyn)** | `{lineNum,3}  {content}` | ` 42  public void Main()` |

All providers use the same 3-digit right-aligned format for consistency.

## Files Changed

- `CSharpProvider/Services/CodeLocationService.cs` - Added line number formatting
- `CSharpProvider/tests/CodeSnipFormatTest.cs` - Added unit tests for the fix

## Related Code

The `contextLines` configuration (default: 10) controls how many lines of context to include before and after the incident location:

```csharp
var contextStart = Math.Max(0, startLine - _config.ContextLines);
var contextEnd = Math.Min(lines.Length - 1, endLine + _config.ContextLines);
```

This can be configured via command-line:
```bash
dotnet run -- --port 14651 --context-lines 15
```

Or via provider configuration:
```yaml
initConfig:
  - location: /path/to/project
    analysisMode: source-only
    providerSpecificConfig:
      contextLines: 15
```

## Next Steps

After applying this fix:
1. Rebuild the provider
2. Rebuild the container image (if using containers)
3. Re-run any failed analyses
4. Verify code snippets display correctly in the Konveyor UI

The combination of the YAML parsing fix (BUGFIX-YAML-PARSING.md) and this code snippet formatting fix makes the Roslyn provider fully compatible with the Konveyor analysis workflow.
