#!/usr/bin/env bash
# Quick manual test of the C# provider against nerd-dinner.
# Usage: ./testdata/test-queries.sh
set -u

PORT=9876
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOCATION="$REPO_ROOT/testdata/nerd-dinner/mvc4"

summarize() {
    python3 -c "
import json,sys
d=json.load(sys.stdin)
items=d.get('response',{}).get('incidentContexts',[])
print(f'Total: {len(items)}')
types={}
for i in items:
    t=i['variables']['syntax_type']
    types[t]=types.get(t,0)+1
for k,v in sorted(types.items()):
    print(f'  {k}: {v}')
"
}

detail() {
    python3 -c "
import json,sys
d=json.load(sys.stdin)
items=d.get('response',{}).get('incidentContexts',[])
print(f'Total: {len(items)}')
for i in items:
    v=i['variables']
    f=i['fileURI'].split('/')[-1]
    print(f\"  {v['syntax_type']:20s} {v['symbol']:40s} {f}:{i['LineNumber']}\")
"
}

evaluate() {
    local pattern="$1"
    local mode="${2:-summary}"
    local escaped
    escaped=$(echo "$pattern" | sed 's/\\/\\\\/g; s/"/\\"/g')
    local condition="{\"referenced\": {\"pattern\": \"$escaped\"}}"
    local data="{\"cap\": \"referenced\", \"id\": \"1\", \"conditionInfo\": $(echo "$condition" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')}"

    if [ "$mode" = "detail" ]; then
        grpcurl -max-msg-sz 10485760 -plaintext -d "$data" "localhost:$PORT" provider.ProviderService/Evaluate 2>&1 | detail
    else
        grpcurl -max-msg-sz 10485760 -plaintext -d "$data" "localhost:$PORT" provider.ProviderService/Evaluate 2>&1 | summarize
    fi
}

# Build
echo "=== Building ==="
dotnet build "$REPO_ROOT/CSharpProvider/CSharpProvider.csproj" || exit 1

# Start server
echo "=== Starting server on port $PORT ==="
kill $(lsof -ti :"$PORT") 2>/dev/null
sleep 1
dotnet run --project "$REPO_ROOT/CSharpProvider" -- --port "$PORT" > /tmp/csharp-test.log 2>&1 &
SERVER_PID=$!
trap "kill $SERVER_PID 2>/dev/null" EXIT

for i in $(seq 1 60); do
    nc -z localhost "$PORT" 2>/dev/null && break
    sleep 1
done
echo "  Server ready"

# Init
echo "=== Init on nerd-dinner ==="
grpcurl -plaintext -d "{\"location\": \"$LOCATION\"}" "localhost:$PORT" provider.ProviderService/Init

# Queries
echo ""
echo "=== ^System\.Web\.Mvc\.Controller$ (exact) ==="
evaluate '^System\.Web\.Mvc\.Controller$' detail

echo ""
echo "=== ^System\.Web\.Mvc (wildcard) ==="
evaluate '^System\.Web\.Mvc(\..*)?$' summary

echo ""
echo "=== ^System\.Data\.Entity\.DbContext$ (exact) ==="
evaluate '^System\.Data\.Entity\.DbContext$' detail

echo ""
echo "=== ^NerdDinner\..* (internal refs) ==="
evaluate '^NerdDinner\..*' summary

echo ""
echo "=== ^ViewBag\.ReturnUrl$ (dynamic specific) ==="
evaluate '^ViewBag\.ReturnUrl$' detail
echo ""
echo "=== ^System\.Data\.Entity (wildcard) ==="
evaluate '^System\.Data\.Entity(\..*)?$' summary

echo ""
echo "=== Done ==="
