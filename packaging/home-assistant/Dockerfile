FROM rust:1.88-bookworm@sha256:af306cfa71d987911a781c37b59d7d67d934f49684058f96cf72079c3626bfe0 AS builder
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bins

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
ARG BUILD_VERSION=0.4.0
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
COPY --from=builder /source/target/release/ezviz-vtm-bridge /usr/local/bin/
COPY packaging/home-assistant/run.sh /usr/local/bin/vistoda-app-run
COPY packaging/home-assistant/vistoda-app-bootstrap.sh /usr/local/lib/vistoda-app-bootstrap
EXPOSE 8765
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8765/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/vistoda-app-run"]
