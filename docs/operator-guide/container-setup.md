# Container Setup

## vigild as PID 1

vigild is designed to run as PID 1 inside a container. When running as PID 1
it automatically enables zombie-reaper mode: orphaned child processes (from
exec checks, helper scripts, etc.) are reaped automatically so they do not
accumulate as zombies.

If you run vigild as a non-PID-1 process and still want zombie reaping, pass
`--reaper` or set `VIGIL_REAPER=1`. vigild will then register itself as a
subreaper via `prctl(PR_SET_CHILD_SUBREAPER)`.

## Minimal Dockerfile

```dockerfile
# Stage 1: build
FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin vigild --bin vigil

# Stage 2: runtime
FROM alpine:3.21

COPY --from=builder /src/target/release/vigild /usr/local/bin/vigild
COPY --from=builder /src/target/release/vigil  /usr/local/bin/vigil

COPY layers/ /etc/vigil/layers/

RUN mkdir -p /run/vigil

ENTRYPOINT ["/usr/local/bin/vigild", \
    "--layers-dir", "/etc/vigil/layers", \
    "--socket",     "/run/vigil/vigild.sock"]
```

## OCI image labels

Add standard OCI annotations for compliance and tooling:

```dockerfile
LABEL org.opencontainers.image.source="https://github.com/your-org/your-repo"
LABEL org.opencontainers.image.licenses="AGPL-3.0-only"
LABEL org.opencontainers.image.description="My application supervised by vigil-rs"
```

## vigild CLI flags

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--layers-dir <PATH>` | `VIGIL_LAYERS` | `/etc/vigil/layers` | Directory containing layer YAML files |
| `--socket <PATH>` | `VIGIL_SOCKET` | `/run/vigil/vigild.sock` | Unix socket path for the HTTP API |
| `--tls-addr <ADDR>` | `VIGIL_TLS_ADDR` | — | Enable HTTPS listener, e.g. `0.0.0.0:8443` |
| `--cert <PATH>` | `VIGIL_CERT` | — | PEM certificate file for TLS (auto-generated if omitted) |
| `--key <PATH>` | `VIGIL_KEY` | — | PEM private key file for TLS (auto-generated if omitted) |
| `--reaper` | `VIGIL_REAPER` | — | Force subreaper mode (automatic when PID 1) |
| `--log-format <FMT>` | — | `text` | Log format: `text` or `json` |

## Signal forwarding

vigild intercepts these signals and forwards them to all running service
process groups:

| Signal | Action |
|---|---|
| `SIGHUP` | Forwarded to all running services |
| `SIGUSR1` | Forwarded to all running services |
| `SIGUSR2` | Forwarded to all running services |
| `SIGTERM` | Graceful shutdown: stop all services, then exit |
| `SIGINT` | Graceful shutdown |
| `SIGQUIT` | Graceful shutdown |

Example — trigger a config reload in all nginx instances:

```bash
# From outside the container
podman kill --signal HUP <container-name>
# vigild receives SIGHUP and forwards it to all service process groups
```

## Log format

### Text (default)

```
2026-03-18T10:00:00.123456Z  INFO vigild starting version=1.0.0 layers_dir=/etc/vigil/layers socket=/run/vigil/vigild.sock
2026-03-18T10:00:00.150000Z  INFO service=haproxy starting service
```

### JSON

```bash
vigild --log-format json ...
```

```json
{"timestamp":"2026-03-18T10:00:00.123456Z","level":"INFO","message":"vigild starting","version":"1.0.0"}
```

JSON format is recommended for log aggregators (Loki, Elasticsearch, etc.).

## Multi-service example

See [examples/hug/](../../examples/hug/) for a complete working example with:
- HAProxy + controller process
- Startup ordering (`after:`)
- Custom stop signal (`SIGUSR1` for graceful drain)
- Exec health check via Unix socket
- `on-check-failure` restart policy

```bash
# Build and run
podman build -f examples/hug/Containerfile -t vigil-hug .
podman run --rm --network host --name vigil-hug vigil-hug

# Interact
podman exec vigil-hug vigil services
podman exec vigil-hug vigil checks
podman exec vigil-hug vigil logs -f
podman exec vigil-hug vigil restart haproxy
```

## Interacting with a running container

```bash
# List services
podman exec <ctr> vigil services

# Show logs
podman exec <ctr> vigil logs
podman exec <ctr> vigil logs -f

# Start / stop / restart
podman exec <ctr> vigil start myapp
podman exec <ctr> vigil stop myapp
podman exec <ctr> vigil restart myapp

# Check health
podman exec <ctr> vigil checks

# Hot-reload config
podman exec <ctr> vigil replan
```

## Exit codes

vigild propagates the exit code of the managed service when using
`on-success: shutdown` / `on-failure: shutdown`:

| Exit code | Meaning |
|---|---|
| `0` | Clean exit or `success-shutdown` policy triggered |
| `10` | `failure-shutdown` policy triggered |
| `N` | Actual exit code of the service that triggered shutdown |

This allows container orchestrators to distinguish success from failure.
