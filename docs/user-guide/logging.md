# Logging

vigild captures stdout and stderr from every supervised service, stores them
in an in-memory ring buffer, and makes them available through two delivery
paths:

| Path | When to use |
|------|-------------|
| **vigild's own stdout/stderr** (`podman logs`) | Default; each line is prefixed with `[service-name]`. Enabled by `logs-forward: enabled`. |
| **SSE log-stream API** (`/v1/logs/follow`) | Consumed by `vigil logs -f`, external collectors (Vector, Filebeat, …), or any HTTP client. Always active regardless of `logs-forward`. |

---

## `logs-forward`

The `logs-forward` field on a service controls how vigild handles the
service's stdout and stderr.

```yaml
services:
  myapp:
    command: /usr/local/bin/myapp
    logs-forward: enabled    # default
```

| Value | Behaviour |
|-------|-----------|
| `enabled` | **(default)** Captured, stored in the ring buffer, and printed to vigild's stdout/stderr with a `[service-name]` prefix. Output appears in `podman logs` / `docker logs`. |
| `disabled` | Captured and stored in the ring buffer (accessible via `vigil logs` and the SSE API), but **not** printed to vigild's stdout. Nothing appears in `podman logs` for this service. |
| `passthrough` | vigild does **not** capture stdout/stderr at all. The service process inherits vigild's file descriptors and writes directly to the container's stdout/stderr. Use for log-collector services (Vector, Filebeat, …) that format and emit their own output. |

### Choosing the right value

```
Service role                          Recommended value
─────────────────────────────────────────────────────────
Regular application / server          enabled  (default)
High-volume access log emitter        disabled  (keep clean; query via API)
Application feeding an external       disabled  (raw lines stored in buffer;
  log collector via SSE API                     collector reads via /v1/logs/follow)
The log collector itself              passthrough  (writes formatted JSON
  (Vector, Filebeat, …)                           directly to podman logs)
```

---

## Log ring buffer

vigild keeps the **last N lines** per service in memory (one ring buffer per
service). Old lines are evicted as new ones arrive.

### Configuring buffer size

| Method | Description |
|--------|-------------|
| `--log-buffer <N>` | CLI flag passed to `vigild` |
| `VIGIL_LOG_BUFFER=<N>` | Environment variable |

Default: **1000 lines** per service.

```bash
vigild --log-buffer 5000 --layers-dir /etc/vigil/layers --socket /run/vigil/vigild.sock
```

The buffer size also governs the SSE broadcast channel: the channel holds
`max(64, min(buffer/2, 4096))` entries. Slow SSE consumers that fall behind
this limit receive a "lagged" error and are disconnected.

### What is stored

The ring buffer is filled regardless of the `logs-forward` setting (except
`passthrough`, where the output is never captured). This means:

- `vigil logs` always shows recent output even for services with
  `logs-forward: disabled`.
- The SSE stream (`/v1/logs/follow`) always includes these services.
- Only `passthrough` services are invisible to the buffer and API.

---

## Daemon log format

vigild's **own** internal log messages (startup, service lifecycle events,
errors) are written to stderr. The format is configurable:

| Method | Description |
|--------|-------------|
| `--log-format text` | Human-readable with colours (default) |
| `--log-format json` | Structured JSON — suitable for log aggregation pipelines |
| `VIGIL_LOG_FORMAT=json` | Same via environment variable |

```bash
# JSON format — useful when vigild itself is ingested by a log shipper
vigild --log-format json --layers-dir /etc/vigil/layers --socket /run/vigil/vigild.sock
```

> This controls vigild's *own* diagnostic output, not the forwarded service
> logs. Service logs are always forwarded as plain text lines (with the
> `[service]` prefix when `logs-forward: enabled`).

---

## `vigil logs` command

Show buffered and live service output from the CLI.

```bash
vigil logs                      # last 100 lines, all services
vigil logs myapp -n 50          # last 50 lines from myapp
vigil logs -f                   # follow live (Ctrl+C to stop)
vigil logs -f myapp sidecar     # follow specific services
```

Output format:

```
11:23:45.123 [myapp] [stdout] Server listening on :8080
11:23:45.456 [myapp] [stderr] WARN: deprecated config key 'timeout'
```

`vigil logs -f` subscribes to the SSE stream (`GET /v1/logs/follow`).
Buffered lines from before the subscription opened are replayed first, then
live lines follow.

---

## SSE log-stream API

The raw SSE endpoint is available at:

```
GET /v1/logs/follow
```

### Query parameters

| Parameter | Values | Default | Description |
|-----------|--------|---------|-------------|
| `service` | service name | *(all)* | Filter to a single service |
| `format` | `json` \| `text` \| `ndjson` | `json` | Output format |

### Format: `json` (default)

Each SSE `data:` event is a JSON object:

```
data: {"timestamp":"2026-03-18T11:23:45.123Z","service":"myapp","stream":"stdout","message":"Server listening on :8080"}

data: {"timestamp":"2026-03-18T11:23:45.456Z","service":"myapp","stream":"stderr","message":"WARN: deprecated config key"}
```

Fields:

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | RFC 3339 | When vigild received the line |
| `service` | string | Service name |
| `stream` | `"stdout"` \| `"stderr"` | Original output stream |
| `message` | string | The log line (without trailing newline) |

### Format: `text`

Each SSE event is a plain-text line `[service] message`:

```
data: [myapp] Server listening on :8080

data: [myapp] WARN: deprecated config key
```

### Format: `ndjson`

Returns `application/x-ndjson` — one JSON object per line, no SSE framing,
no keep-alive comments. The same fields as `json` format, just without the
`data: ` prefix and event separators:

```
{"timestamp":"2026-03-18T11:23:45.123Z","service":"myapp","stream":"stdout","message":"Server listening on :8080"}
{"timestamp":"2026-03-18T11:23:45.456Z","service":"myapp","stream":"stderr","message":"WARN: deprecated config key"}
```

This is the recommended format for log collectors (Vector, Filebeat, …)
because it feeds directly into stdin-based inputs without any preprocessing.

### Keep-alive

vigild sends `: ping` SSE comments every 15 seconds to keep idle connections
alive through proxies and load balancers. These are filtered out by
`grep "^data: "` in collector pipelines.

### Consuming with curl

```bash
# Stream all JSON events (SSE)
curl -sN --unix-socket /run/vigil/vigild.sock \
  'http://localhost/v1/logs/follow?format=json'

# Stream a single service as text (SSE)
curl -sN --unix-socket /run/vigil/vigild.sock \
  'http://localhost/v1/logs/follow?format=text&service=myapp'

# Newline-delimited JSON — one object per line, no SSE framing (best for collectors)
curl -sN --unix-socket /run/vigil/vigild.sock \
  'http://localhost/v1/logs/follow?format=ndjson'
```

---

## External log collectors

vigild supports two integration patterns for log collectors. Choose based on
whether your collector can act as a server (recommended) or only as a client.

### Push mode (recommended)

vigild connects to the collector's listening socket and streams ndjson
directly. No curl process, no SSE framing. Collectors act as servers —
exactly their natural role.

```yaml
services:

  myapp:
    command: /usr/local/bin/myapp
    startup: enabled
    logs-forward: disabled          # raw lines go to buffer only
    logs-push-socket: /run/collector/input.sock   # vigild connects here

  log-collector:
    command: <collector-command>
    startup: enabled
    on-failure: restart
    backoff-delay: 2s
    backoff-factor: 2.0
    backoff-limit: 30s
    logs-forward: passthrough       # collector output goes directly to podman logs
```

Key points:
- `logs-push-socket` (Unix socket) or `logs-push-addr` (TCP `host:port`) are
  per-service — each service can push to a different collector.
- vigild retries the connection with exponential backoff (500 ms → 30 s), so
  no `after:` dependency is needed; logs during reconnect are best-effort.
- The collector needs no vigil-specific plugin — it just listens on a socket
  and receives newline-delimited JSON.

#### Filebeat (Unix socket push)

[`examples/filebeat-push/`](../../examples/filebeat-push/) — vigild pushes to
Filebeat's `unix` input over `/run/collector/input.sock`:

```yaml
# layer excerpt
myapp:
  logs-forward: disabled
  logs-push-socket: /run/collector/input.sock

filebeat:
  command: sh -c 'filebeat run --strict.perms=false -c /etc/filebeat/vigil.yml 2>/dev/null'
  startup: enabled
  logs-forward: passthrough
```

```yaml
# filebeat.yml
filebeat.inputs:
  - type: unix
    path: /run/collector/input.sock
processors:
  - decode_json_fields:
      fields: ["message"]
      target: ""
      overwrite_keys: true
  - add_fields:
      target: ''
      fields: {collector: filebeat}
  - drop_fields:
      fields: [agent, input, ecs, host, log]
output.console: {}
```

```
podman build -f examples/filebeat-push/Containerfile -t vigil-filebeat-push .
podman run --rm vigil-filebeat-push
# {"@timestamp":"...","collector":"filebeat","message":"INFO req=1...","service":"myapp","stream":"stdout",...}
```

#### Fluent Bit (TCP push)

[`examples/fluentbit/`](../../examples/fluentbit/) — vigild pushes to Fluent
Bit's `tcp` input over `127.0.0.1:5170`:

```yaml
# layer excerpt
myapp:
  logs-forward: disabled
  logs-push-addr: 127.0.0.1:5170

fluentbit:
  # -q suppresses the ASCII banner (printed before config loads, Quiet on has no effect)
  # 2>/dev/null suppresses Fluent Bit's startup stderr so stdout stays pure JSON
  command: sh -c '/fluent-bit/bin/fluent-bit -q -c /fluent-bit/etc/vigil-fluent-bit.conf 2>/dev/null'
  startup: enabled
  logs-forward: passthrough
```

```ini
# fluent-bit.conf
[INPUT]
    Name    tcp
    Listen  127.0.0.1
    Port    5170
    Format  json

[FILTER]
    Name    record_modifier
    Match   *
    Record  collector fluent-bit

[OUTPUT]
    Name    stdout
    Match   *
    Format  json_lines
```

```
podman build -f examples/fluentbit/Containerfile -t vigil-fluentbit .
podman run --rm vigil-fluentbit
# {"date":...,"collector":"fluent-bit","message":"INFO req=1...","service":"myapp","stream":"stdout",...}
```

---

### Pull mode (curl / SSE)

For collectors that cannot listen on a socket, vigild exposes an SSE log
stream. Run the collector as a supervised service that polls the stream via
curl.

```yaml
services:

  myapp:
    command: /usr/local/bin/myapp
    startup: enabled
    logs-forward: disabled      # raw lines go to buffer only, not podman logs

  log-collector:
    command: >-
      sh -c 'curl -sN --unix-socket /run/vigil/vigild.sock
      "http://localhost/v1/logs/follow?format=ndjson"
      | <collector-specific-command>'
    startup: enabled
    after:
      - myapp
    on-failure: restart
    backoff-delay: 2s
    backoff-factor: 2.0
    backoff-limit: 30s
    logs-forward: passthrough   # collector output goes directly to podman logs
```

Key points:
- `myapp` uses `disabled` so its raw lines don't appear unprocessed in
  `podman logs`.
- The collector uses `passthrough` so its formatted output reaches
  `podman logs` directly without vigild wrapping it.
- `after: [myapp]` ensures the collector doesn't start subscribing before
  the application is running.
- `on-failure: restart` with backoff handles transient collector crashes or
  vigild restarts.

### Vector example

[`examples/vector/`](../../examples/vector/) demonstrates Vector consuming
the vigild SSE stream and emitting enriched JSON. Vector runs on Alpine
(musl) — no extra base-image layers needed.

```toml
# vector.toml
[sources.vigil_logs]
type    = "exec"
mode    = "streaming"
# ?format=ndjson: one JSON object per line, no SSE framing — feeds directly
# into Vector's newline_delimited json decoder without any grep/sed.
command = [
  "curl", "-sN", "--unix-socket", "/run/vigil/vigild.sock",
  "http://localhost/v1/logs/follow?format=ndjson"
]
decoding.codec   = "json"
framing.method   = "newline_delimited"

[transforms.enrich]
type   = "remap"
inputs = ["vigil_logs"]
source = """
.collector = "vector"
.level = if .stream == "stderr" { "error" } else { "info" }
del(.command); del(.source_type); del(.pid); del(.host)
"""

[sinks.console]
type            = "console"
inputs          = ["enrich"]
encoding.codec  = "json"
```

```
podman build -f examples/vector/Containerfile -t vigil-vector .
podman run --rm vigil-vector
# {"collector":"vector","level":"info","message":"INFO req=1...","service":"myapp",...}
```

### Filebeat (stdin) example

[`examples/filebeat/`](../../examples/filebeat/) shows the curl pull pattern with
Filebeat. The official `docker.elastic.co/beats/filebeat:8.17.0` image is used
as the final stage — it includes `curl` and `sh`, and runs on Ubuntu.

```
podman build -f examples/filebeat/Containerfile -t vigil-filebeat .
podman run --rm vigil-filebeat
# {"@timestamp":"...","collector":"filebeat","message":"INFO req=1...","service":"myapp",...}
```

#### vigil-log-relay — generic HTTP→TCP ndjson streamer

`vigil-log-relay` is a standalone binary that reads ndjson from an HTTP
source and forwards it to a TCP sink (Filebeat, Fluent Bit, Logstash, …).
Three source modes are available:

| Flag | Source |
|---|---|
| `--kubernetes` | Kubernetes pod logs via the K8s API (in-cluster) |
| `--source-socket PATH` | Unix-domain socket (e.g. vigild's `/tmp/vigild.sock`) |
| `--source-url URL` | HTTP/HTTPS URL |

**Streaming vigild logs to Filebeat via TCP:**

```yaml
# layers/001-services.yaml
services:
  vigil-log-relay:
    command: >
      vigil-log-relay
      --source-socket /tmp/vigild.sock
      --source-path /v1/logs/follow?format=ndjson
      --tcp-sink-host 127.0.0.1
      --tcp-sink-port 5170
    startup: enabled
    after: [filebeat]
    on-failure: restart
    logs-forward: disabled

checks:
  vigil-log-relay-healthz:
    level: alive
    period: 30s
    timeout: 5s
    http:
      url: http://127.0.0.1:9091/healthz
```

Key parameters:

```
--source-socket PATH       Unix socket to connect to
--source-path PATH         HTTP path (default: /v1/logs/follow?format=ndjson)
--tcp-sink-host HOST       TCP sink host (default: 127.0.0.1)
--tcp-sink-port PORT       TCP sink port (default: 5170)
--reconnect-delay MS       Initial backoff delay in ms (default: 500)
--reconnect-max MS         Backoff ceiling in ms (default: 30000)
--reconnect-retries N      Max failures before exit, 0 = unlimited (default: 0)
--healthcheck HOST:PORT    Address for GET /healthz endpoint (default: 127.0.0.1:9091)
--healthcheck-max-age SECS Seconds without tick before /healthz returns 503 (default: 90)
```

#### Kubernetes pod log collector

[`examples/kubernetes-pod-logs/`](../../examples/kubernetes-pod-logs/) — a
self-contained pod that collects logs from *other* pods in a namespace via
the Kubernetes API and forwards them to Filebeat. Both Filebeat and
`vigil-log-relay` are supervised by vigild with automatic restart and
exponential backoff.

A `pod-log-collector-alive` HTTP check polls `GET /healthz` on the streamer,
restarting it if the watch loop stalls. Configure which pods to watch via
environment variables in the Kubernetes Deployment — no layer-level
`environment:` block so operator settings are not overridden.

```
podman build -f examples/kubernetes-pod-logs/Containerfile -t vigil-k8s-pod-logs .
kubectl apply -f examples/kubernetes-pod-logs/k8s/rbac.yaml
kubectl apply -f examples/kubernetes-pod-logs/k8s/deployment.yaml
# {"@timestamp":"...","namespace":"...","pod":"...","stream":"stdout","message":"...","collector":"filebeat"}
```

---

### Connecting to OTLP and other sinks

Neither Vector nor Filebeat requires any vigil-specific plugins — they
receive plain JSON over a socket or via the exec source. From there, any of
their native sinks work: Loki, Elasticsearch, OpenSearch, S3, OTLP/Tempo,
Splunk, etc.

---

## Troubleshooting

### No output in `podman logs`

Check the `logs-forward` setting. For services where you want output in
`podman logs`, set `logs-forward: enabled` (the default) or `logs-forward:
passthrough` (for collector services that manage their own output).

### `vigil logs -f` shows nothing

1. Confirm the service started: `vigil services list`
2. Check that the service does not use `logs-forward: passthrough` — such
   services bypass the ring buffer entirely.
3. Verify the service is producing output (connect interactively or check
   `podman logs`).

### Collector crashes immediately

The collector's curl command will fail if vigild's socket is not ready yet.
Make sure the collector service has `after: [myapp]` (or whichever service
it depends on). vigild starts the collector only once that dependency is
`active`.

### Log lines are duplicated

If both `logs-forward: enabled` on the application service AND the collector
service is also printing the same lines, you have two delivery paths active.
Set `logs-forward: disabled` on the application to suppress the raw lines
from `podman logs`; keep `logs-forward: passthrough` on the collector.

### Slow consumer / "lagged" disconnect

The SSE broadcast channel has a finite depth (`buffer/2`, clamped to
64–4096). A consumer that cannot keep up is disconnected with a "lagged"
error. The collector service's `on-failure: restart` will reconnect it.
To increase headroom, raise `--log-buffer`:

```bash
vigild --log-buffer 4000 ...
```
