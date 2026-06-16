# Build stage
FROM mcr.microsoft.com/dotnet/sdk:9.0 AS builder

WORKDIR /build
COPY src/CSharpProvider.csproj .
RUN dotnet restore

COPY src/ .
RUN dotnet publish -c Release -o /app

# Runtime stage
FROM registry.access.redhat.com/ubi10/ubi-minimal

# Install .NET SDK (required at runtime for dotnet restore on analyzed projects)
RUN rpm --import https://packages.microsoft.com/keys/microsoft.asc && \
    curl -sL https://packages.microsoft.com/config/rhel/10/prod.repo -o /etc/yum.repos.d/microsoft-prod.repo && \
    microdnf install -y dotnet-sdk-9.0 && \
    microdnf clean all && \
    rm -rf /var/cache/dnf

# Create directories with proper permissions for OpenShift compatibility
# Group 0 (root group) needs rwx for OpenShift arbitrary UIDs
RUN mkdir -p /analyzer-lsp /projects && \
    chgrp -R 0 /home /analyzer-lsp /projects && \
    chmod -R g=u /home /analyzer-lsp /projects

# Run as non-root user (kantra/podman convention)
USER 1001

# Environment variables
ENV HOME=/home
ENV DOTNET_ROOT=/usr/lib64/dotnet
# Disable .NET telemetry in container
ENV DOTNET_CLI_TELEMETRY_OPTOUT=1
# Enable optimizations for container environments
ENV DOTNET_RUNNING_IN_CONTAINER=true

WORKDIR /analyzer-lsp

# Copy published application
COPY --from=builder /app /usr/local/lib/csharp-provider

ENTRYPOINT ["dotnet", "/usr/local/lib/csharp-provider/CSharpProvider.dll"]
CMD ["--name", "c-sharp", "--port", "14651"]
