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
| `--path-prefix <DIR>` | `VIGIL_PATH_PREFIX` | `/run/vigil` | Directory for everything vigild creates at runtime (control socket, sd_notify sockets). Checked at startup: must be a directory owned by vigild's UID, not a symlink, not writable by others (`g+w` is fine). A missing **last** component is created with mode 0711 (search only for others), and so is `notify/` inside it. vigild refuses to start otherwise. On OpenShift use e.g. `/tmp/vigil`. |
| `--socket <PATH>` | `VIGIL_SOCKET` | `<path-prefix>/vigild.sock` | Unix socket path for the HTTP API. vigild refuses to start if the path is a symlink, a non-socket file, or a stale socket it cannot remove. |
| `--socket-mode <OCTAL>` | `VIGIL_SOCKET_MODE` | `600` | Permission bits of the Unix socket. While no identity was added over the API, root and vigild's own UID are admin on the socket (bootstrap); widen this (e.g. `660`) only together with local identities. |
| `--tls-addr <ADDR>` | `VIGIL_TLS_ADDR` | — | Enable HTTPS listener, e.g. `0.0.0.0:8443` |
| `--exit-code <MODE>` | `VIGIL_EXIT_CODE` | `propagate` | vigild's exit code when a policy shuts it down: `propagate` (the service's code, `128+signal` for signals) or `fixed` (`0`/`10`, v3 behaviour) |
| `--tls-anonymous-access <LEVEL>` | `VIGIL_TLS_ANONYMOUS_ACCESS` | `metrics` | Access of TLS callers without a matching identity: `metrics` (`/v1/metrics`, `/v1/checks`, `/v1/system-info`) or `open` (without metrics). Logs, services and state changes always need an identity over TLS. |
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

With `--exit-code propagate` (the default since v4), vigild exits with the
exit code of the service whose policy shut it down:

| Exit code | Meaning |
|---|---|
| `0` | Clean exit or `success-shutdown` policy triggered |
| `N` | Exit code of the service that triggered `shutdown` / `failure-shutdown` (after `exit-code-map`) |
| `128 + S` | The service was killed by signal `S` (e.g. `143` for SIGTERM) |
| `10` | `failure-shutdown` after a successful exit, or a shutdown triggered by `on-check-failure` |

With `--exit-code fixed` (v3 behaviour) vigild only uses `0` and `10`.
This allows container orchestrators (e.g. Kubernetes Job `podFailurePolicy`)
to act on the real exit code. See the
[configuration reference](../user-guide/configuration.md#on-success--on-failure).
