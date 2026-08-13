# syntax=docker/dockerfile:1.7
FROM ghcr.io/astral-sh/uv:0.11.19@sha256:b46b03ddfcfbf8f547af7e9eaefdf8a39c8cebcba7c98858d3162bd28cf536f6 AS uv

FROM python:3.13.5-slim-bookworm@sha256:4c2cf9917bd1cbacc5e9b07320025bdb7cdf2df7b0ceaccb55e9dd7e30987419 AS builder
WORKDIR /build
COPY --from=uv /uv /usr/local/bin/uv
COPY pyproject.toml uv.lock README.md ./
COPY src ./src
RUN uv export --frozen --only-group build --no-emit-project \
      --format requirements-txt --output-file /build-requirements.txt \
    && python -m pip wheel --no-cache-dir --require-hashes \
      --wheel-dir /build-wheels --requirement /build-requirements.txt \
    && python -m pip install --no-cache-dir --no-index \
      --find-links=/build-wheels "hatchling==1.27.0" \
    && uv export --frozen --no-dev --no-emit-project --format requirements-txt \
      --output-file /requirements.txt \
    && python -m pip wheel --no-cache-dir --require-hashes \
      --wheel-dir /wheels --requirement /requirements.txt \
    && python -m pip wheel --no-cache-dir --no-build-isolation --no-deps \
      --wheel-dir /wheels .

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
RUN python -m pip install --no-cache-dir --no-index --find-links=/wheels \
      "ezviz-vtm-bridge==$VERSION" \
    && rm -rf /wheels
COPY --chmod=0555 scripts/scenetrove_pull.py /usr/local/bin/scenetrove-pull
USER 10001:10001
WORKDIR /app
EXPOSE 8765
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["python", "-c", "import urllib.request; urllib.request.urlopen('http://127.0.0.1:8765/healthz', timeout=3).read()"]
ENTRYPOINT ["ezviz-vtm-bridge"]
CMD ["serve"]
