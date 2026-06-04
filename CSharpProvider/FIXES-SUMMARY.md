# Roslyn C# Provider - Bug Fixes Summary

This document summarizes the critical bug fixes applied to make the Roslyn-based C# provider fully functional with Konveyor's analyzer-lsp.

## Overview

Two critical bugs prevented the provider from working correctly with kantra:
1. **YAML Parsing Error** - Provider couldn't parse condition info
2. **Code Snippet Format** - Frontend couldn't display code snippets

Both issues have been fixed and documented.

---

## Bug #1: YAML Parsing in Evaluate RPC

### Symptom
Every `Evaluate` RPC call failed with JSON parsing errors:
```
error="'tags:\n  BSD License: true\n...' is an invalid JSON literal."
```

### Root Cause
The provider was using `JsonDocument.Parse()` but analyzer-lsp sends condition info as **YAML**, not JSON.

### Fix Location
- **File**: `CSharpProvider/Analysis/SymbolQuery.cs`
- **Change**: Replaced `System.Text.Json` with `YamlDotNet` deserialization

### Details
See [BUGFIX-YAML-PARSING.md](BUGFIX-YAML-PARSING.md) for complete details.

### Impact
- ✅ All evaluate queries now work
- ✅ Rules can be executed successfully
- ✅ Analysis generates incidents

---

## Bug #2: Code Snippet Formatting

### Symptom
Frontend showed "Code snippet unavailable" even though snippets were present in `output.yaml`.

### Root Cause
Code snippets were missing line numbers. The analyzer-lsp expects each line formatted as:
```
{lineNumber,3}  {content}
```

### Fix Location
- **File**: `CSharpProvider/Services/CodeLocationService.cs`
- **Change**: Added line number prefix to each line in code snippets

### Details
See [BUGFIX-CODE-SNIPPET-FORMAT.md](BUGFIX-CODE-SNIPPET-FORMAT.md) for complete details.

### Impact
- ✅ Frontend displays code snippets correctly
- ✅ Line numbers visible for easy source navigation
- ✅ Matches format used by other providers

---

## Testing the Fixes

### Quick Test

```bash
# 1. Rebuild
cd CSharpProvider
dotnet build -c Release

# 2. Run provider
dotnet run -- --port 14651

# 3. In another terminal, test with kantra
kantra analyze \
  --input /path/to/csharp/project \
  --output /path/to/output \
  --rules /path/to/dotnet-rulesets \
  --provider-settings provider-config-roslyn.yaml

# 4. Check the output
cat /path/to/output/output.yaml | grep -A 10 "incidents:"
```

### Expected Results

**output.yaml should contain:**
```yaml
violations:
  dotnet-core-mvc-controller-01:
    incidents:
    - uri: file:///path/to/Controller.cs
      message: ASP.NET MVC Controller detected
      codeSnip: |
         12  using System.Web.Mvc;
         13  
         14  public class HomeController : Controller
         15  {
         16      public ActionResult Index()
      lineNumber: 14
```

**Analysis log should NOT contain:**
```
# These errors should be gone:
❌ "is an invalid JSON literal"
❌ "unable to make evaluate call"
```

---

## Files Changed

### Core Fixes
1. `CSharpProvider/Analysis/SymbolQuery.cs`
   - Changed from JSON to YAML parsing
   - Added data structures for YAML deserialization

2. `CSharpProvider/Services/CodeLocationService.cs`
   - Added line number formatting to code snippets

### Documentation
1. `BUGFIX-YAML-PARSING.md` - Detailed YAML parsing fix
2. `BUGFIX-CODE-SNIPPET-FORMAT.md` - Detailed snippet formatting fix
3. `FIXES-SUMMARY.md` - This summary document

### Tests
1. `CSharpProvider/tests/CodeSnipFormatTest.cs` - Unit tests for snippet formatting

---

## Prerequisites for Building

⚠️ **Important**: You need .NET SDK 9.0 or higher

```bash
# Check version
dotnet --version

# Should show 9.0.x or higher
# If not, download from: https://dotnet.microsoft.com/download/dotnet/9.0
```

---

## Container Deployment

If you're using containers, rebuild the image after applying these fixes:

```bash
# Build container
./scripts/build-container.sh

# Or manually
cd CSharpProvider
podman build -f Containerfile -t quay.io/konveyor/c-sharp-roslyn-provider:latest .

# Test container
./scripts/test-container.sh
```

The container image includes .NET SDK 9.0, so it will build correctly.

---

## Before vs. After

### Before Fixes
```
Analysis Results:
❌ 0 incidents found
❌ All evaluate calls failed with JSON errors
❌ Code snippets showed as "unavailable"
❌ Analysis log filled with errors
```

### After Fixes
```
Analysis Results:
✅ Incidents found and reported correctly
✅ Evaluate calls succeed
✅ Code snippets display with line numbers
✅ Clean analysis log (normal info/debug only)
```

---

## Integration Status

The Roslyn C# provider is now **fully compatible** with:
- ✅ analyzer-lsp protocol (YAML-based condition info)
- ✅ kantra CLI tool
- ✅ Konveyor UI (code snippet display)
- ✅ Standard .NET analysis rulesets

---

## Known Limitations

These are not bugs, just features not yet implemented:

1. **Dependency Location Service**
   - `ProviderDependencyLocationService` not implemented
   - Doesn't affect basic analysis
   - Can be added later if needed

2. **Actual Dependency Extraction**
   - `GetDependencies()` returns empty list
   - Doesn't affect code pattern matching
   - Would enable dependency-based rules

These limitations don't prevent the provider from working for code analysis rules (which is the primary use case).

---

## Support

If you encounter issues:

1. Check prerequisites (✅ .NET SDK 9.0+)
2. Review the detailed fix documents:
   - [BUGFIX-YAML-PARSING.md](BUGFIX-YAML-PARSING.md)
   - [BUGFIX-CODE-SNIPPET-FORMAT.md](BUGFIX-CODE-SNIPPET-FORMAT.md)
3. Check logs with `--log-file /tmp/provider.log`
4. See [INTEGRATION.md](INTEGRATION.md) for troubleshooting

---

## Contributing

When contributing fixes or enhancements:

1. Ensure compatibility with analyzer-lsp protocol
2. Match the behavior of other language providers
3. Add unit tests for new functionality
4. Update documentation

The Roslyn provider should behave identically to the tree-sitter provider from the analyzer-lsp perspective, even though it uses a different analysis engine internally.

---

## Version History

- **v1.1** (2026-06-02)
  - ✅ Fixed YAML parsing in SymbolQuery
  - ✅ Fixed code snippet formatting with line numbers
  - ✅ Full compatibility with analyzer-lsp

- **v1.0** (Initial)
  - ❌ JSON parsing (incorrect)
  - ❌ Plain text snippets (missing line numbers)
  - ❌ Not compatible with analyzer-lsp
