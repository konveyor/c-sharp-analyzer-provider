FROM registry.access.redhat.com/ubi9/ubi as builder

RUN dnf install -y rust-toolset unzip


WORKDIR /csharp-provider
COPY Cargo.lock  /csharp-provider/
COPY Cargo.toml /csharp-provider/
COPY build.rs  /csharp-provider/
COPY src  /csharp-provider/src

RUN --mount=type=cache,id=cagohome,uid=1001,gid=0,mode=0777,target=/root/.cargo cargo build --release

FROM registry.access.redhat.com/ubi9/ubi-minimal

RUN microdnf install -y dotnet-sdk-9.0 dotnet-runtime-9.0 tar gzip findutils && \
    microdnf clean all && \
    rm -rf /var/cache/dnf
RUN dotnet tool install --tool-path=/usr/local/bin Paket
RUN dotnet tool install --tool-path=/usr/local/bin ilspycmd
RUN chgrp -R 0 /home && chmod -R g=u /home
USER 1001

ENV HOME=/home
ENV RUST_LOG=INFO,c_sharp_analyzer_provider_cli=DEBUG,
COPY --chmod=0755 scripts/dotnet-install.sh /usr/local/bin/scripts/dotnet-install.sh
COPY --chmod=0755 scripts/dotnet-install.ps1 /usr/local/bin/scripts/dotnet-install.ps1

WORKDIR /analyzer-lsp
RUN chgrp -R 0 /analyzer-lsp && chmod -R g=u /analyzer-lsp


COPY --from=builder /csharp-provider/target/release/c-sharp-analyzer-provider-cli /usr/local/bin/c-sharp-provider
ENTRYPOINT ["/usr/local/bin/c-sharp-provider"]
CMD ["--name", "c-sharp", "--port", "14651"]
