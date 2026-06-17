#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HACK_DIR="$SCRIPT_DIR"

PORT="${PORT:-9876}"
PROJECT="${PROJECT:-$REPO_ROOT/tests/repos/nerd-dinner/mvc4}"
RULES_SRC="${RULES:-$REPO_ROOT/rulesets/dotnet-core-migration}"
RULES_DIR="${RULES_DIR:-$HACK_DIR/rules}"
OUTPUT="${OUTPUT:-$HACK_DIR/output.yaml}"
FIX_GLOBS="${FIX_GLOBS:-1}"

ANALYZER_LSP_DIR="${ANALYZER_LSP_DIR:?Set ANALYZER_LSP_DIR to the path of your analyzer-lsp checkout}"
ANALYZER_LSP_BIN="${ANALYZER_LSP_BIN:-$HACK_DIR/analyzer-lsp}"

PROVIDER_SETTINGS="$HACK_DIR/provider_settings.json"
PROVIDER_PID=""

cleanup() {
    if [[ -n "$PROVIDER_PID" ]]; then
        echo "Stopping C# provider (PID $PROVIDER_PID)..."
        kill "$PROVIDER_PID" 2>/dev/null || true
        wait "$PROVIDER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Build analyzer-lsp ---
if [[ ! -f "$ANALYZER_LSP_BIN" ]]; then
    echo "Building analyzer-lsp from $ANALYZER_LSP_DIR..."
    go build -C "$ANALYZER_LSP_DIR" -o "$ANALYZER_LSP_BIN" ./cmd/analyzer/
    echo "Built: $ANALYZER_LSP_BIN"
else
    echo "Using cached analyzer-lsp binary: $ANALYZER_LSP_BIN"
fi

# --- Clone rulesets ---
rm -rf "$RULES_DIR"
cp -r "$RULES_SRC" "$RULES_DIR"
if [[ "$FIX_GLOBS" == "1" ]]; then
    sed -i -E '/pattern:/{s/\./\\./g; s/\*/.*/g;}' "$RULES_DIR"/*.yaml
    echo "Cloned rulesets to: $RULES_DIR (glob patterns converted to regex)"
else
    echo "Cloned rulesets to: $RULES_DIR (patterns unchanged)"
fi

# --- Validate project exists ---
if [[ ! -d "$PROJECT" ]]; then
    echo "ERROR: Project directory not found: $PROJECT"
    echo "Run: uv run tests/test_runner.py setup"
    exit 1
fi

# --- Generate provider_settings.json ---
PROJECT_ABS="$(cd "$PROJECT" && pwd)"

cat > "$PROVIDER_SETTINGS" <<EOF
[
    {
        "name": "csharp",
        "address": "127.0.0.1:$PORT",
        "initConfig": [
            {
                "analysisMode": "source-only",
                "location": "$PROJECT_ABS"
            }
        ]
    }
]
EOF
echo "Generated provider settings: $PROVIDER_SETTINGS"

# --- Start C# provider ---
echo "Starting C# provider on port $PORT..."
dotnet run --project "$REPO_ROOT/src" -- --port "$PORT" &
PROVIDER_PID=$!

echo "Waiting for provider (PID $PROVIDER_PID) to listen on port $PORT..."
for i in $(seq 1 60); do
    if curl -s -o /dev/null "http://127.0.0.1:$PORT" 2>/dev/null || \
       bash -c "echo > /dev/tcp/127.0.0.1/$PORT" 2>/dev/null; then
        echo "Provider is ready."
        break
    fi
    if ! kill -0 "$PROVIDER_PID" 2>/dev/null; then
        echo "ERROR: Provider process died."
        exit 1
    fi
    sleep 1
done

# --- Run analyzer-lsp ---
echo ""
echo "Running analyzer-lsp..."
echo "  Provider settings: $PROVIDER_SETTINGS"
echo "  Rules: $RULES_DIR"
echo "  Project: $PROJECT_ABS"
echo "  Output: $OUTPUT"
echo ""

"$ANALYZER_LSP_BIN" \
    --provider-settings "$PROVIDER_SETTINGS" \
    --rules "$RULES_DIR" \
    --output-file "$OUTPUT" \
    --context-lines 0 \
    --analysis-mode source-only

echo ""
echo "Analysis complete. Output: $OUTPUT"

if [[ -f "$OUTPUT" ]]; then
    violations=$(grep -c "ruleID:" "$OUTPUT" 2>/dev/null || echo "0")
    echo "Output file size: $(wc -c < "$OUTPUT") bytes"
fi
