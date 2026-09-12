# syntax=docker/dockerfile:1.7
FROM rust:1.88.0-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin \
    && printf '\n' > src/lib.rs \
    && printf 'fn main() {}\n' > src/bin/bridge.rs \
    && printf 'fn main() {}\n' > src/bin/scenetrove_pull.rs \
    && cargo build --release --locked --bins \
    && cargo clean --release --package ezviz-vtm-bridge \
    && rm -rf src
COPY src ./src
RUN cargo build --release --locked --bins \
    && strip target/release/ezviz-vtm-bridge target/release/scenetrove-pull

COPY packaging/collect-licenses.sh /usr/local/bin/collect-licenses
RUN sh /usr/local/bin/collect-licenses /licenses

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime
ARG VERSION=0.7.2
ARG REVISION=unknown
LABEL org.opencontainers.image.title="Vistoda EZVIZ" \
      org.opencontainers.image.version=$VERSION \
      org.opencontainers.image.revision=$REVISION \
      org.opencontainers.image.source="https://git.luigibarretta.com/luigibarretta/vistoda-ezviz" \
      org.opencontainers.image.licenses="Apache-2.0"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 bridge \
    && useradd --uid 10001 --gid bridge --no-create-home --home-dir /nonexistent bridge
COPY --from=builder /licenses /usr/share/doc/vistoda/dependencies
COPY LICENSE NOTICE /usr/share/doc/vistoda/
COPY third_party /usr/share/doc/vistoda/third_party
COPY DISTRIBUTION.md /usr/share/doc/vistoda/
COPY packaging/collect-runtime-inventory.sh /usr/local/lib/vistoda-collect-runtime-inventory
RUN sh /usr/local/lib/vistoda-collect-runtime-inventory /usr/share/doc/vistoda/runtime
COPY --from=builder --chmod=0555 /build/target/release/ezviz-vtm-bridge /usr/local/bin/
COPY --from=builder --chmod=0555 /build/target/release/scenetrove-pull /usr/local/bin/
USER 10001:10001
WORKDIR /app
EXPOSE 8765
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["ezviz-vtm-bridge", "healthcheck"]
ENTRYPOINT ["ezviz-vtm-bridge"]
CMD ["serve"]

FROM runtime AS corresponding_sources
USER root
COPY packaging/collect-debian-sources.sh /usr/local/lib/vistoda-collect-debian-sources
RUN sh /usr/local/lib/vistoda-collect-debian-sources

FROM runtime
COPY --from=corresponding_sources /corresponding-source.tar /corresponding-source.tar.sha256 /usr/share/doc/vistoda/
