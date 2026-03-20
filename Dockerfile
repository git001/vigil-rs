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
COPY dist/${TARGETARCH}/vigild           /usr/local/bin/vigild
COPY dist/${TARGETARCH}/vigil            /usr/local/bin/vigil
COPY dist/${TARGETARCH}/vigil-log-relay  /usr/local/bin/vigil-log-relay

RUN apk add --no-cache bash curl \
 && chmod +x /usr/local/bin/vigild /usr/local/bin/vigil /usr/local/bin/vigil-log-relay \
 && mkdir -p /etc/vigil/layers \
 && chown -R 1001:0 /etc/vigil \
 && chmod -R g=u   /etc/vigil

LABEL org.opencontainers.image.title="vigil-rs" \
      org.opencontainers.image.description="Rust service supervisor and container init daemon" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/git001/vigil-rs"

# Use /tmp for the socket: /run is a root-owned tmpfs on OpenShift and
# any chmod done in the image layer is lost at runtime.
ENV VIGIL_LAYERS=/etc/vigil/layers \
    VIGIL_SOCKET=/tmp/vigild.sock

VOLUME ["/etc/vigil/layers"]

USER 1001

ENTRYPOINT ["/usr/local/bin/vigild"]
