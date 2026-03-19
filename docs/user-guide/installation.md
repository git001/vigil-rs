# Installation

## Build from source

### Prerequisites

- Rust toolchain 1.85 or later (`rustup update stable`)
- `musl-dev` for static Alpine builds (see below)

### Build

```bash
git clone https://github.com/git001/vigil-rs
cd vigil-rs
cargo build --release --bin vigild --bin vigil
```

Binaries are placed in `target/release/`:

| Binary | Description |
|---|---|
| `vigild` | Daemon — runs as PID 1 or system service |
| `vigil` | CLI client — connect to a running daemon |

### Static binary (Alpine / musl)

For minimal container images:

```bash
# Install musl toolchain
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --bin vigild --bin vigil
```

The resulting binaries have zero shared library dependencies.

## Container images

vigil-rs is distributed as container images via GitHub Container Registry:

```bash
docker pull ghcr.io/git001/vigil-rs:latest
```

Available tags:

| Tag | Description |
|---|---|
| `latest` | Latest stable release |
| `x.y.z` | Specific version |
| `main` | Built from the main branch (unstable) |

Image labels follow OCI conventions:

```
org.opencontainers.image.source  = https://github.com/git001/vigil-rs
org.opencontainers.image.licenses = AGPL-3.0-only
```

## Daemon flags

`vigild` accepts the following flags (all have environment variable equivalents):

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--layers-dir <PATH>` | `VIGIL_LAYERS` | `/etc/vigil/layers` | Directory containing layer YAML files |
| `--socket <PATH>` | `VIGIL_SOCKET` | `/run/vigil/vigild.sock` | Unix socket for the HTTP API |
| `--tls-addr <ADDR>` | `VIGIL_TLS_ADDR` | *(disabled)* | `host:port` for the HTTPS API |
| `--cert <PATH>` | `VIGIL_CERT` | *(auto-generated)* | PEM certificate for TLS |
| `--key <PATH>` | `VIGIL_KEY` | *(auto-generated)* | PEM private key for TLS |
| `--reaper` | `VIGIL_REAPER` | *(auto when PID 1)* | Enable zombie-reaping (init mode) |
| `--log-format <FORMAT>` | `VIGIL_LOG_FORMAT` | `text` | vigild's own log format: `text` or `json` |
| `--log-buffer <N>` | `VIGIL_LOG_BUFFER` | `1000` | Per-service log ring-buffer size (lines) |

See [Logging](logging.md) for details on `--log-format` and `--log-buffer`.

## Directory layout

vigild expects the following directories at runtime:

| Path | Purpose | Notes |
|---|---|---|
| `/etc/vigil/layers/` | Layer YAML files | Configurable via `--layers-dir` |
| `/run/vigil/` | Runtime files (socket) | Create in Dockerfile |

Minimal Dockerfile setup:

```dockerfile
RUN mkdir -p /run/vigil /etc/vigil/layers
COPY layers/ /etc/vigil/layers/
COPY --from=vigil-builder /vigild /usr/local/bin/vigild
COPY --from=vigil-builder /vigil  /usr/local/bin/vigil

ENTRYPOINT ["/usr/local/bin/vigild", \
    "--layers-dir", "/etc/vigil/layers", \
    "--socket",     "/run/vigil/vigild.sock"]
```

See [Container Setup](../operator-guide/container-setup.md) for a full example.
