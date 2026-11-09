# Container Setup

## vigild as PID 1

vigild is designed to run as PID 1 inside a container. When running as PID 1
it automatically enables zombie-reaper mode: orphaned child processes (from
exec checks, helper scripts, etc.) are reaped automatically so they do not
accumulate as zombies.

If you run vigild as a non-PID-1 process and still want zombie reaping, pass
`--reaper` or set `VIGIL_REAPER=true`. vigild will then register itself as a
subreaper via `prctl(PR_SET_CHILD_SUBREAPER)`.

## Minimal Dockerfile

```dockerfile
# Stage 1: build
FROM docker.io/library/rust:alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin vigild --bin vigil

# Stage 2: runtime
FROM docker.io/library/alpine:3.21

COPY --from=builder /src/target/release/vigild /usr/local/bin/vigild
COPY --from=builder /src/target/release/vigil  /usr/local/bin/vigil

COPY layers/ /etc/vigil/layers/

ENTRYPOINT ["/usr/local/bin/vigild", "--layers-dir", "/etc/vigil/layers"]
```

vigild creates its runtime directory (`--path-prefix`, default `/run/vigil`)
itself, so no `mkdir` is needed.

### Unprivileged / OpenShift

OpenShift runs containers with a random UID in group `0`, without
capabilities, and `/run` is a root-owned tmpfs. Put the runtime files under
`/tmp` and let group `0` own what vigild must read, as the release image
([Dockerfile](../../Dockerfile)) does:

```dockerfile
FROM docker.io/library/alpine:3.21

COPY --from=builder /src/target/release/vigild /usr/local/bin/vigild
COPY --from=builder /src/target/release/vigil  /usr/local/bin/vigil
COPY layers/ /etc/vigil/layers/

RUN chown -R 1001:0 /etc/vigil && chmod -R g=u /etc/vigil

# vigild creates /tmp/vigil (mode 0711); vigil reads the same variable.
ENV VIGIL_LAYERS=/etc/vigil/layers \
    VIGIL_PATH_PREFIX=/tmp/vigil

USER 1001
ENTRYPOINT ["/usr/local/bin/vigild"]
```

`vigil` inside the container finds the socket through `VIGIL_PATH_PREFIX`;
no `--socket` is needed on either side.

> [!WARNING]
> **Keep `VIGIL_PATH_PREFIX` at 78 bytes or less.** A Unix socket path can be
> at most 107 bytes long (a Linux limit). The control socket
> `<path-prefix>/vigild.sock` adds 12 bytes, a `ready: notify` socket
> `<path-prefix>/notify/<16 hex digits>.sock` adds 29. A prefix on a deep
> volume mount fails: vigild does not start, or (with a `ready: notify`
> service) rejects the layers. See [Socket path limit](../user-guide/installation.md#socket-path-limit-107-bytes).

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
| `--path-prefix <DIR>` | `VIGIL_PATH_PREFIX` | `/run/vigil` | Directory for everything vigild creates at runtime (control socket, sd_notify sockets). Checked at startup: must be a directory owned by vigild's UID, not a symlink, not writable by others (`g+w` is fine). A missing **last** component is created with mode 0711 (search only for others), and so is `notify/` inside it. vigild refuses to start otherwise. On OpenShift use e.g. `/tmp/vigil`. **At most 78 bytes** ([socket path limit](../user-guide/installation.md#socket-path-limit-107-bytes)). |
| `--socket <PATH>` | `VIGIL_SOCKET` | `<path-prefix>/vigild.sock` | Unix socket path for the HTTP API. vigild refuses to start if the path is a symlink, a non-socket file, or a stale socket it cannot remove, or if it is longer than **107 bytes** ([socket path limit](../user-guide/installation.md#socket-path-limit-107-bytes)). |
| `--socket-mode <OCTAL>` | `VIGIL_SOCKET_MODE` | `600` | Permission bits of the Unix socket. While no identity was added over the API, root and vigild's own UID are admin on the socket (bootstrap); widen this (e.g. `660`) only together with local identities. |
| `--tls-addr <ADDR>` | `VIGIL_TLS_ADDR` | — | Enable HTTPS listener, e.g. `0.0.0.0:8443` |
| `--tls-client-ca <PATH>` | `VIGIL_TLS_CLIENT_CA` | — | PEM CA certificate(s) to verify client certificates (mTLS); see [TLS](tls.md) |
| `--exit-code <MODE>` | `VIGIL_EXIT_CODE` | `propagate` | vigild's exit code when a policy shuts it down: `propagate` (the service's code, `128+signal` for signals) or `fixed` (`0`/`10`, v3 behaviour) |
| `--tls-anonymous-access <LEVEL>` | `VIGIL_TLS_ANONYMOUS_ACCESS` | `metrics` | Access of TLS callers without a matching identity: `open` (`/v1/checks`, `/v1/health`, `/v1/system-info`, API docs) or `metrics` (also `/v1/metrics`). Logs, services and state changes always need an identity over TLS. |
| `--cert <PATH>` | `VIGIL_CERT` | — | PEM certificate file for TLS (auto-generated if omitted) |
| `--key <PATH>` | `VIGIL_KEY` | — | PEM private key file for TLS (auto-generated if omitted) |
| `--reaper` | `VIGIL_REAPER` | — | Force subreaper mode (automatic when PID 1) |
| `--log-format <FMT>` | `VIGIL_LOG_FORMAT` | `text` | Log format: `text` or `json` |
| `--log-buffer <N>` | `VIGIL_LOG_BUFFER` | `1000` | Per-service log ring-buffer size (lines); see [Logging](../user-guide/logging.md) |
| `--log-max-line-bytes <BYTES>` | `VIGIL_LOG_MAX_LINE_BYTES` | `131072` | Longest log entry of a service (1024 to 1048576 bytes); a longer line or joined stack trace is split. See [Logging](../user-guide/logging.md#long-lines) |
| `--watch-layers <INTERVAL>` | `VIGIL_WATCH_LAYERS` | — | Reload the layers when their files change, checking every `INTERVAL` (`1s` to `24h`); see [Orchestrators](orchestrators.md#replan-on-configmap-change) |
| `--shutdown-delay <DURATION>` | `VIGIL_SHUTDOWN_DELAY` | `0s` | After SIGTERM, keep the services running for this long (up to `10m`) and answer `503` on `/v1/health` (not for `level=alive`), so a load balancer stops sending requests first; a second signal stops at once. See [Orchestrators](orchestrators.md#scaling-several-replicas) |
| `--worker-threads <N>` | `VIGIL_WORKER_THREADS` | `2` (1 with a single CPU) | Worker threads of the async runtime; each costs about 2 MiB. Without a limit tokio would start one per CPU of the node |

Boolean variables such as `VIGIL_REAPER` take `true` or `false`.

**`VIGIL_WORKER_THREADS` is not a CPU request.** It sets how many threads
vigild uses to wait for events (processes, checks, API requests); almost all
of that time is idle, so 2 threads need far less than 2 CPUs. Do not size
`resources.requests.cpu` or `resources.limits.cpu` after it: give the
container what its services need, plus a few millicores for vigild. The
other direction applies too: with a CPU limit below 1 CPU vigild uses one
thread, and you do not need to raise the limit to get two. The same holds
for `VIGIL_LOG_WORKER_THREADS` of `vigil-log-relay`.

## Sizing memory and CPU

The container's memory limit covers **every process in it**: vigild, the
services, the processes that exec checks start, and `vigil-log-relay` if it
runs there. Size the limit after the services first; vigild's own share
grows with the number of services and checks and with how much they log:

| Part | Memory (RSS) | CPU |
|---|---|---|
| vigild without services | about 22 MiB | none while idle |
| each service | about 30 KiB, plus its log buffer | none while it is quiet |
| each service's log buffer | up to `--log-buffer` entries (default 1000), each its length plus about 30 bytes: about 120 KiB with 1000 lines of 90 characters. An entry is at most `--log-max-line-bytes` (128 KiB): at worst 128 MiB per service, if every entry in the buffer is that long | about 18 µs per line (each line is also written to vigild's stdout) |
| each exec check | almost nothing in vigild; **every run starts a process** (`command`), which counts against the limit while it runs | about 0.5 millicores per run per second, plus the command's own CPU |
| each HTTP or TCP check | runs inside vigild, almost nothing | small |
| reading files (`env-file`, templates, `user`) | up to 16 extra threads of about 2 MiB each while starts read files; they end after 10 s idle | — |

Measured with the release build on Linux x86_64 (2 runtime workers):

| Setup | vigild RSS | vigild CPU |
|---|---|---|
| no services | 22 MiB | 0 |
| 20 quiet services | 29 MiB | 0 |
| 100 quiet services | 31 MiB | 0 |
| 100 exec checks, `period: 1s` | 27 MiB | 50 millicores |
| 100 services, 100 lines/s each, buffers full | 43 MiB | 180 millicores |
| the same with `--log-buffer 100` | 33 MiB | — |

As a rule of thumb, reserve for vigild **25 MiB plus 150 KiB per service**
at the default log buffer, and add 32 MiB if many services that read files
start at the same time. Services that write long lines need more: the
buffer keeps whole entries, and with `logs-multiline` a stack trace is one
entry (a Java trace typically 1 to 10 KB, a `StackOverflowError` with its
1024 frames 33 KB). Lower `--log-buffer` when many services log a
lot: `vigil logs` and `/v1/logs` then go back fewer lines, and a slow
`/v1/logs/follow` consumer skips lines sooner (see
[Logging](../user-guide/logging.md#log-ring-buffer)); the output on
vigild's stdout is unaffected. Each exec check costs a
process start per `period`: many exec checks with a short period cost more
CPU than one HTTP check for the same thing.

The Pod example in [Orchestrators](orchestrators.md) shows a `resources`
block; the numbers above tell how much of it vigild itself uses.

## Signal forwarding

vigild intercepts these signals and forwards them to all running service
process groups:

| Signal | Action |
|---|---|
| `SIGHUP` | Forwarded to all running services |
| `SIGUSR1` | Forwarded to all running services |
| `SIGUSR2` | Forwarded to all running services |
| `SIGTERM` | Graceful shutdown: stop all services, then exit (with `--shutdown-delay`, after reporting not ready for that long) |
| `SIGINT` | Graceful shutdown, as `SIGTERM` |
| `SIGQUIT` | Graceful shutdown, as `SIGTERM` |

Example — trigger a config reload in all nginx instances:

```bash
# From outside the container
podman kill --signal HUP <container-name>
# vigild receives SIGHUP and forwards it to all service process groups
```

To signal only some services, use the API instead (needs `write` access):

```bash
podman exec <container-name> vigil services signal SIGHUP nginx
```

See [`services signal`](../user-guide/cli-reference.md#services-signal-signal-name)
and [`POST /v1/signals`](../api-reference/rest-api.md#post-v1signals).

## Log format

### Text (default)

```
2026-11-08T22:10:33.089809Z  INFO vigild: vigild starting version="4.0.0" layers_dir=/etc/vigil/layers path_prefix=/run/vigil
2026-11-08T22:10:33.090233Z  INFO vigild::server::unix: API listening on "/run/vigil/vigild.sock" (mode 600)
```

### JSON

```bash
vigild --log-format json ...
```

```json
{"timestamp":"2026-11-08T22:10:34.818174Z","level":"INFO","fields":{"message":"vigild starting","version":"4.0.0","layers_dir":"/etc/vigil/layers","path_prefix":"/run/vigil"},"target":"vigild"}
```

The message and its key-value pairs are in `fields`; `target` names the
module that logged.

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
podman exec vigil-hug vigil services list
podman exec vigil-hug vigil checks list
podman exec vigil-hug vigil logs -f
podman exec vigil-hug vigil services restart haproxy
```

## Interacting with a running container

```bash
# List services
podman exec <ctr> vigil services list

# Show logs
podman exec <ctr> vigil logs
podman exec <ctr> vigil logs -f

# Start / stop / restart
podman exec <ctr> vigil services start myapp
podman exec <ctr> vigil services stop myapp
podman exec <ctr> vigil services restart myapp

# Check health
podman exec <ctr> vigil checks list

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
