# vigil-log-relay

Forwards ndjson log streams to a TCP sink with automatic reconnection.

Three source modes are supported (exactly one required):

| Mode | Flag | Use case |
|------|------|----------|
| Kubernetes | `--kubernetes` | Watch Running pods in a namespace via the K8s Watch API |
| HTTP URL | `--source-url URL` | Read stream from any HTTP/HTTPS ndjson or SSE endpoint |
| Unix socket | `--source-socket PATH` | Read stream from a local vigild Unix-domain socket |

Output is newline-delimited JSON (ndjson) written to a TCP listener — compatible with Filebeat `tcp` input, Fluent Bit `tcp` input, and Logstash `tcp` input with json codec.

> **Standalone use:** `vigil-log-relay` has no dependency on vigil-rs, vigild, or any other crate in this repository. It can be used independently to forward logs from any Kubernetes cluster, HTTP endpoint, or Unix-domain socket to any TCP-based log shipper — no vigil-rs installation required. The only vigil-rs-specific default is `--source-path /v1/logs/follow?format=ndjson`, which can be overridden freely.

---

## Source modes

### Kubernetes (`--kubernetes`)

Watches Running pods in a namespace using the Kubernetes Watch API (event-driven, no polling delay). A background reconcile loop (`--watch-interval`) restarts streams that were closed by the API server.

Each log line is wrapped in a JSON envelope:

```json
{"timestamp":"2026-03-19T10:23:45.123Z","namespace":"default","pod":"api-xyz","stream":"stdout","message":"..."}
```

Requires an in-cluster service account with `get`, `list`, `watch` on `pods` and `get` on `pods/log`. See [`examples/kubernetes-pod-logs/k8s/rbac.yaml`](../../examples/kubernetes-pod-logs/k8s/rbac.yaml).

```
vigil-log-relay --kubernetes \
  --namespace production \
  --pod-selector "app=api" \
  --tail-lines 100 \
  --since-seconds 10 \
  --exclude-pod "^.*-job-" \
  --max-log-requests 20 \
  --tcp-sink-host 127.0.0.1 --tcp-sink-port 5170
```

**Options:**

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--namespace` | `NAMESPACE` | `default` | Namespace to watch |
| `--pod-selector` | `POD_SELECTOR` | _(all pods)_ | Label selector, e.g. `app=myapp` |
| `--watch-interval` | `WATCH_INTERVAL` | `10` | Seconds between stream-reconnect checks |
| `--container` | — | _(first container)_ | Container name to stream |
| `--tail-lines` | `TAIL_LINES` | `0` | Lines to emit on first connect (0 = disabled) |
| `--since-seconds` | `SINCE_SECONDS` | `10` | Seconds back to start on reconnect |
| `--exclude-pod` | — | — | Exclude pods by name regex (repeatable) |
| `--max-log-requests` | `MAX_LOG_REQUESTS` | `0` | Max concurrent pod streams (0 = unlimited) |

**`--tail-lines` vs `--since-seconds`**

`--tail-lines` applies only on the very first connect per pod. On every subsequent reconnect (after the K8s API server closes the stream, typically every ~5 minutes), `--since-seconds` is used instead to cover the reconnect gap. Setting `--since-seconds` ≥ `--watch-interval` avoids missing log lines. When `--tail-lines` is set, `--since-seconds` is ignored on first connect.

**`--max-log-requests`**

Limits simultaneous pod log streams to avoid overloading the K8s API server. Pods that cannot get a slot are deferred to the next reconcile cycle automatically.

---

### HTTP URL (`--source-url`)

Read stream from any HTTP/HTTPS endpoint — both plain ndjson and SSE framing are handled transparently. SSE `data:` prefixes are stripped, metadata lines (`event:`, `id:`, `retry:`) and keepalives are skipped. Self-signed TLS certificates are accepted (useful for vigild's built-in TLS API).

```
vigil-log-relay --source-url https://vigild.example.com/v1/logs/follow?format=ndjson \
  --tcp-sink-host 127.0.0.1 --tcp-sink-port 5170
```

---

### Unix socket (`--source-socket`)

Streams ndjson from a vigild local Unix-domain socket. Useful when vigil-log-relay runs in the same pod or on the same host as vigild.

```
vigil-log-relay --source-socket /tmp/vigild.sock \
  --source-path /v1/logs/follow?format=ndjson \
  --tcp-sink-host 127.0.0.1 --tcp-sink-port 5170
```

---

## Filtering

`--include` and `--exclude` are available in all source modes. Both flags are repeatable; multiple patterns are **OR-combined**.

| Flag | Description |
|------|-------------|
| `--include REGEX` | Forward line only if **any** include pattern matches |
| `--exclude REGEX` | Drop line if **any** exclude pattern matches (applied after `--include`) |

In Kubernetes mode, patterns are matched against the **log message** (the timestamp prefix is stripped first). In URL and Unix-socket modes, patterns are matched against the **full raw line**.

```
# Kubernetes: only ERROR/WARN, skip healthcheck noise
vigil-log-relay --kubernetes \
  --include "ERROR" --include "WARN" \
  --exclude "GET /healthz" --exclude "GET /readyz"

# URL mode: only JSON lines containing "level":"error"
vigil-log-relay --source-url https://host/logs \
  --include '"level":"error"'
```

---

## TCP sink

All source modes write to the same TCP sink. The sink reconnects with exponential backoff if the connection drops.

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--tcp-sink-host` | `TCP_SINK_HOST` | `127.0.0.1` | Sink host |
| `--tcp-sink-port` | `TCP_SINK_PORT` | `5170` | Sink port |

---

## Connection & timeout options

All timeouts default to 0 (disabled). Set them to detect stale connections.

### Source connection

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--source-connect-timeout MS` | `SOURCE_CONNECT_TIMEOUT` | `10000` | TCP connect timeout |
| `--source-read-timeout MS` | `SOURCE_READ_TIMEOUT` | `0` | Max time without any data |
| `--source-idle-timeout MS` | `SOURCE_IDLE_TIMEOUT` | `0` | Max time without a new log line |
| `--source-keepalive-interval SECS` | `SOURCE_KEEPALIVE_INTERVAL` | `0` | TCP keepalive interval |
| `--source-keepalive-timeout SECS` | `SOURCE_KEEPALIVE_TIMEOUT` | `0` | TCP keepalive probe timeout |

### Source reconnect

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--source-reconnect-delay MS` | `SOURCE_RECONNECT_DELAY` | `500` | Initial backoff |
| `--source-reconnect-max MS` | `SOURCE_RECONNECT_MAX` | `30000` | Backoff ceiling |
| `--source-reconnect-retries N` | `SOURCE_RECONNECT_RETRIES` | `0` | Max consecutive failures before exit (0 = unlimited) |

A clean stream EOF (server closed the connection intentionally) resets the failure counter and backoff. Only connection errors, timeouts, and HTTP non-2xx responses count toward `--source-reconnect-retries`.

### Destination connection

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--dest-connect-timeout MS` | `DEST_CONNECT_TIMEOUT` | `10000` | TCP connect timeout |
| `--dest-read-timeout MS` | `DEST_READ_TIMEOUT` | `0` | Per-write timeout |
| `--dest-idle-timeout MS` | `DEST_IDLE_TIMEOUT` | `0` | Reconnect if no data written for this long |
| `--dest-keepalive-interval SECS` | `DEST_KEEPALIVE_INTERVAL` | `0` | TCP keepalive interval |
| `--dest-keepalive-timeout SECS` | `DEST_KEEPALIVE_TIMEOUT` | `0` | TCP keepalive probe timeout |

### Destination reconnect

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--dest-reconnect-delay MS` | `DEST_RECONNECT_DELAY` | `500` | Initial backoff |
| `--dest-reconnect-max MS` | `DEST_RECONNECT_MAX` | `30000` | Backoff ceiling |

---

## Health check

A lightweight HTTP server answers `GET /healthz`:

- `200 ok` — last liveness tick was within `--healthcheck-max-age` seconds
- `503 stale` — no tick received within the deadline

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--healthcheck HOST:PORT` | `HEALTHCHECK` | `127.0.0.1:9091` | Listen address |
| `--healthcheck-max-age SECS` | `HEALTHCHECK_MAX_AGE` | `30` | Stale threshold |

Kubernetes mode ticks on every watch cycle. HTTP modes tick in the background every 30 s regardless of stream activity (prevents false 503s during quiet streams).

---

## Debug logging

`--debug` enables structured logging with timestamps and DEBUG-level output:

```
vigil-log-relay --kubernetes --debug ...
```

Without `--debug`, timestamps are omitted and only INFO level and above are shown.

---

## Proxy support

Unix-socket and TCP sink communicate locally and do not use proxies. All other source modes support both env-var and explicit proxy configuration.

### Environment variables (all source modes)

```
HTTP_PROXY=http://proxy.corp:3128   vigil-log-relay --source-url http://...
HTTPS_PROXY=http://proxy.corp:3128  vigil-log-relay --source-url https://...
NO_PROXY=localhost,127.0.0.1        vigil-log-relay --source-url https://...
```

### Explicit flags (`--source-url` and `--kubernetes`)

| Flag | Env | Description |
|------|-----|-------------|
| `--source-proxy URL` | `SOURCE_PROXY` | Proxy URL — overrides `HTTP_PROXY` / `HTTPS_PROXY` |
| `--source-proxy-insecure` | `SOURCE_PROXY_INSECURE` | Skip TLS certificate verification for the proxy |
| `--source-proxy-cacert PATH` | `SOURCE_PROXY_CACERT` | PEM file with CA certificate(s) or chain to verify the proxy's TLS |

`--source-proxy-cacert` supports chain files with multiple concatenated `-----BEGIN CERTIFICATE-----` blocks — useful for corporate CA hierarchies.

```
# Corporate proxy with internal CA chain — HTTP URL source
vigil-log-relay --source-url https://vigild.internal/v1/logs/follow?format=ndjson \
  --source-proxy https://proxy.corp:3128 \
  --source-proxy-cacert /etc/ssl/certs/corp-ca-chain.pem

# Corporate proxy with internal CA chain — Kubernetes source
vigil-log-relay --kubernetes \
  --source-proxy https://proxy.corp:3128 \
  --source-proxy-cacert /etc/ssl/certs/corp-ca-chain.pem

# Proxy with self-signed certificate
vigil-log-relay --kubernetes \
  --source-proxy https://proxy.internal:3128 \
  --source-proxy-insecure
```

---

## Deployment example

See [`examples/kubernetes-pod-logs/`](../../examples/kubernetes-pod-logs/) for a complete Kubernetes/OpenShift deployment including:

- `Containerfile` — multi-stage build (vigil + vigil-log-relay + Filebeat)
- `k8s/rbac.yaml` — ServiceAccount, ClusterRole, ClusterRoleBinding
- `k8s/deployment.yaml` — Deployment with liveness/readiness probes
- `filebeat.yml` — Filebeat configuration for the TCP input

---

## Source line handling (ndjson and SSE)

Both plain ndjson and SSE framing are handled transparently. The output is always raw ndjson regardless of what the source sends.

| Input line | Action |
|------------|--------|
| *(empty)* | skip — SSE event delimiter |
| `: ping` | skip — SSE keepalive / comment |
| `event: heartbeat` | skip — SSE metadata, not forwarded |
| `id: 42` | skip — SSE metadata, not forwarded |
| `retry: 3000` | skip — SSE metadata, not forwarded |
| `data: {"level":"info",...}` | strip prefix, forward `{"level":"info",...}` |
| `data:{"level":"info",...}` | strip prefix (no space variant), forward payload |
| `{"level":"info",...}` | forward verbatim — plain ndjson line |
