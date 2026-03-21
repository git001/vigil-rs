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
| `--socket <PATH>` | `VIGIL_SOCKET` | `/run/vigil/vigild.sock` | Unix socket path. Ignored when `--url` is set. |
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

#### `services list [NAME...]`

List services and their current status.

```bash
vigil services list
vigil services list myapp sidecar
```

Example output:

```
Service                  Startup    Status     On-Success         On-Failure         Stop-Signal
--------------------------------------------------------------------------------------------
haproxy                  enabled    active     ignore             restart            SIGUSR1
controller               enabled    active     restart            restart            SIGTERM
```

**Status values:**

| Status | Meaning |
|---|---|
| `active` | Process is running |
| `inactive` | Not running (stopped or not yet started) |
| `backoff` | Waiting before the next restart attempt |
| `error` | Exceeded backoff limit or `requires` dependency failed |

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
Check                    Level    Status Failures
----------------------------------------------------
myapp-alive              alive    up     0/3
postgres-ready           ready    up     0/3
```

**Status values:** `up` (healthy) | `down` (failed `threshold` times)

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
- New services with `startup: enabled` are started.
- Services removed from the plan are stopped.
- Configuration changes take effect on the next restart of each service.

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
| `open` | No auth required — health and system-info endpoints |
| `metrics` | `GET /v1/metrics` only |
| `read` | All `GET` endpoints |
| `write` | `read` + service/check control (start, stop, restart) |
| `admin` | Full access including identity management |

**Bootstrap mode:** When the identity store is empty, every caller is granted
`admin` access so the first identity can be added without credentials.

#### `identities list [NAME...]`

```bash
vigil identities list
vigil identities list ops-user
```

Example output:

```
Name                     Access   Auth
----------------------------------------------
ops-user                 admin    local(uid=1000)
ci-bot                   write    local(any)
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

`basic` (HTTP Basic Auth) and `tls` (client-certificate) identities are added
directly via the REST API (`POST /v1/identities`). See the
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
  --source-socket /tmp/vigild.sock \
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

`stream` is `stdout`, `stderr` (K8s ≥ 1.32 with stream param enabled), or
`output` (combined, fallback for older clusters or when `--no-stream-param` is set).

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--namespace NS` | `NAMESPACE` | `default` | Kubernetes namespace to watch |
| `--pod-selector SELECTOR` | `POD_SELECTOR` | _(all pods)_ | Label selector, e.g. `app=myapp` |
| `--container NAME` | — | _(first container)_ | Stream only this container |
| `--tail-lines N` | `TAIL_LINES` | `0` | Lines to fetch on first connect (`0` = from start) |
| `--since-seconds N` | `SINCE_SECONDS` | `10` | Look-back window in seconds on reconnect |
| `--watch-interval SECS` | `WATCH_INTERVAL` | `10` | Seconds between reconcile ticks (restart finished streams) |
| `--exclude-pod REGEX` | — | — | Skip pods whose name matches (repeatable, OR-combined) |
| `--max-log-requests N` | `MAX_LOG_REQUESTS` | `0` | Max concurrent pod log streams (`0` = unlimited) |
| `--no-stream-param` | `NO_STREAM_PARAM` | `false` | Disable `stream=Stdout/Stderr` query parameter; use combined stream |

**`--no-stream-param`**: The `stream=` parameter was introduced in K8s 1.32 and
is auto-enabled when the API server version is detected as ≥ 1.32. Some
distributions (e.g. OpenShift) forbid it via admission control even on newer
versions — set this flag when you see
`"stream: Forbidden: may not be specified"` errors.

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
| `--healthcheck HOST:PORT` | `127.0.0.1:9091` | Listen address for `GET /healthz` |
| `--healthcheck-max-age SECS` | `90` | Seconds without a tick before `/healthz` returns `503 stale` |

### Other

| Flag | Description |
|---|---|
| `--log-format text\|json` | Log output format (default: `text`) |
| `--version` | Print version and exit |
