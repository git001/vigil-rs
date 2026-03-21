# vigil-rs

**vigil** is a service supervisor and container init daemon written in Rust.
It manages multiple processes inside a single container — starting, stopping,
restarting, and health-checking them — and exposes a REST API over a Unix
socket for programmatic control.

> vigil-rs is inspired by the layer-based service model pioneered by
> [Canonical Pebble](https://github.com/canonical/pebble), implemented from
> scratch in Rust with native PID 1 / container-init support and an extended
> stop-signal model.

---

## Features

| Feature | Description |
|---|---|
| **PID 1 / zombie reaper** | Safe to run as container entrypoint; reaps orphaned child processes automatically |
| **YAML layer config** | Declarative service & check definitions; multiple layers are merged in order |
| **Service supervision** | Start, stop, restart with exponential backoff |
| **Custom stop signal** | Per-service `stop-signal` (SIGTERM, SIGUSR1, SIGHUP, …) + `kill-delay` |
| **Startup ordering** | `after:` / `before:` / `requires:` dependency graph |
| **Health checks** | HTTP, TCP, and exec checks with configurable period/timeout/threshold/delay |
| **on-check-failure** | Automatic service restart (or shutdown) when a check goes down |
| **Alerting** | HTTP(S) notifications on check state transitions — Alertmanager, CloudEvents, OTLP logs, or generic webhook; `env:VAR` resolution for `url`/headers/labels/proxy; proxy support; configurable retry with backoff; startup warning for unset env vars; `body-template` uses [minijinja](https://github.com/mitsuhiko/minijinja) (Jinja2-compatible) for custom webhook payloads |
| **on-success / on-failure** | Per-service exit policies: `restart`, `ignore`, `shutdown`, `failure-shutdown`, `success-shutdown` |
| **Exit-code propagation** | vigild exits with the managed service's actual exit code |
| **Log streaming** | stdout/stderr captured per service; `vigil logs` and SSE follow (`vigil logs -f`) |
| **REST API** | Full HTTP API over Unix socket; Swagger UI at `/docs` (assets loaded from unpkg.com — requires internet access; use `/openapi.json` in air-gapped environments) |
| **OpenAPI** | Auto-generated spec via utoipa; served at `/openapi.json` |
| **TLS listener** | Optional HTTPS API (`--tls-addr`); auto-generates self-signed cert |
| **Identity management** | Named principals with `read`/`write`/`admin` access levels |
| **Signal forwarding** | SIGHUP / SIGUSR1 / SIGUSR2 forwarded to all running service process groups |
| **Replan** | Hot-reload layer YAML files without restarting the daemon |
| **mimalloc** | High-performance allocator enabled globally |

---

## Workspace layout

```
vigil-rs/
├── crates/
│   ├── vigil-types/      # Shared types (plan, API, identity, signal) — no async
│   ├── vigild/           # Daemon binary — axum HTTP server, overlord, service actors
│   ├── vigil/            # CLI binary — hyper Unix-socket client
│   └── vigil-log-relay/  # Standalone log forwarder — K8s pods / HTTP / Unix socket → TCP sink
└── examples/
    ├── full-container/        # vigild + h2o (single service)
    ├── hug/                   # vigild + HAProxy + controller (multi-service)
    ├── kubernetes-pod-logs/   # vigil-log-relay + Filebeat on Kubernetes
    └── ...                    # filebeat, fluentbit, vector, php-caddy, quarkus, …
```

---

## Quick start

### Build

```bash
# Core binaries (daemon + CLI)
cargo build --release --bin vigild --bin vigil

# Include the log forwarder
cargo build --release --bin vigild --bin vigil --bin vigil-log-relay

# All workspace binaries
cargo build --release
```

### Run the daemon

```bash
vigild --layers-dir /etc/vigil/layers --socket /run/vigil/vigild.sock
```

### Use the CLI

```bash
# Default: connect via Unix socket
vigil services
vigil checks
vigil logs -f
vigil start myservice
vigil restart myservice
vigil stop myservice

# Connect via HTTP (when vigild is reachable over the network)
vigil --url http://myhost:8080 services

# Connect via HTTPS (vigild --tls-addr)
vigil --url https://myhost:8443 services

# HTTPS with auto-generated self-signed cert
vigil --url https://myhost:8443 --insecure services
```

### API via curl

```bash
# Unix socket
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/system-info
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/services

# HTTP / HTTPS
curl http://myhost:8080/v1/services
curl -k https://myhost:8443/v1/services
```

---

## Layer configuration

Services and health checks are defined in YAML layer files.
Multiple layers are merged in order — later layers override earlier ones.

```yaml
# /etc/vigil/layers/001-app.yaml
summary: My application

services:

  myapp:
    summary: Main application process
    command: /usr/local/bin/myapp --config /etc/myapp/config.yaml
    startup: enabled
    stop-signal: SIGTERM
    kill-delay: 10s
    on-success: restart
    on-failure: restart
    backoff-delay: 1s
    backoff-factor: 2.0
    backoff-limit: 30s
    on-check-failure:
      myapp-alive: restart

  sidecar:
    summary: Helper process (starts after myapp)
    command: /usr/local/bin/sidecar
    startup: enabled
    after:
      - myapp
    on-success: restart
    on-failure: restart

checks:

  myapp-alive:
    level: alive
    startup: enabled
    delay: 3s       # wait before first check (default: 3s)
    period: 10s
    timeout: 3s
    threshold: 3
    http:
      url: http://localhost:8080/healthz
```

### Supported check types

```yaml
# HTTP check
http:
  url: https://localhost:8080/healthz
  headers:                              # optional — arbitrary key/value map
    Authorization: "Bearer token"
  # insecure: true                      # skip TLS verification (self-signed / dev)
  # ca: /etc/vigil/internal-ca.pem      # custom CA chain for TLS verification

# TCP check
tcp:
  host: localhost
  port: 5432

# Exec check (exit code 0 = healthy)
exec:
  command: pg_isready -U postgres
  service-context: myapp   # inherit env/user/group from service
```

---

## Container usage

vigild is designed to run as PID 1 inside a container.

```dockerfile
ENTRYPOINT ["/usr/local/bin/vigild", \
    "--layers-dir", "/etc/vigil/layers", \
    "--socket",     "/run/vigil/vigild.sock"]
```

Interact with the running container:

```bash
podman exec <ctr> vigil services
podman exec <ctr> vigil logs -f
podman exec <ctr> vigil restart myapp
```

See the [examples/](examples/) directory for complete, buildable examples.

---

## Comparison: container init systems

### vs. dumb-init / tini

| | dumb-init / tini | vigil-rs |
|---|---|---|
| Purpose | Minimal PID 1 shim | Full service supervisor |
| Manages multiple processes | ❌ (single process only) | ✅ |
| Restart on failure | ❌ | ✅ with backoff |
| Health checks | ❌ | ✅ HTTP / TCP / exec |
| HTTP check headers | ❌ | ✅ arbitrary key/value map |
| HTTP check TLS (`insecure` / `ca`) | ❌ | ✅ skip verification or custom CA |
| Alerting (Alertmanager / CloudEvents / OTLP) | ❌ | ✅ |
| Runtime API | ❌ | ✅ Unix-socket REST |
| Configuration | None (CLI args) | YAML layers |
| Signal forwarding | ✅ | ✅ |
| Zombie reaping | ✅ | ✅ |
| Binary size | ~20 KB | ~10 MB |

**When to use dumb-init/tini:** You have exactly one process and only need
zombie reaping + signal forwarding. Zero configuration overhead.

**When to use vigil:** You run multiple processes and want health checks,
automatic restarts, and programmatic control.

---

### vs. s6-overlay

| | s6-overlay | vigil-rs |
|---|---|---|
| Language | C | Rust |
| Configuration | Directory-based (`/etc/s6-overlay/s6-rc.d/`) | YAML layers |
| Health checks | ❌ (external tooling) | ✅ built-in |
| HTTP check headers | ❌ | ✅ arbitrary key/value map |
| HTTP check TLS (`insecure` / `ca`) | ❌ | ✅ skip verification or custom CA |
| Alerting (Alertmanager / CloudEvents / OTLP) | ❌ | ✅ |
| Runtime API | ❌ | ✅ REST over Unix socket |
| Log routing | ✅ (dedicated log daemon) | stdout/stderr → log store |
| Dependency ordering | ✅ (`dependencies.d/`) | ✅ (`after:`) |
| Custom stop signal | ❌ | ✅ per-service |
| Programmatic control | ❌ | ✅ (`vigil` CLI / REST) |
| Used by | linuxserver.io images | - |

**When to use s6-overlay:** Classic multi-service containers (nginx + cron +
sshd) where you don't need runtime control. Battle-tested, extremely small.

**When to use vigil:** You need health checks, per-service restart policies,
dynamic reconfiguration (replan), and a REST API to query or control services
from outside the container.

---

### vs. supervisord

| | supervisord | vigil-rs |
|---|---|---|
| Language | Python | Rust |
| Configuration | INI file | YAML layers |
| Health checks | ❌ | ✅ |
| HTTP check headers | ❌ | ✅ arbitrary key/value map |
| HTTP check TLS (`insecure` / `ca`) | ❌ | ✅ skip verification or custom CA |
| Alerting (Alertmanager / CloudEvents / OTLP) | ❌ | ✅ |
| REST API | XML-RPC | JSON over Unix socket |
| Memory footprint | ~30 MB (Python) | ~10 MB |
| PID 1 safe | ❌ (not designed for it) | ✅ |
| Container-native | ❌ | ✅ |
| Layer merging / replan | ❌ | ✅ |

---

### vs. Canonical Pebble

vigil-rs is a Rust rewrite of Pebble with the following differences:

| | Pebble | vigil-rs |
|---|---|---|
| Language | Go | Rust |
| PID 1 / zombie reaper | ❌ | ✅ |
| Custom stop signal | 🔶 [PR 720](https://github.com/canonical/pebble/pull/720) (unmerged) | ✅ |
| Check `delay` field | ❌ | ✅ (vigil extension) |
| HTTP check headers | ✅ | ✅ arbitrary key/value map |
| HTTP check TLS (`insecure` / `ca`) | ❌ | ✅ skip verification or custom CA |
| Alerting (Alertmanager / CloudEvents / OTLP) | ❌ | ✅ |
| Memory footprint | ~20 MB | ~10 MB |
| API compatibility | Pebble API | Pebble-compatible |
| OpenAPI / Swagger UI | ❌ | ✅ |
| TLS API listener | ❌ | ✅ |
| Exit-code propagation | ❌ (hardcoded 0/10) | ✅ (real exit code) |

vigil-rs is **API-compatible** with Pebble for the core endpoints
(`/v1/services`, `/v1/checks`, `/v1/logs`, `/v1/changes`, `/v1/system-info`)
so existing tooling built against Pebble can be pointed at vigild without
changes.

---

## Examples

### `examples/full-container` — vigild + h2o

Single web server supervised by vigild.
Demonstrates health checks, on-check-failure restart, and the vigil CLI.

```bash
podman build -f examples/full-container/Containerfile -t vigil-h2o .
podman run --rm --network host --name vigil-h2o vigil-h2o
```

### `examples/kubernetes-pod-logs` — vigil-log-relay + Filebeat on Kubernetes

Runs vigild (service supervisor) and vigil-log-relay (K8s pod log forwarder)
as sidecars alongside Filebeat in a single pod. vigil-log-relay watches a
namespace via the K8s Watch API, streams pod logs, and forwards them as ndjson
to Filebeat over a local TCP connection.

**K8s `stream` query parameter (stdout/stderr separation)**

Kubernetes ≥ 1.32 supports a `stream=Stdout` / `stream=Stderr` query parameter
on the pod log endpoint, allowing stdout and stderr to be collected as separate
labeled streams. vigil-log-relay detects the API server version at startup and
enables this automatically.

Some distributions forbid the `stream=` parameter via
admission control even when running K8s 1.32+. In that case the log stream open
fails with HTTP 422 `"stream: Forbidden: may not be specified"`. Set the
`--no-stream-param` flag (or `NO_STREAM_PARAM=true` env var) to fall back to the
combined stdout+stderr stream:

```yaml
env:
  - name: NO_STREAM_PARAM
    value: "true"
```

**HTTP/2 multiplexing**

The kube client (hyper + rustls-tls) currently uses HTTP/1.1 — one TCP
connection per pod stream. HTTP/2 multiplexing is tracked in
[kube-rs PR #1823](https://github.com/kube-rs/kube/pull/1823) (draft). Use
`--max-log-requests` to cap concurrent connections when watching large namespaces.

### `examples/hug` — vigild + HAProxy + controller

Two services: HAProxy (with graceful SIGUSR1 drain) and a controller that
starts after HAProxy. Demonstrates:

- `after:` startup ordering
- `stop-signal: SIGUSR1` for graceful drain
- exec health check via HAProxy Unix socket
- `on-check-failure` restart policy

```bash
podman build -f examples/hug/Containerfile -t vigil-hug .
podman run --rm --network host --name vigil-hug vigil-hug
```

Interact:

```bash
podman exec vigil-hug vigil --socket /run/vigil/vigild.sock services
podman exec vigil-hug vigil --socket /run/vigil/vigild.sock checks
podman exec vigil-hug vigil --socket /run/vigil/vigild.sock logs
```

---

## Testing

### Unit and integration tests

```sh
cargo test --workspace
```

### VTest2 end-to-end tests

End-to-end integration tests are written in the [VTest2](https://code.vinyl-cache.org/vtest/VTest2)
`.vtc` format — the same test framework used by the Varnish HTTP cache. Each
test starts a real `vigild` process (and sometimes `vigil-log-relay`) with a
temporary layers directory and Unix socket, drives it through the HTTP API and
the `vigil` CLI, and asserts on the responses.

22 test files cover: system info, service/check/alert/identity CRUD, log
streaming, TLS listener, reaper, mTLS, alert body templates, log-relay socket
and URL sources, TCP reconnect, JSON log format, HTTPS transport, and more.

```sh
# Run all VTest2 tests (requires VTest2 binary)
tests/vtest/run.sh

# Single test, verbose
tests/vtest/run.sh -v tests/vtest/v0001_system_info.vtc

# Combined coverage report (unit + integration + VTest2)
tests/vtest/run.sh --coverage          # text summary
tests/vtest/run.sh --coverage --html   # HTML report
```

See [`tests/vtest/README.md`](tests/vtest/README.md) for prerequisites,
patterns, and gotchas.

### Coverage

Line coverage across all crates: **~91 %** (measured with
`llvm-cov` via `tests/vtest/run.sh --coverage`).
The `source_k8s` module (live Kubernetes streams) is excluded from automated
coverage — it requires a real cluster.

---

## Architecture

### Two binaries: `vigild` and `vigil`

vigil-rs deliberately splits into two separate binaries — a design choice that
differs from Pebble, which ships a single binary that acts as both daemon and
client depending on the subcommand used.

```
┌──────────────────────────────────────────────────────────────┐
│  Container / Host                                            │
│                                                              │
│  ┌─────────────────────────────────────────────────────┐     │
│  │  vigild  (PID 1)                                    │     │
│  │                                                     │     │
│  │  ┌────────────┐   ┌────────────┐   ┌────────────┐   │     │
│  │  │  Overlord  │   │  Service   │   │   Check    │   │     │
│  │  │  (actor)   │──▶│  Actors    │   │   Actors   │   │     │
│  │  └─────┬──────┘   └────────────┘   └────────────┘   │     │
│  │        │                                            │     │
│  │  ┌─────▼──────┐   ┌────────────┐   ┌────────────┐   │     │
│  │  │  axum API  │   │  LogStore  │   │  TLS API   │   │     │
│  │  │  (HTTP/1.1)│   │ (broadcast)│   │  (opt.)    │   │     │
│  │  └──┬─────────┘   └────────────┘   └─────┬──────┘   │     │
│  └─────┼─────────────────────────────────────┼─────────┘     │
│        │ Unix socket                          │ TCP/TLS      │
│        │ /run/vigil/vigild.sock               │ 0.0.0.0:8443 │
│        │                                      │              │
│  ┌─────▼──────────────┐     ┌─────────────────▼──────────┐   │
│  │  vigil  (CLI)      │     │  vigil  (CLI) /            │   │
│  │  --socket ...      │     │  curl / K8s operator       │   │
│  └────────────────────┘     └────────────────────────────┘   │
└──────────────────────────────────────────────────────────────┘
```

### Why two binaries instead of one?

**Pebble's approach** — single binary, mode-switched by subcommand:
```bash
pebble run      # starts the daemon
pebble services # connects to the running daemon
```
This means the daemon binary must always carry the full client-side HTTP stack,
CLI argument parsing, and output formatting — even when running as PID 1 where
none of that code is ever needed.

**vigil-rs's approach** — strict separation:

| | `vigild` | `vigil` |
|---|---|---|
| Role | Daemon / PID 1 | CLI client |
| Dependencies | axum, rustls, tokio, rcgen, … | hyper-util, reqwest |
| Runs as | PID 1 in container | `podman exec` / host / CI |
| Contains | API server, supervisor logic | HTTP client, output formatting |
| Transports | Unix socket + optional TLS TCP | Unix socket **or** HTTP/HTTPS |
| Final image | Required | Optional |

**Benefits of the split:**

1. **Smaller attack surface in production.** `vigild` in the container image
   does not contain CLI parsing code or any client-side formatting logic.
   `vigil` can be excluded from images where no interactive access is needed.

2. **Independent versioning.** The CLI and daemon can evolve at different
   speeds. A newer `vigil` client can talk to an older `vigild` as long as the
   API contract (via `vigil-types`) holds.

3. **Shared types as the contract.** The `vigil-types` crate is the single
   source of truth for all API request/response types. Both binaries depend on
   it — the daemon serializes with it, the client deserializes with it.
   Adding a field in one place propagates automatically to both sides with full
   compile-time verification.

4. **No PID 1 bloat.** In production containers `vigild` is PID 1 and runs for
   the entire container lifetime. Keeping it lean (no CLI parsing, no output
   formatting, no reqwest) means fewer dependencies and a faster startup.

5. **Mirrors real-world patterns.** Container orchestrators (Kubernetes,
   Nomad) interact with the daemon via the REST API, not the CLI. The CLI
   (`vigil`) is a convenience tool for developers — it is optional and does not
   need to exist in a production image at all.

### Internal daemon architecture

`vigild` uses an **actor-per-service** model built on Tokio:

```
main
 └─ Overlord (tokio task)
     ├─ ServiceActor "haproxy"  (tokio task, mpsc mailbox)
     ├─ ServiceActor "hug"      (tokio task, mpsc mailbox)
     ├─ CheckActor  "check-haproxy" (tokio task, mpsc mailbox)
     └─ LogStore    (broadcast channel → SSE clients)
```

- **Overlord** owns the plan and routes commands from the API to the right
  actor. It is the only component that reads layer YAML files.
- **ServiceActors** manage a single child process each: spawn, signal, wait,
  backoff, restart. They are state machines
  (`Inactive → Starting → Active → Stopping → Backoff → Error`).
- **CheckActors** run health checks on a timer and emit events back to the
  Overlord when a check transitions to `Down`.
- **LogStore** holds a ring buffer of recent log lines and a broadcast channel
  for live SSE streaming to `vigil logs -f` clients.
- All communication is via typed `mpsc` / `oneshot` channels — no shared
  mutable state, no locks.

## License

vigil-rs is dual-licensed:

| Use case | License |
|---|---|
| Open-source projects, internal infrastructure | [AGPL-3.0](LICENSE) — free |
| Closed-source products, SaaS, corporate AGPL policy exception | [Commercial](LICENSE-COMMERCIAL.md) |

**AGPL-3.0 in short:** You can use vigil-rs freely as long as modifications
and derivative works that are made available over a network are published under
the same license. If you embed vigil-rs in a closed-source product or do not
want to open-source your service, you need a commercial license.

Contact **<al-virgilrs@none.at>** for commercial licensing.
