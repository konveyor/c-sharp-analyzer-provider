#!/bin/bash
# Build script for the Roslyn C# provider container image

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CSHARP_PROVIDER_DIR="$PROJECT_ROOT"

# Default values
IMAGE_NAME="${IMAGE_NAME:-quay.io/konveyor/c-sharp-roslyn-provider}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}"

echo "=================================================="
echo "Building Roslyn C# Provider Container"
echo "=================================================="
echo "Image: $IMAGE_NAME:$IMAGE_TAG"
echo "Runtime: $CONTAINER_RUNTIME"
echo "Build context: $CSHARP_PROVIDER_DIR"
echo ""

# Build the container image
cd "$CSHARP_PROVIDER_DIR"
$CONTAINER_RUNTIME build \
    -f Dockerfile \
    -t "$IMAGE_NAME:$IMAGE_TAG" \
    .

echo ""
echo "=================================================="
echo "Build complete!"
echo "=================================================="
echo "Image: $IMAGE_NAME:$IMAGE_TAG"
echo ""
echo "To test the image:"
echo "  $CONTAINER_RUNTIME run --rm -p 14651:14651 $IMAGE_NAME:$IMAGE_TAG"
echo ""
echo "To test with a project:"
echo "  $CONTAINER_RUNTIME run --rm -p 14651:14651 \\"
echo "    -v /path/to/project:/projects:Z \\"
echo "    $IMAGE_NAME:$IMAGE_TAG"
echo ""
echo "To push the image:"
echo "  $CONTAINER_RUNTIME push $IMAGE_NAME:$IMAGE_TAG"
echo ""
