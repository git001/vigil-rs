# Logging

vigild captures stdout and stderr from every supervised service, stores them
in an in-memory ring buffer, and makes them available through two delivery
paths:

| Path | When to use |
|------|-------------|
| **vigild's own stdout/stderr** (`podman logs`) | Default; each line is prefixed with `[service-name]`. Enabled by `logs-forward: enabled`. |
| **Log API** (`/v1/logs`, `/v1/logs/follow`) | Recent lines and a live stream, for `vigil logs`, external collectors (Vector, Filebeat, `vigil-log-relay`, …) or any HTTP client. Always active regardless of `logs-forward`. |
| **Push** (`logs-push-socket`, `logs-push-addr`) | vigild connects to a collector and streams ndjson; see [Push mode](#push-mode-recommended). |

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
vigild --log-buffer 5000 --layers-dir /etc/vigil/layers
```

**Memory:** a full buffer costs about its line count times (line length +
30 bytes) per service: about 120 KiB with the default 1000 lines of 90
characters, 12 MiB for 100 such services. Long entries (stack traces with
`logs-multiline`, JSON lines) cost their length: at worst `--log-buffer`
times `--log-max-line-bytes` per service, 128 MiB with the defaults. It
counts against the container's memory limit; see
[Sizing memory and CPU](../operator-guide/container-setup.md#sizing-memory-and-cpu).

The buffer size also governs the live stream: the broadcast channel holds
`max(64, min(buffer/2, 4096))` entries. A consumer of `/v1/logs/follow` that
falls this far behind skips the entries it missed and continues; see
[Slow consumer](#slow-consumer--lagged).

### What is stored

The ring buffer is filled regardless of the `logs-forward` setting (except
`passthrough`, where the output is never captured). This means:

- `vigil logs` always shows recent output even for services with
  `logs-forward: disabled`.
- The SSE stream (`/v1/logs/follow`) always includes these services.
- Only `passthrough` services are invisible to the buffer and API.

A service's output does not have to be clean text. vigild reads it line by
line to its end in any case:

- Bytes that are not valid UTF-8 are stored and forwarded as `�` (U+FFFD).
- A line longer than `--log-max-line-bytes` (default **128 KiB**) is stored
  and forwarded in several entries of at most that size, cut between
  characters. Nothing is lost, and a service that writes without newlines
  cannot make vigild's memory grow.
- A `\r` before the newline is removed.

A service whose output must reach vigild's stdout byte for byte (binary
data, another character set) uses `logs-forward: passthrough`: vigild then
does not read it at all.

### Long lines

| Method | Description |
|--------|-------------|
| `--log-max-line-bytes <BYTES>` | CLI flag passed to `vigild` |
| `VIGIL_LOG_MAX_LINE_BYTES=<BYTES>` | Environment variable |

Default: **131072** (128 KiB); allowed 1024 to 1048576 (1 MiB). The default
leaves room for a stack trace joined by `logs-multiline` or written as one
JSON line (logback's JSON encoder, python-json-logger, structlog): Java
and Python traces typically have 1 to 10 KB, a Java `StackOverflowError`
with the JVM's limit of 1024 frames 33 KB. A smaller
limit splits such a line into parts that are no valid JSON.

### Stack traces: `logs-multiline`

A stack trace is many lines. Without further settings each line is an
entry of its own, as with `docker logs` and `kubectl logs`, and a log
collector has to join them again. With `logs-multiline` vigild joins them
itself, since it still sees each service's output alone and in order:

```yaml
services:
  api:
    command: java -jar /app/api.jar
    logs-multiline:
      preset: java          # or python, or indent
  worker:
    command: python3 /app/worker.py
    logs-multiline:
      start: '^\d{4}-\d{2}-\d{2} '   # every log line starts with a date
      timeout: 500ms
```

A line either starts a new entry or belongs to the one before it, then
joined with a newline:

| Setting | A line belongs to the entry before it when … |
|---|---|
| `preset: java` | it is indented (`\tat …`, `\t... 2 more`, `\tSuppressed: …`), starts with `Caused by: `, or names an exception class (`java.lang.IllegalStateException: boom`, a qualified name ending in `Exception`, `Error` or `Throwable`) |
| `preset: python` | it is indented, is `Traceback (most recent call last):`, names an exception (`KeyError: 'id'`, a name ending in `Error`, `Exception`, `Warning`, `Interrupt` or `Exit`), is one of the sentences between chained exceptions, or is empty |
| `preset: indent` | it starts with a space or a tab |
| `start: REGEX` | it does **not** match the regex |
| `continue: REGEX` | it matches the regex |

Exactly one of `preset`, `start` and `continue` is set. So the log line of
`logger.error("request failed", e)` or `logger.exception(…)` and its trace
are one entry. An entry is complete when the next one starts, when no line
came for `timeout` (default `1s`, `10ms` to `10s`), or when it would
grow past `--log-max-line-bytes`. The timeout delays each entry by that
much, when no further line follows at once. The default was `200ms`
before 4.0.1: on a node with no CPU to spare, the lines of one stack trace
can arrive more than 200 ms apart, and the trace was split (one in about
72,000 in our load test). Set a shorter `timeout` where a quiet service's
log lines must appear sooner.

The joined entry is one entry everywhere: in `vigil logs`, `/v1/logs`, the
stream and `logs-push`. On vigild's stdout (`podman logs`) only its first
line has the `[service]` prefix, so the trace looks as the service wrote
it. In the text format of `/v1/logs/follow` (`[service] message`) and of
SSE the lines of an entry are separate lines again; use `format=ndjson` or
`json` for a collector. `logs-multiline` needs `logs-forward: enabled` or
`disabled`: with `passthrough` vigild does not read the output.

---

## Daemon log format

vigild's **own** internal log messages (startup, service lifecycle events,
errors) are written to stdout, like the forwarded service output. The format
is configurable:

| Method | Description |
|--------|-------------|
| `--log-format text` | Human-readable with colours (default) |
| `--log-format json` | Structured JSON — suitable for log aggregation pipelines |
| `VIGIL_LOG_FORMAT=json` | Same via environment variable |

```bash
# JSON format — useful when vigild itself is ingested by a log shipper
vigild --log-format json --layers-dir /etc/vigil/layers
```

See [Container Setup](../operator-guide/container-setup.md#log-format) for
sample lines.

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

Output format (UTC):

```
2026-03-18 11:23:45.123 [myapp] [stdout] Server listening on :8080
2026-03-18 11:23:45.456 [myapp] [stderr] WARN: deprecated config key 'timeout'
```

`vigil logs` reads the buffered lines (`GET /v1/logs`); with `-f` it then
follows the live stream (`GET /v1/logs/follow`).

---

## Log API

All log endpoints need access `read` (see
[access levels](cli-reference.md#identities)).

### Recent lines: `GET /v1/logs`

```
GET /v1/logs?services=myapp,sidecar&n=50
```

Returns the last `n` lines (default 100) of the given services (default:
all) from the ring buffer, in the usual JSON envelope. With `follow=true` the
response is a stream instead: the last `n` lines, then each new line, as
ndjson (`application/x-ndjson`), with nothing lost or doubled in between.
This is the endpoint to use when a collector must not miss the lines from
before it connected.

### Live stream: `GET /v1/logs/follow`

Only new lines, from the moment the client connects.

| Parameter | Values | Default | Description |
|-----------|--------|---------|-------------|
| `services` | comma-separated service names | *(all)* | Only these services |
| `format` | `json` \| `text` \| `ndjson` | `json` | Output format |

Any other parameter is rejected with `400`.

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
  'http://localhost/v1/logs/follow?format=text&services=myapp'

# Newline-delimited JSON — one object per line, no SSE framing (best for collectors)
curl -sN --unix-socket /run/vigil/vigild.sock \
  'http://localhost/v1/logs/follow?format=ndjson'

# The last 20 lines of myapp, then live, as ndjson
curl -sN --unix-socket /run/vigil/vigild.sock \
  'http://localhost/v1/logs?services=myapp&n=20&follow=true'

# Over TLS: an identity of access `read` is needed
curl -sN -k -u viewer:secret \
  'https://myhost:8443/v1/logs/follow?format=ndjson'
```

### Access over the Unix socket

Until the first identity is added over the API, root and vigild's own UID
are admin on the socket (bootstrap), so a collector running as either can
read the logs without credentials. Once an identity is added over the API,
that ends: give the collector's UID a local identity of access `read`, for
example in a layer file:

```yaml
identities:
  log-collector:
    access: read
    local:
      user-id: 1001     # the UID the collector runs as
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
- `logs-push-socket` (Unix socket) and `logs-push-addr` (TCP `host:port`) are
  per-service — each service can push to a different collector. A service
  may set both; its lines then go to both targets.
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
  command: sh -c 'filebeat run --strict.perms=false -c /usr/share/filebeat/vigil-filebeat.yml 2>/dev/null'
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
Three source modes are available; exactly one must be given:

| Flag | Source |
|---|---|
| `--kubernetes` | Kubernetes pod logs via the K8s API |
| `--source-socket PATH` | Unix-domain socket (e.g. vigild's `/tmp/vigil/vigild.sock`) |
| `--source-url URL` | HTTP/HTTPS URL, e.g. vigild's TLS API on another host |

All options and their environment variables are listed in the
[CLI reference](cli-reference.md#vigil-log-relay).

**Streaming vigild logs to Filebeat via TCP:**

```yaml
# layers/001-services.yaml
services:
  vigil-log-relay:
    command: >
      vigil-log-relay
      --source-socket /tmp/vigil/vigild.sock
      --source-path /v1/logs/follow?format=ndjson
      --tcp-sink-host 127.0.0.1
      --tcp-sink-port 5170
    startup: enabled
    after: [filebeat]
    on-failure: restart
    on-check-failure:
      vigil-log-relay-healthz: restart    # restart a stalled relay
    logs-forward: disabled

checks:
  vigil-log-relay-healthz:
    level: alive
    startup: enabled                      # checks are disabled by default
    period: 30s
    timeout: 5s
    http:
      url: http://127.0.0.1:9091/healthz
```

Key parameters:

```
--source-socket PATH             Unix socket to connect to
--source-path PATH               HTTP path (default: /v1/logs/follow?format=ndjson)
--stdout-sink                    Write to stdout instead of TCP (ignores all TCP sink options)
--tcp-sink-host HOST             TCP sink host (default: 127.0.0.1)
--tcp-sink-port PORT             TCP sink port (default: 5170)
--source-reconnect-delay MS      Initial backoff delay in ms (default: 500)
--source-reconnect-max MS        Backoff ceiling in ms (default: 30000)
--source-reconnect-retries N     Max failures before exit, 0 = unlimited (default: 0)
--dest-reconnect-delay MS        Initial TCP sink reconnect delay in ms (default: 500)
--dest-reconnect-max MS          TCP sink reconnect ceiling in ms (default: 30000)
--healthcheck HOST:PORT          Address for GET /healthz endpoint (default: 127.0.0.1:9091)
--healthcheck-max-age SECS       Seconds without tick before /healthz returns 503 (default: 30)
--include REGEX                  Forward a line only if it matches this regex (repeatable)
--exclude REGEX                  Drop a line if it matches this regex (applied after --include, repeatable)
--max-line-bytes BYTES           Longest message; longer lines are forwarded in parts (default: 131072)
--multiline PRESET               Join stack traces into one record (java, python, indent)
```

**From vigild's TLS API** — a relay on another host (or in another
container) reads the stream over HTTPS with a `read` identity. The password
file is read on every reconnect, so a rotated Secret is picked up:

```bash
vigil-log-relay \
  --source-url 'https://vigild.example:8443/v1/logs/follow?format=ndjson' \
  --source-cacert /etc/vigil/ca.pem \
  --source-user-file /var/run/secrets/vigil/log-relay \
  --tcp-sink-host 127.0.0.1 --tcp-sink-port 5170
```

The file holds `user:password`; the identity can come from a layer file (see
[Identities](configuration.md#identities)).

**TCP sink** — when the sink is down or restarts, the relay connects again
(`--dest-reconnect-delay`, doubling up to `--dest-reconnect-max`) and keeps
the lines meanwhile, up to `--send-buffer-lines` (`SEND_BUFFER_LINES`,
default 8192, 64 to 1048576; each kept line costs its length in memory).
Beyond that it drops lines and sums them up in one warning per second:
`send buffer full — dropped log lines … dropped=430 buffer_lines=8192`.
Before each write it checks
whether the sink closed the connection, so a restarted sink gets the line
that would otherwise vanish into the dead connection. Plain TCP has no
acknowledgement, though: a line written in the moment the sink goes away
can still be lost. A sink that must not lose a line needs its own
delivery guarantee (e.g. a local spool of the collector).

**Stdout sink** — `--stdout-sink` writes all forwarded lines directly to stdout instead of
opening a TCP connection. Useful for debugging or piping output to another process.
The relay's own log messages always go to stderr, so stdout carries only the
forwarded ndjson lines:

```
vigil-log-relay --source-socket /tmp/vigil/vigild.sock --stdout-sink
```

**Line filtering** — `--include` and `--exclude` take regular expressions
in Rust regex syntax and can be repeated any number of times. In socket and
URL mode they are matched against the raw ndjson line as vigild sends it,
e.g. `{"timestamp":"…","service":"myapp","stream":"stdout","message":"…"}`.
Evaluation order:

1. If any `--include` pattern is given, a line must match at least one to proceed.
2. If the line then matches any `--exclude` pattern, it is dropped.
3. Otherwise the line is forwarded to the TCP sink (or stdout).

**Line limits:** bytes that are not valid UTF-8 become `�` (U+FFFD) in
every source mode. Nothing is dropped for being long:

- `--max-line-bytes` (`MAX_LINE_BYTES`, default 128 KiB as vigild's
  `--log-max-line-bytes`, allowed 1 KiB to 64 MiB) is the longest
  message. In `--kubernetes` mode a longer pod log line is forwarded in
  several records, each with the line's timestamp.
- In socket and URL mode a line is one record from vigild (ndjson or SSE).
  A record may be up to 6 times `--max-line-bytes` plus 4 KiB long (JSON
  escaping, the other fields), so a vigild record is never cut as long as
  `--max-line-bytes` is at least vigild's `--log-max-line-bytes`. A longer
  line is forwarded in parts; the parts are no valid JSON, and the relay
  logs a warning (`forwarding a line longer than the limit in parts`).
  A collector keeps them as raw text.

**Stack traces** — `--multiline java|python|indent`, `--multiline-start
REGEX` or `--multiline-continue REGEX` join the lines of a stack trace into
one record, with the same rules as vigild's
[`logs-multiline`](#stack-traces-logs-multiline), in every source mode:

- Pod logs (`--kubernetes`): per pod and container; the record has the
  timestamp of its first line.
- Records from vigild (socket or URL mode): per `service` and `stream`, on
  their `message`, so the interleaved output of other services does not
  break a trace. The joined record is the first one with the joined
  message; its other fields (`timestamp`, …) stay as they were, in their
  order. A record that vigild already joined (`logs-multiline`) is one
  entry and stays one.
- Other lines (a source that sends plain text): joined as text. vigild's
  `format=text` puts `[service] ` in front of each line, which hides the
  indentation from the rules; read vigild with `format=ndjson` (the
  default `--source-path`).

`--include`/`--exclude` apply to the whole record. A record that comes in
parts (a line longer than the limit, or a stack trace that would grow past
`--max-line-bytes`) is forwarded or dropped as a whole: the filter decides
on its first part. `--multiline-timeout`
(default `1s`, `10ms` to `10s`; `200ms` before 4.0.1) is how long a record
waits for a further line. Pod logs pass through the container runtime and
the kubelet, so on a busy node the lines of one stack trace can arrive with
gaps; a shorter timeout may split a trace there. Where vigild runs the service, its own `logs-multiline` is the better
place: then `vigil logs`, the API and every consumer get the joined entry.

```yaml
# Only forward lines whose message contains ERROR or WARN, but not the
# health-check requests
services:
  vigil-log-relay:
    command: >
      vigil-log-relay
      --source-socket /tmp/vigil/vigild.sock
      --source-path /v1/logs/follow?format=ndjson
      --include '"message":"[^"]*(ERROR|WARN)'
      --exclude 'GET /healthz'
      --tcp-sink-host 127.0.0.1
      --tcp-sink-port 5170
    startup: enabled
    on-failure: restart
    logs-forward: disabled
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

**Multi-container pods:** by default vigil-log-relay streams the first (or
annotated) container only. Pass `--container all` to stream every container in
`pod.spec.containers`, or `--container app,sidecar` for a comma-separated
subset; a pod gets the named containers it has, and a pod with none of them
is not streamed. When a container name is set, each ndjson record gains a `"container"`
field so downstream systems can distinguish the sources.

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

### Slow consumer / "lagged"

The broadcast channel behind `/v1/logs/follow` has a finite depth
(`buffer/2`, clamped to 64–4096). A consumer that cannot keep up is not
disconnected: it skips the entries it missed and continues with the newest
ones. With `format=json` or `text` the stream carries an SSE comment
`: lagged: skipped N entries` at that point; with `ndjson` the lines are
just missing. To increase headroom, raise `--log-buffer`:

```bash
vigild --log-buffer 4000 ...
```
