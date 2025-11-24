FROM registry.access.redhat.com/ubi9/ubi as builder

RUN dnf install -y rust-toolset unzip

RUN curl -LO https://github.com/protocolbuffers/protobuf/releases/download/v30.2/protoc-30.2-linux-x86_64.zip &&\
    unzip protoc-30.2-linux-x86_64.zip -d $HOME/protoc

WORKDIR /csharp-provider
COPY Cargo.lock  /csharp-provider/
COPY Cargo.toml /csharp-provider/
COPY build.rs  /csharp-provider/
COPY src  /csharp-provider/src

RUN --mount=type=cache,id=cagohome,uid=1001,gid=0,mode=0777,target=/root/.cargo PROTOC=$HOME/protoc/bin/protoc cargo build --release

FROM registry.access.redhat.com/ubi9/ubi

RUN dnf install -y dotnet-sdk-8.0 dotnet-runtime-8.0

RUN dotnet tool install --global Paket
RUN dotnet tool install --global ilspycmd
ENV PATH="/root/.dotnet/tools:${PATH}"
ENV RUST_LOG=INFO,c_sharp_analyzer_provider_cli=DEBUG,

# TODO: move this whole thing to ubi-minimal, and make these permissions actually correct.
COPY --chmod=0777 scripts/dotnet-install.sh /usr/local/bin/scripts/dotnet-install.sh
COPY --chmod=0777 scripts/dotnet-install.ps1 /usr/local/bin/scripts/dotnet-install.ps1

COPY --from=builder /csharp-provider/target/release/c-sharp-analyzer-provider-cli /usr/local/bin/c-sharp-provider
ENTRYPOINT ["/usr/local/bin/c-sharp-provider", "--name", "c-sharp"]
