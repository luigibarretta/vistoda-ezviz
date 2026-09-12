FROM rust:1.88-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS builder
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bins

COPY packaging/collect-licenses.sh /usr/local/bin/collect-licenses
RUN sh /usr/local/bin/collect-licenses /licenses

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime
ARG BUILD_VERSION=0.7.2
ARG BUILD_ARCH=unknown
LABEL io.hass.version="${BUILD_VERSION}" \
      io.hass.type="app" \
      io.hass.arch="${BUILD_ARCH}" \
      org.opencontainers.image.title="Vistoda EZVIZ for Home Assistant" \
      org.opencontainers.image.source="https://github.com/luigibarretta/vistoda-ezviz" \
      org.opencontainers.image.licenses="Apache-2.0"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl ffmpeg gosu jq \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 bridge \
    && useradd --uid 10001 --gid bridge --no-create-home --shell /usr/sbin/nologin bridge
COPY --from=builder /licenses /usr/share/doc/vistoda/dependencies
COPY LICENSE NOTICE /usr/share/doc/vistoda/
COPY third_party /usr/share/doc/vistoda/third_party
COPY DISTRIBUTION.md /usr/share/doc/vistoda/
COPY packaging/collect-runtime-inventory.sh /usr/local/lib/vistoda-collect-runtime-inventory
RUN sh /usr/local/lib/vistoda-collect-runtime-inventory /usr/share/doc/vistoda/runtime
COPY --from=builder /source/target/release/ezviz-vtm-bridge /usr/local/bin/
COPY packaging/home-assistant/run.sh /usr/local/bin/vistoda-app-run
COPY packaging/home-assistant/vistoda-app-bootstrap.sh /usr/local/lib/vistoda-app-bootstrap
COPY packaging/home-assistant/cameras.jq /usr/local/lib/vistoda-cameras.jq
EXPOSE 8765
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8765/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/vistoda-app-run"]

FROM runtime AS corresponding_sources
COPY packaging/collect-debian-sources.sh /usr/local/lib/vistoda-collect-debian-sources
RUN sh /usr/local/lib/vistoda-collect-debian-sources

FROM runtime
COPY --from=corresponding_sources /corresponding-source.tar /corresponding-source.tar.sha256 /usr/share/doc/vistoda/
