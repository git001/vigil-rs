# CLI Reference — `vigil`

The `vigil` binary is the command-line client for the vigild daemon.
It communicates with a running `vigild` via Unix socket or HTTP/HTTPS.

## Global flags

```
vigil [FLAGS] <COMMAND>
```

### Connection

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--path-prefix <DIR>` | `VIGIL_PATH_PREFIX` | `/run/vigil` | Runtime directory of vigild (same variable as for vigild). The socket is `<path-prefix>/vigild.sock` unless `--socket` is set. |
| `--socket <PATH>` | `VIGIL_SOCKET` | `<path-prefix>/vigild.sock` | Unix socket path. Ignored when `--url` is set. |
| `--socket-uid <UID>` | `VIGIL_SOCKET_UID` | — | Also trust a daemon socket owned by this UID. By default `vigil` only talks to a vigild running as root or as your own UID and refuses any other listener. |
| `--url <URL>` | `VIGIL_URL` | — | HTTP or HTTPS base URL (e.g. `http://host:8080`, `https://host:8443`). Overrides `--socket`. |
| `-k`, `--insecure` | — | false | Skip TLS certificate verification. Only effective with `--url https://...` |
| `--cacert <PATH>` | `VIGIL_CACERT` | — | PEM file with CA certificate(s) to trust for the vigild server TLS connection. Supports chains. Only effective with `--url https://...` |

### Authentication (HTTP/HTTPS transport only)

| Flag | Env var | Default | Description |
|---|---|---|---|
| `-u`, `--user <USER:PASS>` | `VIGIL_USER` | — | HTTP Basic Auth credentials as `username:password`. Sends `Authorization: Basic …` on every request. |
| `--cert <PATH>` | `VIGIL_CERT` | — | PEM file with a client certificate for mTLS. Requires `--key`. Only effective with `--url https://...` |
| `--key <PATH>` | `VIGIL_KEY` | — | PEM file with the private key matching `--cert`. Only effective with `--url https://...` |

### Proxy (HTTP/HTTPS transport only)

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--proxy <URL>` | `VIGIL_PROXY` | — | HTTP or HTTPS proxy URL. Falls back to `HTTPS_PROXY`, `ALL_PROXY`, `HTTP_PROXY` env vars (checked in that order). Ignored with `--socket`. |
| `--proxy-cacert <PATH>` | `VIGIL_PROXY_CACERT` | — | PEM file of the proxy's CA certificate (e.g. corporate MITM proxy). Ignored with `--socket`. |
| `--no-proxy <LIST>` | `VIGIL_NO_PROXY` | — | Comma-separated entries that bypass the proxy. Supports hostnames (`internal.corp`), domain suffixes (`.corp`), IPv4 CIDRs (`192.168.0.0/16`), and IPv6 CIDRs (`fd00::/8`). Ignored with `--socket`. |

### Other

| Flag | Description |
|---|---|
| `--help` | Print help. |
| `--version` | Print version. |

### Transport selection

`--url` takes precedence over `--socket`. Both env vars can coexist; `VIGIL_URL` wins.

```bash
# Unix socket (default — inside the container)
vigil services list

# Explicit socket path
vigil --socket /run/vigil/vigild.sock services list

# HTTP URL (no TLS)
vigil --url http://10.0.0.5:8080 services list

# HTTPS with self-signed cert (skip verification)
vigil --url https://10.0.0.5:8443 --insecure services list

# HTTPS with a trusted server CA
vigil --url https://vigild.internal:8443 --cacert /etc/vigil/ca.pem services list

# HTTP Basic Auth
vigil --url https://10.0.0.5:8443 --insecure --user deploy:secret services list

# mTLS client certificate
vigil --url https://10.0.0.5:8443 --insecure \
      --cert /etc/vigil/client.crt \
      --key  /etc/vigil/client.key \
      services list

# HTTPS through a corporate MITM proxy
vigil --url https://vigild.internal:8443 \
      --proxy http://proxy.corp:3128 \
      --proxy-cacert /etc/corp-ca.pem \
      --no-proxy "localhost,internal.corp,192.168.0.0/16" \
      services list

# Via environment variables
export VIGIL_URL=https://mycontainer:8443
export VIGIL_USER=deploy:secret
vigil --insecure services list
```

---

## Subcommands

### `system-info`

Show daemon version, boot ID, and API addresses.

```bash
vigil system-info
```

Example output:

```
version:       1.0.0
boot-id:       a1b2c3d4-...
http-address:  /run/vigil/vigild.sock
https-address: 0.0.0.0:8443
```

> For a richer status view including uptime, use `vigil vigild status`.

---

### `services`

Manage supervised services.

#### `services list [NAME...] [-o table|json]`

List services and their current status. `-o json` (`VIGIL_OUTPUT=json`)
prints the API objects, including `last-exit`.

```bash
vigil services list
vigil services list myapp sidecar
vigil services list -o json
```

Example output:

```
Service                  Startup    Status     On-Success         On-Failure         Stop-Signal  Last-Exit
------------------------------------------------------------------------------------------------------------
haproxy                  enabled    active     ignore             restart            SIGUSR1      -
controller               enabled    inactive   restart            ignore             SIGTERM      SIGTERM
migrate                  enabled    inactive   ignore             ignore             SIGTERM      0
```

`Last-Exit` is the exit code of the service's last process, the signal name
if it was killed by a signal, or `-` if it has not exited yet. A `Next-Run`
column (UTC) is added when a service has a `schedule`, a `Message` column when
a service reported a `STATUS=` text via sd_notify or waits for a dependency
before autostart starts it (`waiting for db`).

**Status values:**

| Status | Meaning |
|---|---|
| `starting` | Process is running but not ready yet (`ready: notify` / `ready: check`) |
| `active` | Process is running and ready |
| `inactive` | Not running: stopped, not yet started, or stopped because a `requires` dependency stopped |
| `backoff` | Waiting before the next restart attempt |
| `error` | Gave up: the backoff limit was reached, or the process could not be started (for example an unknown user) |
| `done` | A `type: oneshot` service finished successfully |

#### `services start [NAME...]`

Start one or more services (empty = all).

```bash
vigil services start myapp
vigil services start          # start all
```

#### `services stop [NAME...]`

Stop one or more services.

```bash
vigil services stop myapp
vigil services stop myapp sidecar
```

The stop sequence: send `stop-signal` → wait `kill-delay` → SIGKILL if needed.

#### `services restart [NAME...]`

Restart one or more services. Equivalent to stop followed by start.

```bash
vigil services restart myapp
vigil services restart        # restart all
```

---

### `checks`

Manage health checks.

#### `checks list [NAME...]`

```bash
vigil checks list
vigil checks list myapp-alive
```

Example output:

```
Check                    Level    Status Failures   Next run   Last error
------------------------------------------------------------------------------------------
myapp-alive              alive    up     0/3        7s
postgres-ready           ready    down   3/3        2s         exit status 2: localhost:5432 - no response
```

**Status values:** `up` (healthy) | `down` (failed `threshold` times)

**Last error:** why the last run failed, until a run passes: the first line,
cut to 60 characters. `GET /v1/checks` has the full text (`last-error`). See
[Why a check fails](configuration.md#why-a-check-fails).

#### `checks run NAME`

Run a check now (`POST /v1/checks/refresh`) and print the result. The run
counts like a scheduled one (failures, threshold, status, alerts); a check
with `startup: disabled` runs once without changing any state.

```bash
vigil checks run postgres-ready
```

```
postgres-ready: failed (status down, failures 3/3)
Error: check postgres-ready failed: exit status 2: localhost:5432 - no response
```

Exit code 0 when the check passed (`postgres-ready: passed (status up,
failures 0/3)`), 1 when it failed or does not exist.

---

### `logs [SERVICE...] [-n N] [-f]`

Show recent log output from services. Output is drawn from the in-memory
log ring buffer; live streaming is backed by vigild's SSE endpoint
(`GET /v1/logs/follow`).

```bash
vigil logs                      # last 100 lines, all services
vigil logs myapp -n 50          # last 50 lines from myapp
vigil logs -f                   # follow live stream (Ctrl+C to stop)
vigil logs -f myapp sidecar     # follow specific services
```

**Flags:**

| Flag | Description |
|---|---|
| `-n <N>` | Number of buffered lines to show (default: 100) |
| `-f`, `--follow` | Subscribe to the live SSE stream after showing buffered lines. |

Each line is formatted as:

```
HH:MM:SS.mmm [service-name] [stdout|stderr] message
```

> Services with `logs-forward: passthrough` bypass the ring buffer
> entirely — their output will not appear in `vigil logs`.

See [Logging](logging.md) for details on `logs-forward`, buffer sizing, and
the raw SSE API.

---

### `replan`

Hot-reload layer YAML files from disk without restarting vigild.

```bash
vigil replan
```

After replan:
- New services with `startup: enabled` are started (a `oneshot` that is
  already `done` is not run again; scheduled jobs only re-arm their schedule).
- Services removed from the plan are stopped.
- Configuration changes take effect on the next start of each service; a
  running process is not restarted. A changed `schedule` is re-armed at once.

---

### `vigild`

Control the vigild daemon itself.

#### `vigild status`

Show daemon version, boot ID, uptime, and API addresses.

```bash
vigil vigild status
```

Example output:

```
version:      1.0.0
boot-id:      a1b2c3d4-e5f6-7890-abcd-ef1234567890
uptime:       0d 2h 15m 43s
http-address: /run/vigil/vigild.sock
```

#### `vigild stop`

Gracefully stop all supervised services and exit the daemon.

```bash
vigil vigild stop
```

All services receive their `stop-signal` and the daemon waits (up to
`kill-delay`) before shutting down.

#### `vigild restart`

Stop all supervised services and re-execute vigild in-place with the same
arguments. The process image is replaced via `exec()` — the PID stays the same,
which is important when vigild is PID 1.

```bash
vigil vigild restart
```

A successful restart shows a new `boot-id` in `vigil vigild status`.

---

### `identities`

Manage named principals for the access control system.

**Access levels** (least → most privileged):

| Level | Permissions |
|-------|-------------|
| `open` | No auth required — system-info and checks endpoints |
| `metrics` | `GET /v1/metrics` only |
| `read` | All `GET` endpoints |
| `write` | `read` + service/check control (start, stop, restart) |
| `admin` | Full access including identity management |

**Bootstrap mode:** While no identity was added over the API, root and
vigild's own UID get `admin` over the Unix socket, so the first identity can
be added without credentials. Identities from layer files do not end it.
Callers on the TLS listener get at most `--tls-anonymous-access` (default
`metrics`).

Identities can also come from layer files (`identities:`, see
[Identities](configuration.md#identities)). Those are read-only here: `add-local`
or `remove` with their name fails and names the layer file.

#### `identities list [NAME...]`

```bash
vigil identities list
vigil identities list ops-user
```

Example output:

```
Name                     Access   Auth                     Source
------------------------------------------------------------------------
ci-bot                   write    local(any)               api
log-relay                read     basic                    layer 001-auth
ops-user                 admin    local(uid=1000)          api
```

#### `identities add-local <NAME> [--access LEVEL] [--uid UID]`

Add or update a local (Unix-socket UID) identity.

```bash
# Allow any local user with write access
vigil identities add-local ci-bot --access write

# Restrict to a specific UID with admin rights
vigil identities add-local ops-user --access admin --uid 1000
```

Valid access levels: `metrics`, `read`, `write`, `admin`.

#### Adding basic-auth or TLS identities

`basic` (HTTP Basic Auth) and `tls` (client-certificate) identities are
defined in a layer file (see [Identities](configuration.md#identities)) or
added directly via the REST API (`POST /v1/identities`). See the
[REST API reference](../api-reference/rest-api.md) for details, and
`examples/identities/` for a ready-to-run demonstration with all four levels.

```bash
# Add a basic-auth identity
HASH=$(openssl passwd -6 mysecretpassword)
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d "{\"identities\":{\"deploy\":{\"access\":\"write\",\"basic\":{\"password-hash\":\"$HASH\"}}}}" \
  http://localhost/v1/identities

# Authenticate as that identity (Basic Auth over HTTPS)
vigil --url https://vigild.example.com:8443 --insecure \
      --user deploy:mysecretpassword services list
```

#### `identities remove <NAME...>`

```bash
vigil identities remove ci-bot
vigil identities remove ci-bot ops-user
```

---

## `vigil-log-relay`

`vigil-log-relay` is a standalone binary that reads ndjson from a source and
forwards it line-by-line to a TCP sink (Filebeat, Fluent Bit, Logstash, …).
It is typically supervised by `vigild` as a regular service.

### Source flags (mutually exclusive)

| Flag | Description |
|---|---|
| `--source-socket PATH` | Connect to a Unix-domain socket |
| `--source-url URL` | Fetch from an HTTP/HTTPS URL |
| `--kubernetes` | Stream pod logs via the Kubernetes API (in-cluster) |

If none is given, `--source-socket /run/vigil/vigild.sock` is used as default.

### Source path

| Flag | Default | Description |
|---|---|---|
| `--source-path PATH` | `/v1/logs/follow?format=ndjson` | HTTP path appended to the socket/URL base |

### TCP sink

| Flag | Default | Description |
|---|---|---|
| `--tcp-sink-host HOST` | `127.0.0.1` | Destination host |
| `--tcp-sink-port PORT` | `5170` | Destination port |

### Line filters

Both flags are repeatable. Evaluation: include-check first, then exclude-check.

| Flag | Description |
|---|---|
| `--include REGEX` | Forward line only if at least one include pattern matches |
| `--exclude REGEX` | Drop line if any exclude pattern matches (applied after include) |

```bash
# Only forward error and warn lines, drop health-check noise
vigil-log-relay \
  --source-socket /tmp/vigil/vigild.sock \
  --include '"level":"(error|warn)"' \
  --exclude '"path":"/healthz"' \
  --tcp-sink-host 127.0.0.1 \
  --tcp-sink-port 5170
```

### Kubernetes mode (`--kubernetes`)

Pod logs are streamed via the Kubernetes Watch API. vigil-log-relay must run
inside the cluster (uses the in-cluster service account token).

Each log line is emitted as an ndjson object:

```json
{"timestamp":"2026-03-21T21:09:29.722Z","namespace":"my-ns","pod":"myapp-abc","stream":"stdout","message":"..."}
```

When streaming a specific container (`--container NAME`, `,`-list, or `all`),
a `container` field is added:

```json
{"timestamp":"2026-03-21T21:09:29.722Z","namespace":"my-ns","pod":"myapp-abc","container":"sidecar","stream":"stdout","message":"..."}
```

The `container` field is **omitted** when no `--container` flag is given (K8s
default container). `stream` is `stdout` or `stderr` when the API server
accepts the `stream=` parameter, otherwise `output` (one combined stream: older
clusters, the feature gate is off, or `--no-stream-param` is set).

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--namespace NS` | `NAMESPACE` | `default` | Kubernetes namespace to watch |
| `--pod-selector SELECTOR` | `POD_SELECTOR` | _(all pods)_ | Label selector, e.g. `app=myapp` |
| `--container NAME\|all\|n1,n2,...` | `CONTAINER_SELECTOR` | _(first / annotated container)_ | Container(s) to stream. `all` streams every container listed in `pod.spec.containers`; a comma-separated list (e.g. `app,sidecar`) streams exactly those containers. Omit to let K8s pick the default container. |
| `--tail-lines N` | `TAIL_LINES` | `0` | Lines to fetch on first connect (`0` = from start) |
| `--since-seconds N` | `SINCE_SECONDS` | `10` | Look-back window in seconds on reconnect |
| `--watch-interval SECS` | `WATCH_INTERVAL` | `10` | Seconds between reconcile ticks (restart finished streams) |
| `--exclude-pod REGEX` | — | — | Skip pods whose name matches (repeatable, OR-combined) |
| `--max-log-requests N` | `MAX_LOG_REQUESTS` | `0` | Max concurrent pod log streams (`0` = unlimited) |
| `--no-stream-param` | `NO_STREAM_PARAM` | `false` | Never try the `stream=Stdout/Stderr` query parameter; use the combined stream |

**`--no-stream-param`**: The `stream=` parameter exists since K8s 1.32 but only
works with the feature gate `PodLogsQuerySplitStreams`, which is off by default;
some distributions (e.g. OpenShift) also forbid it via admission control. On
K8s ≥ 1.32 the relay tries it first; when the API server answers
`"stream: Forbidden: may not be specified"`, it logs one warning and uses the
combined stream for all pods from then on. The flag skips that first attempt.

### Source proxy

Applies to `--source-url` mode. The `--source-no-proxy` flag is **not** available
for `--kubernetes` mode (proxy settings for the Kubernetes client are handled
separately by the kube library).

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--source-proxy URL` | `SOURCE_PROXY` | — | HTTP or HTTPS proxy URL for the source connection. |
| `--source-proxy-cacert PATH` | `SOURCE_PROXY_CACERT` | — | PEM CA cert (or chain) to verify the proxy's TLS. |
| `--source-no-proxy LIST` | `SOURCE_NO_PROXY` | — | Comma-separated entries that bypass the proxy. Supports hostnames, domain suffixes, IPv4 CIDRs (`10.0.0.0/8`), and IPv6 CIDRs (`fd00::/8`). |

### Reconnect / backoff

| Flag | Default | Description |
|---|---|---|
| `--reconnect-delay MS` | `500` | Initial backoff delay in milliseconds |
| `--reconnect-max MS` | `30000` | Backoff ceiling in milliseconds |
| `--reconnect-retries N` | `0` | Max consecutive failures before exit (`0` = unlimited) |

### Health check endpoint

| Flag | Default | Description |
|---|---|---|
| `--healthcheck HOST:PORT` | `127.0.0.1:9091` | Listen address for `GET /healthz`; port `0` picks a free port (logged at startup). The relay does not start if the address cannot be bound |
| `--healthcheck-max-age SECS` | `30` | Seconds without a tick before `/healthz` returns `503 stale` |

### Other

| Flag | Description |
|---|---|
| `--log-format text\|json` | Log output format (default: `text`) |
| `--version` | Print version and exit |
