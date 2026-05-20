using Google.Protobuf.WellKnownTypes;
using Provider;

namespace CSharpProvider.Analysis;

public static class ResultBuilder
{
    public static List<QueryResult> Deduplicate(List<QueryResult> results)
    {
        // Group by (fileUri, lineNumber), keep the result with the smallest span
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
            if (r.StartChar > 0) startPos.Character = r.StartChar;

            var endPos = new Position { Line = r.EndLine, Character = r.EndChar };

            return new IncidentContext
            {
                FileURI = r.FileUri,
                CodeLocation = new Location
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
