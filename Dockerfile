# vigil-rs release image
#
# Multi-arch image built by the CI workflow.
# The binaries are pre-built (static musl) and injected via build context.
#
# Build args:
#   TARGETARCH  set automatically by docker buildx (amd64 | arm64)

FROM alpine:3.21

ARG TARGETARCH

# Pre-built static binaries are staged by the CI workflow under dist/<arch>/
COPY dist/${TARGETARCH}/vigild /usr/local/bin/vigild
COPY dist/${TARGETARCH}/vigil  /usr/local/bin/vigil

RUN chmod +x /usr/local/bin/vigild /usr/local/bin/vigil \
 && mkdir -p /run/vigil /etc/vigil/layers

LABEL org.opencontainers.image.title="vigil-rs" \
      org.opencontainers.image.description="Rust service supervisor and container init daemon" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/git001/vigil-rs"

VOLUME ["/etc/vigil/layers"]

ENTRYPOINT ["/usr/local/bin/vigild", \
    "--layers-dir", "/etc/vigil/layers", \
    "--socket",     "/run/vigil/vigild.sock"]
