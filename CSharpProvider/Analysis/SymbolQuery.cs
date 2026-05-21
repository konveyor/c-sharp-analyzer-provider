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
            if (node.Parent is InvocationExpressionSyntax invocation && invocation.Expression == node)
            {
                base.VisitMemberAccessExpression(node);
                return;
            }
            TryResolveAndMatch(node);
            base.VisitMemberAccessExpression(node);
        }

        public override void VisitInvocationExpression(InvocationExpressionSyntax node)
        {
            if (node.Expression is MemberAccessExpressionSyntax memberAccess)
            {
                TryResolveAndMatch(memberAccess, isInvocation: true);
            }
            base.VisitInvocationExpression(node);
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

                if (node.BaseList != null)
                {
                    foreach (var baseType in node.BaseList.Types)
                        TryMatchType(baseType.Type, baseType, "type_reference");
                }
            }
            base.VisitClassDeclaration(node);
        }

        public override void VisitStructDeclaration(StructDeclarationSyntax node)
        {
            if (node.BaseList != null)
            {
                foreach (var baseType in node.BaseList.Types)
                    TryMatchType(baseType.Type, baseType, "type_reference");
            }
            base.VisitStructDeclaration(node);
        }

        public override void VisitInterfaceDeclaration(InterfaceDeclarationSyntax node)
        {
            if (node.BaseList != null)
            {
                foreach (var baseType in node.BaseList.Types)
                    TryMatchType(baseType.Type, baseType, "type_reference");
            }
            base.VisitInterfaceDeclaration(node);
        }

        public override void VisitObjectCreationExpression(ObjectCreationExpressionSyntax node)
        {
            TryMatchType(node.Type, node, "object_creation");
            base.VisitObjectCreationExpression(node);
        }

        public override void VisitVariableDeclaration(VariableDeclarationSyntax node)
        {
            if (node.Type is not PredefinedTypeSyntax)
                TryMatchType(node.Type, node.Type, "type_reference");
            base.VisitVariableDeclaration(node);
        }

        public override void VisitPropertyDeclaration(PropertyDeclarationSyntax node)
        {
            TryMatchType(node.Type, node.Type, "type_reference");
            base.VisitPropertyDeclaration(node);
        }

        public override void VisitFieldDeclaration(FieldDeclarationSyntax node)
        {
            TryMatchType(node.Declaration.Type, node.Declaration.Type, "type_reference");
            base.VisitFieldDeclaration(node);
        }

        public override void VisitMethodDeclaration(MethodDeclarationSyntax node)
        {
            TryMatchType(node.ReturnType, node.ReturnType, "type_reference");
            base.VisitMethodDeclaration(node);
        }

        public override void VisitParameter(ParameterSyntax node)
        {
            TryMatchType(node.Type, node, "type_reference");
            base.VisitParameter(node);
        }

        public override void VisitCastExpression(CastExpressionSyntax node)
        {
            TryMatchType(node.Type, node, "type_reference");
            base.VisitCastExpression(node);
        }

        public override void VisitIsPatternExpression(IsPatternExpressionSyntax node)
        {
            if (node.Pattern is DeclarationPatternSyntax declPattern)
                TryMatchType(declPattern.Type, node, "type_reference");
            else if (node.Pattern is TypePatternSyntax typePattern)
                TryMatchType(typePattern.Type, node, "type_reference");
            base.VisitIsPatternExpression(node);
        }

        public override void VisitTypeOfExpression(TypeOfExpressionSyntax node)
        {
            TryMatchType(node.Type, node, "type_reference");
            base.VisitTypeOfExpression(node);
        }

        public override void VisitAttribute(AttributeSyntax node)
        {
            var symbol = _model.GetSymbolInfo(node).Symbol;
            if (symbol != null)
            {
                var containingType = symbol.ContainingType;
                if (containingType != null)
                {
                    var fqdn = GetTypeFqdn(containingType);
                    if (_query.Pattern.IsMatch(fqdn))
                    {
                        AddResult(node, containingType.Name, "annotation",
                            fqdnNamespace: containingType.ContainingNamespace?.ToDisplayString(),
                            fqdnClass: containingType.Name);
                    }
                }
            }
            base.VisitAttribute(node);
        }

        public override void VisitIdentifierName(IdentifierNameSyntax node)
        {
            if (node.Parent is MemberAccessExpressionSyntax ma && ma.Name == node)
            {
                base.VisitIdentifierName(node);
                return;
            }

            var symbol = _model.GetSymbolInfo(node).Symbol;
            if (symbol is null or ILocalSymbol or IParameterSymbol or INamespaceSymbol or INamedTypeSymbol
                || symbol.ContainingType == null)
            {
                base.VisitIdentifierName(node);
                return;
            }

            var enclosingSymbol = _model.GetEnclosingSymbol(node.SpanStart);
            var enclosingType = (enclosingSymbol?.ContainingType ?? enclosingSymbol as INamedTypeSymbol) as ITypeSymbol;

            if (MatchesFqdnInHierarchy(symbol, enclosingType) && MatchesLocationType(symbol))
            {
                string syntaxType = symbol switch
                {
                    IMethodSymbol => "method_reference",
                    _ => "field_reference",
                };
                AddResult(node, symbol.Name, syntaxType,
                    fqdnNamespace: symbol.ContainingNamespace?.ToDisplayString(),
                    fqdnClass: symbol.ContainingType.Name,
                    fqdnMethod: symbol is IMethodSymbol ? symbol.Name : null,
                    fqdnField: symbol is IFieldSymbol or IPropertySymbol ? symbol.Name : null);
            }
            base.VisitIdentifierName(node);
        }

        public override void VisitArgument(ArgumentSyntax node)
        {
            TryMatchExpressionType(node.Expression, node, "type_usage");
            base.VisitArgument(node);
        }

        public override void VisitReturnStatement(ReturnStatementSyntax node)
        {
            TryMatchExpressionType(node.Expression, node, "type_usage");
            base.VisitReturnStatement(node);
        }

        public override void VisitAssignmentExpression(AssignmentExpressionSyntax node)
        {
            TryMatchExpressionType(node.Right, node, "type_usage");
            base.VisitAssignmentExpression(node);
        }

        private void TryResolveAndMatch(MemberAccessExpressionSyntax node, bool isInvocation = false)
        {
            var symbolInfo = _model.GetSymbolInfo(node);
            var symbol = symbolInfo.Symbol;

            if (symbol != null)
            {
                MatchResolvedSymbol(node, symbol, isInvocation);
                return;
            }

            TryMatchDynamic(node);
        }

        private void TryMatchDynamic(MemberAccessExpressionSyntax node)
        {
            var expr = node.Expression;
            while (expr is MemberAccessExpressionSyntax nested)
            {
                var nestedSymbol = _model.GetSymbolInfo(nested).Symbol;
                if (nestedSymbol != null)
                {
                    var typeInfo = _model.GetTypeInfo(nested);
                    if (typeInfo.Type?.TypeKind == TypeKind.Dynamic)
                    {
                        MatchDynamicAccess(node, nestedSymbol);
                    }
                    return;
                }
                expr = nested.Expression;
            }

            var exprSymbol = _model.GetSymbolInfo(expr).Symbol;
            if (exprSymbol == null)
                return;

            var exprType = _model.GetTypeInfo(expr);
            if (exprType.Type?.TypeKind == TypeKind.Dynamic)
            {
                MatchDynamicAccess(node, exprSymbol);
            }
        }

        private void MatchDynamicAccess(MemberAccessExpressionSyntax node, ISymbol resolvedSymbol)
        {
            var baseFqdn = GetFqdn(resolvedSymbol);
            var dynamicSuffix = GetDynamicSuffix(node, resolvedSymbol);
            var fqdn = string.IsNullOrEmpty(dynamicSuffix)
                ? baseFqdn
                : $"{baseFqdn}.{dynamicSuffix}";

            if (!_query.Pattern.IsMatch(fqdn))
                return;

            if (!MatchesLocationType(resolvedSymbol))
                return;

            string? fqdnNs = resolvedSymbol.ContainingNamespace?.ToDisplayString();
            string? fqdnClass = resolvedSymbol.ContainingType?.Name;

            AddResult(node, node.ToString(), "field_reference",
                fqdnNs, fqdnClass, fqdnField: resolvedSymbol.Name);
        }

        private static string GetDynamicSuffix(MemberAccessExpressionSyntax node, ISymbol resolvedSymbol)
        {
            var fullText = node.ToString();
            var resolvedName = resolvedSymbol.Name;
            var idx = fullText.IndexOf(resolvedName, StringComparison.Ordinal);
            if (idx < 0)
                return "";
            var after = fullText[(idx + resolvedName.Length)..];
            return after.TrimStart('.');
        }

        private void MatchResolvedSymbol(
            MemberAccessExpressionSyntax node, ISymbol symbol, bool isInvocation)
        {
            var accessType = _model.GetTypeInfo(node.Expression).Type;
            if (!MatchesFqdnInHierarchy(symbol, accessType))
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

        private void TryMatchType(TypeSyntax? typeSyntax, SyntaxNode reportNode, string syntaxTypeLabel)
        {
            if (typeSyntax == null)
                return;

            var typeInfo = _model.GetTypeInfo(typeSyntax);
            if (typeInfo.Type is not INamedTypeSymbol namedType)
                return;

            var fqdn = GetTypeFqdn(namedType);
            if (_query.Pattern.IsMatch(fqdn))
            {
                AddResult(reportNode, namedType.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat),
                    syntaxTypeLabel,
                    fqdnNamespace: namedType.ContainingNamespace?.ToDisplayString(),
                    fqdnClass: namedType.Name);
            }

            foreach (var typeArg in namedType.TypeArguments)
            {
                if (typeArg is INamedTypeSymbol argType)
                {
                    var argFqdn = GetTypeFqdn(argType);
                    if (_query.Pattern.IsMatch(argFqdn))
                    {
                        AddResult(reportNode, argType.ToDisplayString(SymbolDisplayFormat.MinimallyQualifiedFormat),
                            syntaxTypeLabel,
                            fqdnNamespace: argType.ContainingNamespace?.ToDisplayString(),
                            fqdnClass: argType.Name);
                    }
                }
            }
        }

        private void TryMatchExpressionType(ExpressionSyntax? expr, SyntaxNode reportNode, string syntaxTypeLabel)
        {
            if (expr == null)
                return;

            var typeInfo = _model.GetTypeInfo(expr);
            var type = typeInfo.Type;
            if (type == null || type.TypeKind == TypeKind.Error)
                return;

            var current = type;
            while (current != null)
            {
                var fqdn = GetTypeFqdn(current);
                if (_query.Pattern.IsMatch(fqdn))
                {
                    AddResult(reportNode, expr.ToString(), syntaxTypeLabel,
                        fqdnNamespace: current.ContainingNamespace?.ToDisplayString(),
                        fqdnClass: current.Name);
                    return;
                }
                current = current.BaseType;
            }

            foreach (var iface in type.AllInterfaces)
            {
                var fqdn = GetTypeFqdn(iface);
                if (_query.Pattern.IsMatch(fqdn))
                {
                    AddResult(reportNode, expr.ToString(), syntaxTypeLabel,
                        fqdnNamespace: iface.ContainingNamespace?.ToDisplayString(),
                        fqdnClass: iface.Name);
                    return;
                }
            }
        }

        private static string GetTypeFqdn(ITypeSymbol type)
        {
            var ns = type.ContainingNamespace;
            if (ns != null && !ns.IsGlobalNamespace)
                return $"{ns.ToDisplayString()}.{type.Name}";
            return type.Name;
        }

        private bool MatchesFqdnInHierarchy(ISymbol symbol, ITypeSymbol? accessType)
        {
            var fqdn = GetFqdn(symbol);
            if (_query.Pattern.IsMatch(fqdn))
                return true;

            if (accessType == null || symbol.ContainingType == null)
                return false;

            var declaringType = symbol.ContainingType;

            var check = accessType;
            bool inHierarchy = false;
            while (check != null)
            {
                if (SymbolEqualityComparer.Default.Equals(check, declaringType))
                {
                    inHierarchy = true;
                    break;
                }
                check = check.BaseType;
            }
            if (!inHierarchy)
                return false;

            var current = accessType;
            while (current != null && !SymbolEqualityComparer.Default.Equals(current, declaringType))
            {
                var ns = current.ContainingNamespace;
                var altFqdn = (ns != null && !ns.IsGlobalNamespace)
                    ? $"{ns.ToDisplayString()}.{current.Name}.{symbol.Name}"
                    : $"{current.Name}.{symbol.Name}";
                if (_query.Pattern.IsMatch(altFqdn))
                    return true;
                current = current.BaseType;
            }

            return false;
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
            .GroupBy(r => (r.FileUri, r.StartLine, r.StartChar, r.EndLine, r.EndChar))
            .Select(g => g.First())
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
