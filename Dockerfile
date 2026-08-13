# syntax=docker/dockerfile:1.7
FROM python:3.13.5-slim-bookworm@sha256:4c2cf9917bd1cbacc5e9b07320025bdb7cdf2df7b0ceaccb55e9dd7e30987419 AS builder
WORKDIR /build
COPY pyproject.toml README.md ./
COPY src ./src
RUN python -m pip wheel --no-cache-dir --wheel-dir /wheels .

FROM python:3.13.5-slim-bookworm@sha256:4c2cf9917bd1cbacc5e9b07320025bdb7cdf2df7b0ceaccb55e9dd7e30987419
ARG VERSION=0.1.0
LABEL org.opencontainers.image.title="EZVIZ VTM Bridge" \
      org.opencontainers.image.version=$VERSION \
      org.opencontainers.image.licenses="Apache-2.0"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 bridge \
    && useradd --uid 10001 --gid bridge --no-create-home --home-dir /nonexistent bridge
COPY --from=builder /wheels /wheels
RUN python -m pip install --no-cache-dir /wheels/*.whl && rm -rf /wheels
USER 10001:10001
WORKDIR /app
EXPOSE 8765
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["python", "-c", "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8765/healthz', timeout=3).read()"]
ENTRYPOINT ["ezviz-vtm-bridge"]
CMD ["serve"]
