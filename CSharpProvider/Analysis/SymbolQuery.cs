using System.Text.Json;
using System.Text.RegularExpressions;
using Google.Protobuf.WellKnownTypes;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Provider;

namespace CSharpProvider.Analysis;

public enum LocationType
{
    All,
    Method,
    Field,
    Class
}

public record QueryCondition(Regex Pattern, LocationType Location, List<string>? FilePaths);

public record QueryResult(
    string FileUri,
    int StartLine,
    int StartChar,
    int EndLine,
    int EndChar,
    string Symbol,
    string SyntaxType,
    string? FqdnNamespace,
    string? FqdnClass,
    string? FqdnMethod,
    string? FqdnField);

public static class SymbolQuery
{
    public static QueryCondition ParseCondition(string conditionInfo)
    {
        using var doc = JsonDocument.Parse(conditionInfo);
        var root = doc.RootElement;
        var referenced = root.GetProperty("referenced");

        var pattern = referenced.GetProperty("pattern").GetString()
            ?? throw new ArgumentException("Missing pattern");

        var location = LocationType.All;
        if (referenced.TryGetProperty("location", out var locProp))
        {
            var locStr = locProp.GetString()?.ToUpperInvariant();
            location = locStr switch
            {
                "METHOD" => LocationType.Method,
                "FIELD" => LocationType.Field,
                "CLASS" => LocationType.Class,
                _ => LocationType.All,
            };
        }

        List<string>? filePaths = null;
        if (referenced.TryGetProperty("file_paths", out var fpProp) && fpProp.ValueKind == JsonValueKind.Array)
        {
            filePaths = fpProp.EnumerateArray()
                .Select(e => e.GetString()!)
                .Where(s => !string.IsNullOrEmpty(s))
                .ToList();
            if (filePaths.Count == 0)
                filePaths = null;
        }

        return new QueryCondition(new Regex(pattern), location, filePaths);
    }

    public static List<QueryResult> Execute(
        CSharpCompilation compilation, string projectPath, QueryCondition query)
    {
        var results = new List<QueryResult>();

        foreach (var tree in compilation.SyntaxTrees)
        {
            if (string.IsNullOrEmpty(tree.FilePath))
                continue;

            if (query.FilePaths != null && !query.FilePaths.Any(fp =>
                tree.FilePath.Contains(fp, StringComparison.OrdinalIgnoreCase)))
                continue;

            var semanticModel = compilation.GetSemanticModel(tree);
            var root = tree.GetRoot();
            var walker = new SymbolWalker(semanticModel, tree, projectPath, query, results);
            walker.Visit(root);
        }

        return results;
    }

    private class SymbolWalker : CSharpSyntaxWalker
    {
        private readonly SemanticModel _model;
        private readonly SyntaxTree _tree;
        private readonly string _projectPath;
        private readonly QueryCondition _query;
        private readonly List<QueryResult> _results;

        public SymbolWalker(
            SemanticModel model, SyntaxTree tree, string projectPath,
            QueryCondition query, List<QueryResult> results)
        {
            _model = model;
            _tree = tree;
            _projectPath = projectPath;
            _query = query;
            _results = results;
        }

        public override void VisitUsingDirective(UsingDirectiveSyntax node)
        {
            if (_query.Location != LocationType.All && _query.Location != LocationType.Class)
            {
                base.VisitUsingDirective(node);
                return;
            }

            var ns = node.Name?.ToString();
            if (ns != null && _query.Pattern.IsMatch(ns))
            {
                AddResult(node.Name!, ns, "import", fqdnNamespace: ns);
            }
            base.VisitUsingDirective(node);
        }

        public override void VisitMemberAccessExpression(MemberAccessExpressionSyntax node)
        {
            TryResolveAndMatch(node);
            base.VisitMemberAccessExpression(node);
        }

        public override void VisitInvocationExpression(InvocationExpressionSyntax node)
        {
            if (node.Expression is MemberAccessExpressionSyntax memberAccess)
            {
                TryResolveAndMatch(memberAccess, isInvocation: true);
            }
            // Don't call base — we handle the member access above
        }

        public override void VisitClassDeclaration(ClassDeclarationSyntax node)
        {
            if (_query.Location == LocationType.All || _query.Location == LocationType.Class)
            {
                var symbol = _model.GetDeclaredSymbol(node);
                if (symbol != null)
                {
                    var fqdn = GetFqdn(symbol);
                    if (_query.Pattern.IsMatch(fqdn) || _query.Pattern.IsMatch(symbol.Name))
                    {
                        AddResult(node, symbol.Name, "class_def",
                            fqdnNamespace: symbol.ContainingNamespace?.ToDisplayString(),
                            fqdnClass: symbol.Name);
                    }
                }
            }
            base.VisitClassDeclaration(node);
        }

        private void TryResolveAndMatch(MemberAccessExpressionSyntax node, bool isInvocation = false)
        {
            var symbolInfo = _model.GetSymbolInfo(node);
            var symbol = symbolInfo.Symbol;

            if (symbol != null)
            {
                MatchResolvedSymbol(node, symbol, isInvocation);
            }
        }

        private void MatchResolvedSymbol(
            MemberAccessExpressionSyntax node, ISymbol symbol, bool isInvocation)
        {
            var fqdn = GetFqdn(symbol);
            if (!_query.Pattern.IsMatch(fqdn))
                return;

            if (!MatchesLocationType(symbol))
                return;

            string syntaxType;
            string? fqdnNs = null, fqdnClass = null, fqdnMethod = null, fqdnField = null;

            switch (symbol)
            {
                case IMethodSymbol ms:
                    syntaxType = "method_reference";
                    fqdnNs = ms.ContainingNamespace?.ToDisplayString();
                    fqdnClass = ms.ContainingType?.Name;
                    fqdnMethod = ms.Name;
                    break;
                case IFieldSymbol fs:
                    syntaxType = "field_reference";
                    fqdnNs = fs.ContainingNamespace?.ToDisplayString();
                    fqdnClass = fs.ContainingType?.Name;
                    fqdnField = fs.Name;
                    break;
                case IPropertySymbol ps:
                    syntaxType = "field_reference";
                    fqdnNs = ps.ContainingNamespace?.ToDisplayString();
                    fqdnClass = ps.ContainingType?.Name;
                    fqdnField = ps.Name;
                    break;
                case INamedTypeSymbol ts:
                    syntaxType = "class_def";
                    fqdnNs = ts.ContainingNamespace?.ToDisplayString();
                    fqdnClass = ts.Name;
                    break;
                default:
                    syntaxType = "field_reference";
                    fqdnNs = symbol.ContainingNamespace?.ToDisplayString();
                    fqdnClass = symbol.ContainingType?.Name;
                    break;
            }

            var displaySymbol = $"{node.Expression}.{node.Name}";
            // Use just the member access part as the symbol name
            var exprStr = node.Expression.ToString();
            var nameStr = node.Name.ToString();
            var parts = exprStr.Split('.');
            var shortSymbol = $"{parts.Last()}.{nameStr}";

            AddResult(node, shortSymbol, syntaxType,
                fqdnNs, fqdnClass, fqdnMethod, fqdnField);
        }

        private bool MatchesLocationType(ISymbol symbol)
        {
            return _query.Location switch
            {
                LocationType.All => true,
                LocationType.Method => symbol is IMethodSymbol,
                LocationType.Field => symbol is IFieldSymbol or IPropertySymbol,
                LocationType.Class => symbol is INamedTypeSymbol,
                _ => true,
            };
        }

        private void AddResult(SyntaxNode node, string symbol, string syntaxType,
            string? fqdnNamespace = null, string? fqdnClass = null,
            string? fqdnMethod = null, string? fqdnField = null)
        {
            var span = _tree.GetLineSpan(node.Span);
            var fileUri = $"file://{_tree.FilePath}";

            _results.Add(new QueryResult(
                FileUri: fileUri,
                StartLine: span.StartLinePosition.Line,
                StartChar: span.StartLinePosition.Character,
                EndLine: span.EndLinePosition.Line,
                EndChar: span.EndLinePosition.Character,
                Symbol: symbol,
                SyntaxType: syntaxType,
                FqdnNamespace: fqdnNamespace,
                FqdnClass: fqdnClass,
                FqdnMethod: fqdnMethod,
                FqdnField: fqdnField));
        }

        private static string GetFqdn(ISymbol symbol)
        {
            var parts = new List<string>();

            var ns = symbol.ContainingNamespace;
            if (ns != null && !ns.IsGlobalNamespace)
                parts.Add(ns.ToDisplayString());

            var type = symbol.ContainingType;
            if (type != null)
                parts.Add(type.Name);

            if (symbol is not INamespaceSymbol)
                parts.Add(symbol.Name);

            return string.Join(".", parts);
        }
    }

    public static List<QueryResult> Deduplicate(List<QueryResult> results)
    {
        return results
            .GroupBy(r => (r.FileUri, r.StartLine))
            .Select(g => g
                .OrderBy(r => (r.EndLine - r.StartLine) * 10000 + (r.EndChar - r.StartChar))
                .First())
            .ToList();
    }

    public static List<IncidentContext> ToIncidentContexts(List<QueryResult> results)
    {
        return results.Select(r =>
        {
            var variables = new Struct();
            variables.Fields["file"] = Value.ForString(r.FileUri);
            variables.Fields["symbol"] = Value.ForString(r.Symbol);
            variables.Fields["syntax_type"] = Value.ForString(r.SyntaxType);

            if (r.FqdnNamespace != null)
                variables.Fields["fqdn_namespace"] = Value.ForString(r.FqdnNamespace);
            if (r.FqdnClass != null)
                variables.Fields["fqdn_class"] = Value.ForString(r.FqdnClass);
            if (r.FqdnMethod != null)
                variables.Fields["fqdn_method"] = Value.ForString(r.FqdnMethod);
            if (r.FqdnField != null)
                variables.Fields["fqdn_field"] = Value.ForString(r.FqdnField);

            var startPos = new Position { Line = r.StartLine };
            if (r.StartChar > 0)
                startPos.Character = r.StartChar;

            var endPos = new Position { Line = r.EndLine, Character = r.EndChar };

            return new IncidentContext
            {
                FileURI = r.FileUri,
                CodeLocation = new Provider.Location
                {
                    StartPosition = startPos,
                    EndPosition = endPos,
                },
                LineNumber = r.StartLine,
                Variables = variables,
            };
        }).ToList();
    }
}
