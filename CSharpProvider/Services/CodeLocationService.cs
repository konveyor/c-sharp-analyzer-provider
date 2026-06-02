using Grpc.Core;
using Provider;

namespace CSharpProvider.Services;

// Provider.ProviderCodeLocationService.ProviderCodeLocationServiceBase is the
// base class generated from provider.proto.
public class CodeLocationService : ProviderCodeLocationService.ProviderCodeLocationServiceBase
{
    private readonly ProviderConfig _config;
    private readonly ILogger<CodeLocationService> _logger;

    public CodeLocationService(ProviderConfig config, ILogger<CodeLocationService> logger)
    {
        _config = config;
        _logger = logger;
    }

    public override Task<GetCodeSnipResponse> GetCodeSnip(GetCodeSnipRequest request, ServerCallContext context)
    {
        try
        {
            var filePath = request.Uri.StartsWith("file://")
                ? new Uri(request.Uri).LocalPath
                : request.Uri;

            if (!File.Exists(filePath))
            {
                return Task.FromResult(new GetCodeSnipResponse { Snip = "" });
            }

            var lines = File.ReadAllLines(filePath);
            var startLine = (int)request.CodeLocation.StartPosition.Line;
            var endLine = (int)request.CodeLocation.EndPosition.Line;

            var contextStart = Math.Max(0, startLine - _config.ContextLines);
            var contextEnd = Math.Min(lines.Length - 1, endLine + _config.ContextLines);

            // Format snippet with line numbers like analyzer-lsp expects:
            // Each line is: "{lineNum,3}  {content}\n"
            // Example: " 42  some code here\n"
            var snippetLines = new List<string>();
            for (var i = contextStart; i <= contextEnd; i++)
            {
                // Line numbers are 1-based for display, right-aligned to 3 digits
                var lineNum = i + 1;
                var lineContent = i < lines.Length ? lines[i] : "";
                snippetLines.Add($"{lineNum,3}  {lineContent}");
            }

            var snippet = string.Join("\n", snippetLines);

            return Task.FromResult(new GetCodeSnipResponse { Snip = snippet });
        }
        catch (Exception ex)
        {
            _logger.LogError(ex, "Failed to get code snippet for {Uri}", request.Uri);
            return Task.FromResult(new GetCodeSnipResponse { Snip = "" });
        }
    }
}
