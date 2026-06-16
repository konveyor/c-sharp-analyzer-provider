#!/bin/bash
# Test script for the Roslyn C# provider container

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Default values
IMAGE_NAME="${IMAGE_NAME:-quay.io/konveyor/c-sharp-roslyn-provider}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}"
TEST_PORT="${TEST_PORT:-14651}"

echo "=================================================="
echo "Testing Roslyn C# Provider Container"
echo "=================================================="
echo "Image: $IMAGE_NAME:$IMAGE_TAG"
echo "Port: $TEST_PORT"
echo ""

# Check if grpcurl is available
if ! command -v grpcurl &> /dev/null; then
    echo "WARNING: grpcurl not found. Install it to test gRPC endpoints."
    echo "  See: https://github.com/fullstorydev/grpcurl#installation"
    echo ""
fi

# Start the container in background
echo "Starting container..."
CONTAINER_ID=$($CONTAINER_RUNTIME run -d --rm \
    -p "$TEST_PORT:14651" \
    "$IMAGE_NAME:$IMAGE_TAG")

echo "Container started: $CONTAINER_ID"
echo "Waiting for gRPC server to start..."
sleep 3

# Test connectivity
echo ""
echo "Testing gRPC connectivity..."
if command -v grpcurl &> /dev/null; then
    if grpcurl -plaintext localhost:$TEST_PORT list &> /dev/null; then
        echo "✓ gRPC server is responding"

        echo ""
        echo "Available services:"
        grpcurl -plaintext localhost:$TEST_PORT list

        echo ""
        echo "Testing Capabilities RPC..."
        grpcurl -plaintext localhost:$TEST_PORT provider.ProviderService.Capabilities

    else
        echo "✗ gRPC server not responding"
        echo ""
        echo "Container logs:"
        $CONTAINER_RUNTIME logs "$CONTAINER_ID"
    fi
else
    echo "Skipping gRPC tests (grpcurl not installed)"
    echo "Container is running. You can test manually with:"
    echo "  grpcurl -plaintext localhost:$TEST_PORT list"
fi

echo ""
echo "=================================================="
echo "Press Ctrl+C to stop the container"
echo "Or run: $CONTAINER_RUNTIME stop $CONTAINER_ID"
echo "=================================================="
echo ""

# Keep script running and show logs
trap "$CONTAINER_RUNTIME stop $CONTAINER_ID" EXIT
$CONTAINER_RUNTIME logs -f "$CONTAINER_ID"
