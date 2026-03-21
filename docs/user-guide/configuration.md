# Configuration Reference

vigil-rs uses **YAML layer files** for configuration. Multiple layer files are
merged in filename order — later files override earlier ones.

## Layer file basics

Layer files live in the directory specified by `--layers-dir`
(default: `/etc/vigil/layers`). They are loaded in lexicographic order by
filename. A common convention is a numeric prefix:

```
/etc/vigil/layers/
├── 001-base.yaml       # base service definitions
├── 050-overrides.yaml  # environment-specific overrides
└── 099-local.yaml      # local developer overrides
```

A layer file has this top-level structure:

```yaml
summary: Human-readable description of this layer
description: Optional longer description

services:
  <service-name>:
    ...

checks:
  <check-name>:
    ...

alerts:
  <alert-name>:
    ...

identities:
  <identity-name>:
    ...
```

---

## Services

### Full reference

```yaml
services:

  myservice:
    # --- Identity ---
    summary: Short description shown in vigil services output
    description: Longer description (optional)

    # --- Process ---
    command: /usr/local/bin/myapp --config /etc/myapp.yaml
    working-dir: /var/lib/myapp        # working directory (default: /)
    user: myapp                        # run as this username
    user-id: 1000                      # or by UID
    group: myapp                       # primary group
    group-id: 1000                     # or by GID
    environment:
      FOO: bar
      DATABASE_URL: postgres://localhost/mydb

    # --- Startup ---
    startup: enabled                   # enabled | disabled (default: disabled)
    after:                             # start after these services are Active
      - database
      - cache
    before:                            # start before these services
      - frontend
    requires:                          # fail if these services are not Active
      - database

    # --- Stop behaviour ---
    stop-signal: SIGTERM               # signal sent on stop (default: SIGTERM)
    kill-delay: 10s                    # SIGKILL sent after this if still running

    # --- Exit policies ---
    on-success: restart                # what to do when process exits with code 0
    on-failure: restart                # what to do when process exits with non-zero

    # --- Health-check-triggered actions ---
    on-check-failure:
      myservice-alive: restart         # restart when check 'myservice-alive' goes Down

    # --- Restart backoff ---
    backoff-delay: 1s                  # initial delay before first restart attempt
    backoff-factor: 2.0                # multiply delay by this after each attempt
    backoff-limit: 30s                 # maximum delay cap

    # --- Logging ---
    logs-forward: enabled              # enabled (default) | disabled | passthrough
    logs-push-socket: /run/collector/input.sock  # push ndjson to Unix socket (mutually exclusive with logs-push-addr)
    logs-push-addr: 127.0.0.1:5170    # push ndjson to TCP address (mutually exclusive with logs-push-socket)
    logs-push-format: ndjson          # ndjson (default; currently the only supported format)

    # --- Layer merging ---
    override: merge                    # merge (default) | replace
```

### Field reference

#### `command`

Required. The command to execute. Parsed as a shell-style string (split on
whitespace; no shell expansion). To use shell features, wrap in `sh -c`:

```yaml
command: sh -c "echo starting && exec /usr/local/bin/myapp"
```

#### `startup`

| Value | Behaviour |
|---|---|
| `enabled` | Service is started automatically when vigild starts (or after `replan`) |
| `disabled` | Service must be started explicitly via `vigil start` or the API |

Default: `disabled`

#### `after` / `before` / `requires`

Startup ordering and dependency constraints. All referenced service names must
exist in the merged plan.

- `after: [a, b]` — this service starts after `a` and `b` reach `Active`
- `before: [c]` — syntactic sugar for `after` from the reverse direction:
  `B before: [A]` is equivalent to `A after: [B]`. B must be running before A starts.
- `requires: [a]` — implies `after` ordering **and** a runtime stop cascade:
  if `a` leaves the running state (Inactive, Backoff, Error), this service is
  stopped automatically. Use this for hard dependencies where the dependent
  service cannot function without the required one.

#### `stop-signal`

The POSIX signal sent to the process group when stopping a service.
Any signal name accepted by `nix` is valid. Common values:

| Signal | Use case |
|---|---|
| `SIGTERM` | Default graceful termination |
| `SIGUSR1` | HAProxy graceful drain (master-worker mode) |
| `SIGHUP` | Nginx / many daemons — reload config |
| `SIGINT` | Some apps treat SIGINT as graceful shutdown |
| `SIGQUIT` | Core dump / some servers |

Default: `SIGTERM`

#### `kill-delay`

Duration string (e.g. `5s`, `30s`, `500ms`). After sending `stop-signal`,
vigild waits this long for the process to exit. If it is still running,
SIGKILL is sent to the entire process group.

Default: `5s`

#### `on-success` / `on-failure`

Policy applied when the managed process exits.

| Value | Behaviour |
|---|---|
| `restart` | Restart the service (with backoff). **Default.** |
| `ignore` | Do nothing; service transitions to `Inactive` |
| `shutdown` | Stop all services and exit vigild; code depends on context |
| `failure-shutdown` | Stop all and exit with code 10 (useful in `on-success`) |
| `success-shutdown` | Stop all and exit with code 0 (useful in `on-failure`) |

#### `on-check-failure`

Map of `<check-name>: <policy>`. When the named check transitions to `Down`,
the policy is applied to this service. Same values as `on-success`/`on-failure`.

#### Backoff

When a service restarts repeatedly, backoff limits restart frequency:

```
delay_n = min(backoff-delay * backoff-factor^n, backoff-limit)
```

Example with defaults (`1s`, `2.0`, `30s`):
`1s → 2s → 4s → 8s → 16s → 30s → 30s → …`

#### `logs-forward`

Controls how vigild handles the service's stdout and stderr.

| Value | Behaviour |
|-------|-----------|
| `enabled` | **(default)** Captured, stored in the log buffer, and printed to vigild's stdout/stderr with a `[service]` prefix. Appears in `podman logs`. |
| `disabled` | Captured and stored in the log buffer (accessible via `vigil logs` and `/v1/logs/follow`), but **not** printed to vigild's stdout. |
| `passthrough` | Not captured at all. The service inherits vigild's file descriptors and writes directly to the container's stdout/stderr. Use for log-collector services (Vector, Filebeat, …). |

See [Logging](logging.md) for full details and examples.

#### `logs-push-socket` / `logs-push-addr`

vigild actively connects to the specified target and pushes log entries for
this service in ndjson format. This lets collectors act as servers (their
natural role) without any `curl` wrapper in the service command.

- `logs-push-socket` — Unix domain socket path (collector listens here).
- `logs-push-addr` — TCP address `host:port` (collector listens here).

The two are mutually exclusive. vigild retries the connection with
exponential backoff (500 ms → … → 30 s) if the collector is not yet ready.
On connection loss vigild reconnects automatically.

```yaml
services:

  myapp:
    command: /usr/local/bin/myapp
    logs-forward: disabled          # raw lines go to ring buffer only
    logs-push-socket: /run/collector/input.sock  # vigild connects here

  filebeat:
    command: filebeat run --strict.perms=false -c /etc/filebeat/filebeat.yml 2>/dev/null
    startup: enabled
    logs-forward: passthrough       # filebeat writes enriched JSON to podman logs
    on-failure: restart
```

Filebeat `filebeat.yml` side:

```yaml
filebeat.inputs:
  - type: unix
    path: /run/collector/input.sock
    json.keys_under_root: true
```

#### `logs-push-format`

> **Not yet implemented.** The field is accepted and parsed but has no effect.
> `ndjson` is currently the only supported format and is always used regardless
> of this setting.

Format for pushed log lines. `ndjson` — one JSON object per line, same schema
as `GET /v1/logs/follow?format=ndjson`.

#### `override`

Controls how this service definition merges with the same service in earlier
layers.

| Value | Behaviour |
|---|---|
| `merge` | Fields from this layer override individual fields in earlier layers. Lists (`after`, `before`, `requires`, `environment`) are merged. **Default.** |
| `replace` | This definition completely replaces the earlier one. |

---

## Checks

### Full reference

```yaml
checks:

  myservice-alive:
    # --- Level ---
    level: alive                       # alive | ready (default: alive)
    startup: enabled                   # enabled | disabled (default: disabled)

    # --- Timing ---
    delay: 3s                          # wait before first check (default: 3s)
    period: 10s                        # how often to run the check (default: 10s)
    timeout: 3s                        # per-check timeout (default: 3s)
    threshold: 3                       # consecutive failures to declare Down (default: 3)

    # --- Check type (exactly one) ---
    http:
      url: https://localhost:8080/healthz
      headers:
        Authorization: "Bearer secret"
        Content-Type: "application/json"
      insecure: false                        # skip TLS verification (default: false)
      ca: /etc/vigil/certs/internal-ca.pem  # custom CA for TLS verification (optional)

    # tcp:
    #   host: localhost                # default: localhost
    #   port: 5432

    # exec:
    #   command: pg_isready -U postgres
    #   service-context: myservice     # inherit env/user/group from this service
    #   environment:
    #     PGPASSWORD: secret
    #   user: postgres
    #   user-id: 999
    #   group: postgres
    #   working-dir: /tmp
```

### Field reference

#### `level`

| Value | Meaning |
|---|---|
| `alive` | Basic liveness — is the process responsive at all? |
| `ready` | Readiness — is the process ready to serve traffic? |

Currently informational only; both levels behave identically in the scheduler.

#### `delay`

Duration to wait after vigild starts before running the first check. Avoids
false failures during slow startup. Default: **3s**.

#### `period`

How often the check runs. Default: **10s**.

#### `timeout`

How long to wait for the check to complete before counting it as a failure.
Default: **3s**.

#### `threshold`

Number of consecutive failures required before the check is declared `Down` and
`on-check-failure` actions are triggered. Default: **3**.

#### HTTP check

```yaml
http:
  url: https://localhost:8080/healthz
  headers:
    Authorization: "Bearer secret"
    Content-Type: "application/json"
  insecure: true                          # skip TLS certificate verification
  ca: /etc/vigil/certs/internal-ca.pem   # PEM CA cert (or chain) to verify the server
```

The check passes if the HTTP response status is 2xx. The `url` field supports
`http://` and `https://`.

| Field | Default | Description |
|-------|---------|-------------|
| `url` | — | HTTP or HTTPS URL to request |
| `headers` | `{}` | Extra request headers (repeatable key/value map) |
| `insecure` | `false` | Skip TLS certificate verification (self-signed certs) |
| `ca` | — | PEM file with CA certificate(s) to verify the server's TLS. Supports chain files with multiple concatenated PEM blocks. |

`insecure` and `ca` are mutually exclusive in intent — `ca` verifies with a
custom root, `insecure` skips verification entirely.

#### TCP check

```yaml
tcp:
  host: localhost   # optional, default: localhost
  port: 5432
```

The check passes if a TCP connection can be established. No data is sent.

#### Exec check

```yaml
exec:
  command: pg_isready -U postgres
  service-context: myservice
```

The check passes if the command exits with code 0. stdout and stderr are
suppressed (they do not appear in container logs).

`service-context` inherits the environment variables, user, group, and working
directory from the named service. Individual fields (`user`, `user-id`, `group`,
`group-id`, `working-dir`, `environment`) override the inherited values.

---

## Alerts

Alerts send HTTP(S) notifications when a check changes state. They fire on
**state transitions only** — not on every check cycle — and support four
wire formats to integrate with common observability stacks.

### Firing and recovery behaviour

| Transition | Alert sent? |
|---|---|
| First check result: Up | **No** — suppressed to avoid spurious recovery on startup |
| First check result: Down | **Yes** — firing |
| Up → Down | **Yes** — firing |
| Down → Up | **Yes** — resolved / recovered |
| Down → Down | **No** — deduplicated |
| Up → Up | **No** — deduplicated |

### Full reference

```yaml
alerts:

  # --- Global queue settings (apply to ALL alerts in this block) ---
  # Maximum number of delivery jobs that may be pending at once.
  # New jobs are dropped (with a warning) when the queue is full.
  # Default: 256
  max-queue-depth: 256

  # Maximum time a delivery job may wait before being silently discarded.
  # Prevents a flood of stale alerts when an endpoint comes back online
  # after a long outage. Default: 60s
  max-queue-time: 60s

  my-alert:
    # --- Endpoint ---
    # Supports "env:VAR" — resolved at send time. Empty result → ERROR, alert dropped.
    url: "env:ALERTMANAGER_URL"              # or a literal URL
    format: alertmanager   # webhook | alertmanager | cloud-events | otlp-logs

    on-check: [website-alive]   # checks that trigger this alert

    # --- Payload ---
    # Extra HTTP headers (e.g. authentication).
    headers:
      Authorization: "Bearer env:ALERTMANAGER_TOKEN"

    # Labels added to the alert payload.
    # Values prefixed with "env:" are resolved from the process environment
    # at send time. vigild warns at startup/replan for each unset env var.
    labels:
      severity: critical
      env: production
      cluster: "env:CLUSTER_NAME"

    # Arbitrary key/value fields included in the alert body.
    # Placement depends on format (annotations in Alertmanager, attributes
    # in OTLP, info object in webhook/CloudEvents).
    # Values prefixed with "env:" are resolved from the process environment.
    send_info_fields:
      k8s_namespace: "env:KUBERNETES_NAMESPACE"
      k8s_service:   "env:KUBERNETES_SERVICE_NAME"
      runbook: "https://wiki.example.com/runbooks/website-down"

    # --- TLS ---
    tls_insecure: false              # skip cert verification (self-signed)
    tls_ca: /etc/vigil/certs/ca.pem # PEM CA cert or chain file

    # --- Proxy ---
    # If omitted, HTTPS_PROXY / ALL_PROXY / HTTP_PROXY env vars are used
    # automatically (same precedence as the vigil CLI).
    proxy:    "env:HTTPS_PROXY"      # explicit proxy URL (overrides env vars)
    proxy_ca: /etc/vigil/certs/proxy-ca.pem  # CA cert to verify proxy TLS
    no_proxy: "env:NO_PROXY"         # comma-separated bypass list

    # --- Retry ---
    retry_attempts: 3                # total attempts including first (default: 3)
    retry_backoff: ["1s", "2s"]      # delays between attempts (default: ["1s", "2s"])
                                     # last entry is reused if list is shorter than
                                     # retry_attempts - 1

    # --- Custom body template (webhook format only) ---
    # Jinja2-style template rendered instead of the default webhook payload.
    # The rendered string must be valid JSON.
    # Variables: check, status, timestamp, labels (map), info (map)
    # Ignored for formats other than webhook.
    body-template: '{"text": "Check {{ check }} is {{ status }}"}'

    # --- Layer merging ---
    override: merge   # merge (default) | replace
```

### Field reference

#### `url`

Required. HTTP or HTTPS endpoint to POST alerts to. Supports `"env:VAR"` —
the environment variable is resolved at send time. If the resolved value is
empty (variable unset or empty), the alert is dropped with an ERROR log.

#### `format`

| Value | Wire format | Content-Type |
|---|---|---|
| `webhook` | Generic JSON object | `application/json` |
| `alertmanager` | Prometheus Alertmanager v2 API array | `application/json` |
| `cloud-events` | CNCF CloudEvents 1.0 structured JSON | `application/cloudevents+json` |
| `otlp-logs` | OpenTelemetry OTLP HTTP/JSON log record | `application/json` |

Default: `webhook`

#### `on-check`

List of check names whose state changes trigger this alert. The alert is sent
when any of the listed checks transitions between `Up` and `Down`.

#### `env:` value resolution

Any string field that accepts `"env:VAR"` resolves the named environment
variable at send time. This applies to: `url`, `headers` values, `labels`
values, `send_info_fields` values, `proxy`, `no_proxy`.

vigild logs a **WARNING** at startup and after every `replan` for each
`env:VAR` reference where the variable is unset or empty — so misconfiguration
is visible immediately without waiting for the first alert:

```
WARN vigild::alert: alert config references unset env var — field will be empty
  alert=my-alert field=url env_var=ALERTMANAGER_URL
```

For `url` specifically an empty resolved value causes the alert to be dropped
with an ERROR rather than silently sending to a blank URL.

#### `labels` / `send_info_fields`

Both accept a key/value map with optional `env:` resolution.
They differ in placement within the payload:

| Format | `labels` | `send_info_fields` |
|---|---|---|
| `webhook` | `labels` object | `info` object |
| `alertmanager` | Alertmanager `labels` map (+ `alertname`, `check` added automatically) | `annotations` map |
| `cloud-events` | `data.labels` object | `data.info` object |
| `otlp-logs` | LogRecord `attributes` | LogRecord `attributes` |

#### `body-template`

A [Jinja2](https://jinja.palletsprojects.com/)-compatible template string
(rendered by [minijinja](https://github.com/mitsuhiko/minijinja)) that replaces
the default webhook payload when `format: webhook` is configured.

The rendered string **must be valid JSON**. On template parse errors, render
errors, or invalid JSON output vigild logs a `WARN` and falls back to the
default webhook payload — the alert is never silently dropped.

**Available template variables:**

| Variable | Type | Value |
|---|---|---|
| `check` | string | Name of the check that triggered the alert |
| `status` | string | `"down"` (firing) or `"up"` (recovery) |
| `timestamp` | string | RFC 3339 timestamp of the event |
| `labels` | object | Resolved `labels` map |
| `info` | object | Resolved `send_info_fields` map |

**Ignored** for `alertmanager`, `cloud-events`, and `otlp-logs` formats.

Example — Slack incoming webhook:

```yaml
body-template: '{"text": "{% if status == \"down\" %}:red_circle:{% else %}:large_green_circle:{% endif %} *{{ check }}* is *{{ status }}*"}'
```

Example — Microsoft Teams MessageCard:

```yaml
body-template: |
  {
    "@type": "MessageCard",
    "@context": "https://schema.org/extensions",
    "themeColor": "{% if status == 'down' %}FF0000{% else %}00CC00{% endif %}",
    "title": "Vigil Alert — {{ check }}",
    "text": "Check **{{ check }}** is **{{ status }}** on `{{ labels.cluster }}`."
  }
```

#### `tls_insecure` / `tls_ca`

Same semantics as the [HTTP check TLS options](#http-check). `tls_ca` supports
chain files (multiple concatenated PEM blocks).

#### `proxy` / `proxy_ca` / `no_proxy`

| Field | Description |
|---|---|
| `proxy` | Explicit proxy URL. Overrides `HTTPS_PROXY` / `ALL_PROXY` / `HTTP_PROXY` env vars. Supports `env:`. |
| `proxy_ca` | PEM CA cert (or chain) to verify the proxy's TLS connection. |
| `no_proxy` | Comma-separated bypass list. Supports hostnames, domain suffixes, IPv4 CIDRs (`192.168.0.0/16`), and IPv6 CIDRs (`fd00::/8`). Supports `env:`. |

If `proxy` is omitted, vigild automatically reads `HTTPS_PROXY`, `ALL_PROXY`,
and `HTTP_PROXY` env vars (in that order) — the same behaviour as the vigil CLI.

#### `retry_attempts` / `retry_backoff`

| Field | Default | Description |
|---|---|---|
| `retry_attempts` | `3` | Total number of send attempts (1 = no retry) |
| `retry_backoff` | `["1s", "2s"]` | Delays between attempts as duration strings. If shorter than `retry_attempts - 1`, the last entry is reused. |

Retries are performed on connection errors and `5xx` responses. Client errors
(`4xx`) are not retried.

### Format details

#### `webhook` — generic JSON

Default payload (no `body-template`):

```json
{
  "check": "website-alive",
  "status": "down",
  "timestamp": "2026-03-21T12:00:00Z",
  "labels": { "env": "production" },
  "info":   { "k8s_namespace": "prod" }
}
```

`status` is `"down"` on firing, `"up"` on recovery.

When `body-template` is set, vigild renders the template and sends the result
instead. This lets you produce any JSON shape required by the target system
(Slack, Teams, PagerDuty, n8n, Zapier, …). See [`body-template`](#body-template)
for the full variable reference.

#### `alertmanager` — Prometheus Alertmanager v2

```json
[{
  "labels":      { "alertname": "website-alive", "check": "website-alive", "env": "production" },
  "annotations": { "runbook": "https://wiki.example.com/…" },
  "startsAt":    "2026-03-21T12:00:00Z",
  "endsAt":      "0001-01-01T00:00:00Z"
}]
```

On recovery `endsAt` is set to the current time — this is the native
Alertmanager mechanism for marking an alert as resolved.

#### `cloud-events` — CNCF CloudEvents 1.0

```json
{
  "specversion": "1.0",
  "id": "<uuid>",
  "source": "vigild",
  "type": "io.vigil.check.failed",
  "time": "2026-03-21T12:00:00Z",
  "datacontenttype": "application/json",
  "data": {
    "check": "website-alive",
    "status": "down",
    "labels": { "env": "production" },
    "info":   { "k8s_namespace": "prod" }
  }
}
```

`type` is `io.vigil.check.failed` on firing, `io.vigil.check.recovered` on recovery.

#### `otlp-logs` — OpenTelemetry OTLP HTTP/JSON

Compatible with `POST /v1/logs` on any OpenTelemetry Collector.
`severityNumber` is `17` (ERROR) on Down and `9` (INFO) on Up.
`labels` and `send_info_fields` are added as LogRecord `attributes`.

### Example

```yaml
alerts:

  # Alertmanager — URL and token from environment, custom retry
  alertmanager-prod:
    url: "env:ALERTMANAGER_URL"
    format: alertmanager
    on-check: [website-alive]
    headers:
      Authorization: "Bearer env:ALERTMANAGER_TOKEN"
    labels:
      severity: critical
      cluster:  "env:CLUSTER_NAME"
    send_info_fields:
      runbook: "https://wiki.example.com/runbooks/website-down"
    retry_attempts: 5
    retry_backoff: ["1s", "2s", "5s", "10s"]

  # OTLP — via corporate proxy
  otlp-prod:
    url: http://otel-collector:4318/v1/logs
    format: otlp-logs
    on-check: [website-alive]
    proxy:    "http://corp-proxy:3128"
    no_proxy: "localhost, .internal, 10.0.0.0/8"
    labels:
      service.name:            vigild
      service.namespace:       "env:KUBERNETES_NAMESPACE"
      deployment.environment:  production

  # Slack — body-template produces the incoming-webhook JSON shape
  slack:
    url: "env:SLACK_WEBHOOK_URL"
    format: webhook
    on-check: [website-alive]
    labels:
      cluster: "env:CLUSTER_NAME"
    body-template: >-
      {"text": ":{% if status == 'down' %}red_circle{% else %}large_green_circle{% endif %} *{{ check }}* is *{{ status }}* on `{{ labels.cluster }}`"}

  # Microsoft Teams — MessageCard format via body-template
  teams:
    url: "env:TEAMS_WEBHOOK_URL"
    format: webhook
    on-check: [website-alive]
    labels:
      cluster: "env:CLUSTER_NAME"
    send_info_fields:
      runbook: "https://wiki.example.com/runbooks/website-down"
    body-template: |
      {
        "@type": "MessageCard",
        "@context": "https://schema.org/extensions",
        "themeColor": "{% if status == 'down' %}FF0000{% else %}00CC00{% endif %}",
        "title": "Vigil Alert — {{ check }}",
        "text": "Check **{{ check }}** is **{{ status }}** on `{{ labels.cluster }}`.",
        "potentialAction": [{
          "@type": "OpenUri",
          "name": "Open Runbook",
          "targets": [{ "os": "default", "uri": "{{ info.runbook }}" }]
        }]
      }
```

---

## Duration format

All duration fields (e.g. `kill-delay`, `period`, `timeout`, `backoff-delay`,
`backoff-limit`, `delay`, `retry_backoff` entries) accept strings like:

| Example | Meaning |
|---|---|
| `500ms` | 500 milliseconds |
| `3s` | 3 seconds |
| `1m` | 1 minute |
| `1m30s` | 1 minute 30 seconds |
| `1h` | 1 hour |

---

## Complete example

```yaml
summary: Production application stack

services:

  postgres:
    command: docker-entrypoint.sh postgres
    startup: enabled
    user: postgres
    environment:
      POSTGRES_DB: myapp
      POSTGRES_PASSWORD_FILE: /run/secrets/db_password
    stop-signal: SIGINT
    kill-delay: 30s
    on-success: restart
    on-failure: restart
    on-check-failure:
      postgres-ready: restart

  myapp:
    command: /usr/local/bin/myapp
    startup: enabled
    requires:               # implies after: ordering + stop cascade if postgres dies
      - postgres
    environment:
      DATABASE_URL: postgres://postgres@localhost/myapp
      LOG_LEVEL: info
    stop-signal: SIGTERM
    kill-delay: 15s
    on-success: restart
    on-failure: restart
    backoff-delay: 2s
    backoff-factor: 2.0
    backoff-limit: 60s
    on-check-failure:
      myapp-alive: restart

checks:

  postgres-ready:
    level: ready
    startup: enabled
    delay: 2s
    period: 5s
    timeout: 2s
    threshold: 3
    exec:
      command: pg_isready -U postgres -d myapp
      service-context: postgres

  myapp-alive:
    level: alive
    startup: enabled
    delay: 5s
    period: 10s
    timeout: 3s
    threshold: 3
    http:
      url: http://localhost:8080/healthz

alerts:

  alertmanager:
    url: http://alertmanager:9093/api/v2/alerts
    format: alertmanager
    on-check: [postgres-ready, myapp-alive]
    labels:
      severity: critical
      env:      production
      cluster:  "env:CLUSTER_NAME"
    send_info_fields:
      k8s_namespace: "env:KUBERNETES_NAMESPACE"
```

---

## Identities

Identities control who may call the vigild REST API and what they are allowed
to do. They are optional — if no identities are configured the daemon applies
default access rules based on connection type (see below).

### Access levels

| Level | Grants access to |
|-------|-----------------|
| `open` | `GET /v1/system-info`, `GET /v1/health` — no authentication required |
| `metrics` | `GET /v1/metrics` (Prometheus/OpenMetrics) |
| `read` | All other `GET` endpoints |
| `write` | `read` + service/check control (`POST /v1/services`, `POST /v1/checks`) |
| `admin` | `write` + identity management (`POST /v1/identities`, `DELETE /v1/identities`) |

### Default access (no identities configured)

| Connection | Effective level |
|------------|----------------|
| Unix socket, UID 0 (root) or daemon UID | `admin` |
| Unix socket, any other UID | `read` |
| TCP / HTTPS (no client cert) | `open` |

### Authentication methods

Three methods may be combined — an identity may use any one of them.

#### Local (Unix-socket peer credentials)

Identified by the caller's Linux UID via `SO_PEERCRED`. Only applies to
connections over the Unix socket.

```yaml
identities:

  ops-user:
    access: admin
    local:
      user-id: 1000   # only UID 1000 — omit to allow any local user

  any-local:
    access: read
    local: {}         # {} = any UID on the socket
```

#### Basic (HTTP Basic Auth + SHA-512-crypt)

Password is verified against a SHA-512-crypt hash (`$6$…`).
Generate a hash with `openssl passwd -6 <password>`.

```yaml
identities:

  deploy:
    access: write
    basic:
      password-hash: "$6$rounds=5000$mysalt$hashedvalue..."
```

The caller supplies the credential in the `Authorization: Basic …` header
(standard HTTP Basic Auth, base64-encoded `username:password`).

```bash
# Using vigil CLI
vigil --url https://vigild.example.com --user deploy:secret services list

# Using curl
curl -u deploy:secret https://vigild.example.com/v1/services
```

#### TLS (mutual TLS — client certificate)

Trusts connections presenting a client certificate signed by the specified CA.

**Prerequisite:** vigild must be started with `--tls-client-ca <CA-PEM>` (or
`VIGIL_TLS_CLIENT_CA`).  This flag enables mTLS on the TLS listener — the server
sends a `CertificateRequest` during the TLS handshake and verifies any presented
certificate against the supplied CA.  Connections without a client certificate
are still accepted but fall through to Open access unless another auth method
matches.

`ca-cert` must be an inline PEM string (a single CA cert, or a chain of
concatenated PEM blocks):

```yaml
identities:

  ci-pipeline:
    access: write
    tls:
      ca-cert: |
        -----BEGIN CERTIFICATE-----
        MIIBxTCCAW...
        -----END CERTIFICATE-----

  prometheus:
    access: metrics
    tls:
      ca-cert: |
        -----BEGIN CERTIFICATE-----
        MIIBxTCC...
        -----END CERTIFICATE-----
```

See [Mutual TLS (mTLS)](../operator-guide/tls.md#mutual-tls-mtls) in the
operator guide for a complete setup walkthrough.

### Full identities example

```yaml
identities:

  # Root/daemon gets admin automatically — this adds a named admin identity
  # for a specific operator UID
  ops:
    access: admin
    local:
      user-id: 1000

  # CI/CD pipeline authenticates with a password
  ci-deploy:
    access: write
    basic:
      password-hash: "$6$rounds=5000$somesalt$longhashvalue..."

  # Prometheus scraper uses a client certificate
  prometheus:
    access: metrics
    tls:
      ca-cert: |
        -----BEGIN CERTIFICATE-----
        MIIBxTCC...
        -----END CERTIFICATE-----
```
