# Installation

## Build from source

### Prerequisites

- Rust toolchain 1.88 or later (`rustup update stable`)
- A C toolchain (`gcc` or `clang`); some dependencies build C code
- For static builds: `musl-tools` (amd64) or [`cross`](https://github.com/cross-rs/cross)
  (arm64), see below

### Build

```bash
git clone https://github.com/git001/vigil-rs
cd vigil-rs
cargo build --release --bin vigild --bin vigil --bin vigil-log-relay
```

Binaries are placed in `target/release/`:

| Binary | Description |
|---|---|
| `vigild` | Daemon — runs as PID 1 or system service |
| `vigil` | CLI client — connect to a running daemon |
| `vigil-log-relay` | Forwards log streams (vigild, Kubernetes pods) to a TCP sink; see [Logging](logging.md) |

### Static binary (musl)

For minimal container images:

```bash
# amd64: needs musl-tools (Debian/Ubuntu: apt-get install musl-tools)
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl \
  --bin vigild --bin vigil --bin vigil-log-relay

# arm64: cross-compile with cross (uses a container)
cross build --release --target aarch64-unknown-linux-musl \
  --bin vigild --bin vigil --bin vigil-log-relay
```

The resulting binaries have no shared library dependencies. The release
images are built this way.

## Container images

vigil-rs is distributed as container images (amd64 and arm64) via GitHub
Container Registry:

```bash
docker pull ghcr.io/git001/vigil-rs:latest
```

Available tags:

| Tag | Description |
|---|---|
| `x.y.z` | Specific version, e.g. `4.0.0` |
| `x.y` | Latest patch release of a minor version, e.g. `4.0` |
| `x` | Latest release of a major version, e.g. `4` |
| `latest` | Latest stable release (not set for `-rc`, `-beta`, `-alpha` releases) |

The image is based on Alpine and contains `vigild`, `vigil` and
`vigil-log-relay` in `/usr/local/bin/`. It runs as UID `1001` (group `0`, so
OpenShift's random UID works too) and sets:

| Variable | Value |
|---|---|
| `VIGIL_LAYERS` | `/etc/vigil/layers` (also a volume) |
| `VIGIL_PATH_PREFIX` | `/tmp/vigil` — `/run` is root-owned on OpenShift |

Image labels follow OCI conventions:

```
org.opencontainers.image.source  = https://github.com/git001/vigil-rs
org.opencontainers.image.licenses = AGPL-3.0-only
```

## Daemon flags

`vigild` accepts the following flags. Each one can also be set with the
environment variable shown; boolean variables take `true` or `false`.

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--layers-dir <PATH>` | `VIGIL_LAYERS` | `/etc/vigil/layers` | Directory containing layer YAML files |
| `--path-prefix <DIR>` | `VIGIL_PATH_PREFIX` | `/run/vigil` | Directory for everything vigild creates at runtime (control socket, `ready: notify` sockets). Must be owned by vigild's UID, not a symlink, not writable by others; a missing last component is created (mode `0711`). On OpenShift use e.g. `/tmp/vigil` |
| `--socket <PATH>` | `VIGIL_SOCKET` | `<path-prefix>/vigild.sock` | Unix socket for the HTTP API |
| `--socket-mode <MODE>` | `VIGIL_SOCKET_MODE` | `600` | Permission bits of the Unix socket (octal). Widen (e.g. `660`) only together with local identities: without identities every caller that can connect is admin |
| `--tls-addr <ADDR>` | `VIGIL_TLS_ADDR` | *(disabled)* | `host:port` for the HTTPS API; port `0` picks a free port (shown in the log and as `https-address` in `vigil system-info`). vigild does not start if the address cannot be bound |
| `--cert <PATH>` | `VIGIL_CERT` | *(auto-generated)* | PEM certificate for TLS |
| `--key <PATH>` | `VIGIL_KEY` | *(auto-generated)* | PEM private key for TLS |
| `--tls-client-ca <PATH>` | `VIGIL_TLS_CLIENT_CA` | — | PEM CA certificate(s) to verify TLS client certificates (mTLS). Client certificates are optional; see [TLS](../operator-guide/tls.md) |
| `--tls-anonymous-access <LEVEL>` | `VIGIL_TLS_ANONYMOUS_ACCESS` | `metrics` | Access of TLS callers without a matching identity: `open` (`/v1/checks`, `/v1/health`, `/v1/system-info`, API docs) or `metrics` (also `/v1/metrics`) |
| `--exit-code <MODE>` | `VIGIL_EXIT_CODE` | `propagate` | vigild's exit code when a policy shuts it down: `propagate` (the service's code after `exit-code-map`, 128+signal for signals) or `fixed` (0 on success, 10 on failure, as in v3) |
| `--reaper` | `VIGIL_REAPER` | *(auto when PID 1)* | Enable zombie-reaping (init mode) |
| `--log-format <FORMAT>` | `VIGIL_LOG_FORMAT` | `text` | vigild's own log format: `text` or `json` |
| `--log-buffer <N>` | `VIGIL_LOG_BUFFER` | `1000` | Per-service log ring-buffer size (lines) |
| `--watch-layers <INTERVAL>` | `VIGIL_WATCH_LAYERS` | *(off)* | Reload the layers when the files in the layers dir change, checking every `INTERVAL` (`1s` to `24h`); see [Orchestrators](../operator-guide/orchestrators.md#replan-on-configmap-change) |
| `--worker-threads <N>` | `VIGIL_WORKER_THREADS` | `2` (1 with a single CPU) | Worker threads of the async runtime; each costs about 2 MiB. Without a limit tokio would start one per CPU of the node |
| `-h`, `--help` | | | Print help |
| `-V`, `--version` | | | Print version |

See [Logging](logging.md) for details on `--log-format` and `--log-buffer`,
and [CLI Reference](cli-reference.md) for `vigil` and `vigil-log-relay`.

## Directory layout

| Path | Purpose | Notes |
|---|---|---|
| `/etc/vigil/layers/` | Layer YAML files | Configurable via `--layers-dir` |
| `<path-prefix>/` (`/run/vigil/`) | Runtime files: `vigild.sock`, `notify/` | vigild creates the last component and `notify/` itself |

vigild creates the path prefix when its parent exists and is writable, so
`/run/vigil` needs no `mkdir` when vigild runs as root. Running as another
user, point `--path-prefix` at a directory that user may create, e.g.
`/tmp/vigil`.

Minimal Dockerfile setup:

```dockerfile
RUN mkdir -p /etc/vigil/layers
COPY layers/ /etc/vigil/layers/
COPY --from=vigil-builder /vigild /usr/local/bin/vigild
COPY --from=vigil-builder /vigil  /usr/local/bin/vigil

ENV VIGIL_PATH_PREFIX=/tmp/vigil
ENTRYPOINT ["/usr/local/bin/vigild", "--layers-dir", "/etc/vigil/layers"]
```

See [Container Setup](../operator-guide/container-setup.md) for a full example.
