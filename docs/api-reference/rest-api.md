# REST API Reference

vigild exposes an HTTP/1.1 API over:
- A Unix domain socket (always active)
- An optional TLS TCP listener (`--tls-addr`)

Interactive documentation is available at `/docs` (Swagger UI) and
`/openapi.json` (OpenAPI 3.1) on a running daemon; the same spec is in the
repository as [docs/specs/openapi.yaml](../specs/openapi.yaml).

Requests are strict: a request body with a field vigild does not know is
`422`, a query parameter it does not know is `400`.

---

## Authentication and access levels

vigild enforces per-endpoint access control using named identities. The access
levels are ordered from least to most privileged; each includes the ones
above it:

| Level | Required by |
|-------|-------------|
| `open` | `GET /v1/system-info`, `GET /v1/checks`, `GET /v1/health`, `/docs`, `/openapi.json` |
| `metrics` | `GET /v1/metrics` |
| `read` | `GET /v1/services`, `/v1/changes/{id}`, `/v1/alerts`, `/v1/logs`, `/v1/logs/follow` |
| `write` | `POST /v1/services`, `POST /v1/signals`, `POST /v1/checks/refresh`, `POST /v1/replan` |
| `admin` | `POST /v1/vigild`, all `/v1/identities` endpoints (also `GET`) |

Each endpoint table below lists its **required level**.

**Callers without a matching identity:**

| Connection | Access |
|---|---|
| TLS listener | `--tls-anonymous-access` (`metrics` by default, or `open`) |
| Unix socket, root or vigild's UID, **no identity added over the API yet** | `admin` (bootstrap: add the first identity this way, e.g. via `kubectl exec`). Identities from layer files do not end the bootstrap |
| Unix socket, any other UID, or once an identity was added over the API | `open` |

Reading logs or services and anything that changes state over TLS always
requires an identity. Before v4, every caller (including TLS) was `admin`
while the identity store was empty.

**Authentication methods:**

1. **Basic Auth:** `Authorization: Basic <base64(user:pass)>` — password is
   verified against a SHA-512-crypt hash (`$6$...`).
2. **TLS client certificate:** the client cert is verified against the CA
   stored in the identity; the highest matching level counts.
3. **Local (Unix socket):** the caller's Unix UID is looked up in the
   identity store. Omit `user-id` in the identity to allow any local user.

The methods are tried in this order, and the first matching identity
counts. A caller gets the higher of that identity's level and the level in
the table above, so a TLS caller with a `metrics` identity and the default
`--tls-anonymous-access` has `metrics`. Requests that don't meet the
required level receive `403 Forbidden`. The access check comes first: a
caller without access gets `403` even for a request that is invalid
otherwise.

---

## Response envelope

All responses use a standard JSON envelope:

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": { ... }
}
```

On error:

```json
{
  "type": "error",
  "status-code": 422,
  "status": "Unprocessable Entity",
  "result": null,
  "message": "Failed to deserialize the JSON body into the target type: service: unknown field `service`, expected `signal` or `services` at line 1 column 28"
}
```

Every error uses this envelope. Status codes used by all endpoints:

| Code | Meaning |
|---|---|
| `400` | Invalid JSON body, unknown query parameter, or an invalid value |
| `403` | The caller's access level is too low |
| `415` | A request body without `Content-Type: application/json` |
| `422` | A request body with an unknown field or a value of the wrong type |
| `500` | Internal error (and an invalid layer on `POST /v1/replan`) |

Endpoint-specific codes (`404`, `409`, `502`) are listed with the endpoints.

---

## Endpoints

### `GET /v1/system-info`

**Required access level:** `open` (no credentials needed)

Returns daemon version, boot ID, start time, and API addresses.

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": {
    "boot-id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "http-address": "/run/vigil/vigild.sock",
    "https-address": "0.0.0.0:8443",
    "version": "4.0.0",
    "start-time": "2026-03-18T08:00:00.000Z"
  }
}
```

`https-address` is omitted when the TLS listener is not configured.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/system-info
curl -k https://host:8443/v1/system-info
```

---

### `GET /v1/services`

**Required access level:** `read`

List services and their current status.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `names` | string | Comma-separated list of service names to filter. Omit for all. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": [
    {
      "name": "myapp",
      "startup": "enabled",
      "current": "active",
      "current-since": "2026-03-18T10:00:05.123Z",
      "stop-signal": "SIGTERM",
      "on-success": "restart",
      "on-failure": "restart",
      "last-exit": { "code": 3 }
    }
  ]
}
```

`last-exit` describes how the service's last process ended: `{"code": N}` for
a normal exit, `{"signal": N}` if it was killed by a signal. It is omitted
until the service has exited once. The values are raw (before
`exit-code-map`).

`status-message` is the last `STATUS=` text the service sent via sd_notify
(`ready: notify`); it is omitted if there is none and cleared on every start.

`next-run` is the next scheduled run of a `type: oneshot` service with a
`schedule` (only while the schedule is armed, i.e. `startup: enabled`).

`waiting-for` lists the dependencies (`after`, `before`, `requires`) a
service queued by autostart still waits for; it is omitted otherwise.

**Service status values:** `starting` | `active` | `inactive` | `backoff` |
`error` | `done`. `done` (new in v4) means a `type: oneshot` service finished
successfully. `starting` (new in v4) means the process runs but is not ready yet
(`ready: notify` or `ready: check`); clients written for v3 must accept it.

**Example:**

```bash
# All services
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/services

# Filtered
curl --unix-socket /run/vigil/vigild.sock \
  "http://localhost/v1/services?names=myapp,sidecar"
```

---

### `POST /v1/services`

**Required access level:** `write`

Perform an action on one or more services.

**Request body:**

```json
{
  "action": "start",
  "services": ["myapp", "sidecar"]
}
```

Pass an empty `services` array to act on all services.

**Actions:**

| Action | Description |
|---|---|
| `start` | Start the specified services |
| `stop` | Stop the specified services |
| `restart` | Stop then start the specified services |
| `autostart` | Start the services vigild starts at startup: `startup: enabled`, not scheduled, no oneshot that has already run; each once its `after` dependencies are ready. With `services`, only those; a listed service that autostart does not cover (disabled, scheduled, unknown) is reported in `err`. The plan is not changed. |
| `replan` | Reload the layers once, like `POST /v1/replan`; `services` is ignored. Unlike `POST /v1/replan`, an invalid layer is reported in the change (`status: error`, `err`), not as `500` |

`services` is required (`[]` for all).

**Response** `200 OK` — a change record. vigild performs the action before
it answers, so `status` is `done`, or `error` with the reason in `err`. A
problem with single services (an unknown name, a failed start) is such an
`error`, still with HTTP `200`.

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": {
    "id": "42",
    "kind": "start",
    "summary": "Start [\"myapp\"]",
    "status": "done",
    "spawn-time": "2026-03-18T10:01:00.000Z",
    "ready-time": "2026-03-18T10:01:00.250Z",
    "err": null
  }
}
```

```json
{
  "id": "43",
  "kind": "start",
  "summary": "Start [\"web\", \"nope\"]",
  "status": "error",
  "spawn-time": "2026-03-18T10:02:00.000Z",
  "ready-time": "2026-03-18T10:02:00.000Z",
  "err": "nope: unknown service 'nope'"
}
```

**Example:**

```bash
# Start a service
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d '{"action":"start","services":["myapp"]}' \
  http://localhost/v1/services

# Restart all services
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d '{"action":"restart","services":[]}' \
  http://localhost/v1/services
```

---

### `POST /v1/signals`

**Required access level:** `write`

Send a signal to the process groups of running services, e.g. `SIGHUP` to
make nginx re-read its configuration without a restart. Every named service
must exist and have a running process (`starting`, `active` or
`stopping`); otherwise none of them gets the signal.

**Request body:**

```json
{"signal": "SIGHUP", "services": ["nginx"]}
```

| Field | Type | Description |
|---|---|---|
| `signal` | string | Signal name as in `stop-signal`: `SIGHUP`, `SIGUSR1`, … (`HUP` alone is rejected). |
| `services` | array | The services; at least one. Unknown fields are rejected. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": {"signal": "SIGHUP", "services": ["nginx"]}
}
```

**Errors:** `400` unknown signal or empty `services`; `404` a name that is
not a service (`service "ngnix" not found`); `409` a service without a
running process (`not running: "php-fpm"`).

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock -X POST \
  -H "content-type: application/json" \
  -d '{"signal": "SIGHUP", "services": ["nginx"]}' http://localhost/v1/signals
```

---

### `GET /v1/changes/{id}`

**Required access level:** `read`

Get a specific change record by ID.

**Path parameters:**

| Parameter | Description |
|---|---|
| `id` | Change ID (returned by `POST /v1/services`) |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": {
    "id": "42",
    "kind": "start",
    "summary": "Start [\"myapp\"]",
    "status": "done",
    "spawn-time": "2026-03-18T10:01:00.000Z",
    "ready-time": "2026-03-18T10:01:00.250Z"
  }
}
```

**Change status values:** `done` | `error`. The schema also has `doing` and
`hold`, but vigild never returns them: an action is finished when the
response is sent. Changes are kept in memory only.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/changes/42
```

---

### `GET /v1/checks`

**Required access level:** `open` (so Kubernetes `httpGet` probes work without
credentials; the response only contains check names and states)

List health checks and their current status.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `names` | string | Comma-separated check names. Omit for all. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": [
    {
      "name": "myapp-alive",
      "level": "alive",
      "status": "up",
      "failures": 0,
      "threshold": 3,
      "next-run-in-secs": 7
    },
    {
      "name": "db-ready",
      "level": "ready",
      "status": "down",
      "failures": 3,
      "threshold": 3,
      "next-run-in-secs": 2,
      "last-error": "connect localhost:5432: Connection refused (os error 111)"
    }
  ]
}
```

**Check status values:** `up` | `down` | `inactive`. `inactive` is a check
with `startup: disabled`: it does not run, has no failures and no
`next-run-in-secs`.

`next-run-in-secs` is absent during the initial `delay`. `last-error` is why
the last run failed, until a run passes; see [Why a check
fails](../user-guide/configuration.md#why-a-check-fails).

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/checks
curl --unix-socket /run/vigil/vigild.sock \
  "http://localhost/v1/checks?names=myapp-alive"
```

---

### `GET /v1/health`

**Required access level:** `open` (for Kubernetes `httpGet` probes, which
send no credentials)

vigild's health endpoint: `200` while none of the selected checks is down,
`502 Bad Gateway` otherwise. Only running checks count; a check with
`startup: disabled` is ignored. A check during its initial `delay` is up.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `level` | `alive` \| `ready` | `alive`: only checks with `level: alive`. `ready`: checks of both levels. Omit for all checks. |
| `names` | string | Comma-separated check names. Omit for all. |

A name that is not a check is `404 Not Found` (a typo in a probe would
otherwise pass for ever), and an unknown parameter or level is
`400 Bad Request`.

**While shutting down:** with `--shutdown-delay`, vigild keeps its
services running for a while after SIGTERM and answers `503 Service
Unavailable` (`"healthy": false`, message `vigild is shutting down`) for
`level=ready` and without `level`, so the readiness probe takes the pod
out of the load balancer first. `level=alive` keeps reporting the checks:
the liveness probe must not restart a container that is stopping anyway.

**Response** `200 OK`, `502 Bad Gateway` or `503 Service Unavailable`

```json
{
  "type": "sync",
  "status-code": 502,
  "status": "Bad Gateway",
  "result": {"healthy": false}
}
```

**Example:**

```bash
curl -sk -o /dev/null -w '%{http_code}\n' "https://localhost:8443/v1/health?level=ready"
```

---

### `POST /v1/checks/refresh`

**Required access level:** `write`

Run a check now and return its state after the run. The run counts like a
scheduled one: failures, `threshold`, status and alerts. A check with
`startup: disabled` runs once without changing any state; the result then
shows only that run.

**Request body:**

```json
{"name": "db-ready"}
```

| Field | Type | Description |
|---|---|---|
| `name` | string | The check to run. Unknown fields are rejected. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": {
    "info": {
      "name": "db-ready",
      "level": "ready",
      "status": "down",
      "failures": 3,
      "threshold": 3,
      "next-run-in-secs": 7,
      "last-error": "connect localhost:5432: Connection refused (os error 111)"
    },
    "error": "connect localhost:5432: Connection refused (os error 111)"
  }
}
```

`error` is why this run failed; it is absent when the run passed. An unknown
check is `404 Not Found`.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock -X POST \
  -H "content-type: application/json" \
  -d '{"name": "db-ready"}' http://localhost/v1/checks/refresh
```

---

### `GET /v1/logs`

**Required access level:** `read`

Return recent log output from service stdout/stderr. With `follow=true`, keep
the connection open and stream new lines as well.

**Query parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `services` | string | all | Comma-separated service names to filter |
| `n` | integer | 100 | Number of most-recent lines to return |
| `follow` | boolean | `false` | `true`: stream the last `n` lines, then each new line, as NDJSON (see below) |

Any other query parameter is `400 Bad Request`, for example `format`, which
belongs to `/v1/logs/follow`.

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": [
    {
      "timestamp": "2026-03-18T10:00:05.123Z",
      "service": "myapp",
      "stream": "stdout",
      "message": "server listening on :8080"
    }
  ]
}
```

**Stream values:** `stdout` | `stderr`

**Example:**

```bash
# Last 100 lines from all services
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/logs

# Last 20 lines from myapp
curl --unix-socket /run/vigil/vigild.sock \
  "http://localhost/v1/logs?services=myapp&n=20"
```

**With `follow=true`:** `200 OK` with `Content-Type: application/x-ndjson`,
no envelope: one `LogEntry` JSON object per line, first the last `n` lines
(`n=0` for none), then each new line until the client closes the connection.
The same as `/v1/logs/follow?format=ndjson`, plus the lines from before the
request.

```
{"timestamp":"2026-03-18T10:00:05.123Z","service":"myapp","stream":"stdout","message":"server listening on :8080"}
{"timestamp":"2026-03-18T10:00:09.876Z","service":"myapp","stream":"stderr","message":"GET /health 200"}
```

```bash
# The last 10 lines of myapp, then follow (Ctrl+C to stop)
curl -N --unix-socket /run/vigil/vigild.sock \
  "http://localhost/v1/logs?services=myapp&n=10&follow=true"
```

---

### `GET /v1/logs/follow`

**Required access level:** `read`

Stream live log output: only lines logged after the request. For the lines
before it as well, use `GET /v1/logs?follow=true`.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `services` | string | Comma-separated service names to filter. Omit for all. |
| `format` | `json` \| `text` \| `ndjson` | `json` (default): SSE, each event a `LogEntry`. `text`: SSE, each event `[service] message`. `ndjson`: `application/x-ndjson`, one `LogEntry` per line, no SSE framing, no keep-alives |

Any other parameter, or another `format`, is `400 Bad Request`.

**Response** `200 OK` — `text/event-stream` (`application/x-ndjson` with
`format=ndjson`)

With `format=json` each SSE event carries a JSON-encoded `LogEntry` in its
`data` field:

```
data: {"timestamp":"2026-03-18T10:00:05.123Z","service":"myapp","stream":"stdout","message":"hello"}

: ping

data: {"timestamp":"2026-03-18T10:00:15.456Z","service":"myapp","stream":"stdout","message":"world"}
```

Keep-alive comments (`: ping`) are sent every 15 seconds. A client that
falls too far behind skips the entries it missed; the SSE formats mark the
gap with a comment `: lagged: skipped N entries` (see
[Logging](../user-guide/logging.md#slow-consumer--lagged)).

**Example:**

```bash
# Follow all logs (SSE)
curl -N --unix-socket /run/vigil/vigild.sock \
  http://localhost/v1/logs/follow

# Follow myapp only, as ndjson
curl -N --unix-socket /run/vigil/vigild.sock \
  "http://localhost/v1/logs/follow?services=myapp&format=ndjson"
```

---

### `POST /v1/replan`

**Required access level:** `write`

Hot-reload all layer YAML files from disk and apply changes.

After replan:
- New services with `startup: enabled` are started.
- Services removed from the plan are stopped.
- A running service (`starting`, `active` or `backoff`) whose configuration
  changed is restarted with the new one: stopped, then started again. The
  running services that need it via `requires` are restarted too, each once
  the service it requires is ready again. A change of only `summary` or
  `description` restarts nothing. The log line `plan reloaded` lists the
  restarted services (`restarted=["db", "api"]`).
- A stopped service gets the new configuration at its next start. A
  scheduled job re-arms a changed `schedule` at once; a run in progress
  finishes with the old configuration.

**Request body:** none

**Response** `200 OK`

```json
{ "type": "sync", "status-code": 200, "status": "OK", "result": null }
```

An invalid layer (a YAML error, an unknown key, a failed validation) is
`500` with the reason in `message`; the running plan stays unchanged.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock \
  -X POST http://localhost/v1/replan
```

---

### `GET /v1/metrics`

**Required access level:** `metrics`

Return Prometheus/OpenMetrics metrics in text exposition format.

**Response** `200 OK` — `application/openmetrics-text; version=1.0.0; charset=utf-8`

Every metric, with PromQL examples: [Metrics](../operator-guide/metrics.md).
Check metrics carry the labels `check` and `level`; `vigil_health{level}`
says what `GET /v1/health?level=` answers.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/metrics
```

Sample output (excerpt; OpenMetrics names a counter family without
`_total` and ends the output with `# EOF`):

```
# HELP vigil_service_start_count Number of times the service has started.
# TYPE vigil_service_start_count counter
vigil_service_start_count_total{service="haproxy"} 1
vigil_service_start_count_total{service="controller"} 1
# HELP vigil_service_active Whether the service is currently active (1) or not (0).
# TYPE vigil_service_active gauge
vigil_service_active{service="haproxy"} 1
vigil_service_active{service="controller"} 1
# HELP vigil_check_up Whether the health check is up (1) or not (0).
# TYPE vigil_check_up gauge
vigil_check_up{check="check-haproxy",level="alive"} 1
# HELP vigil_health Whether GET /v1/health?level= answers healthy (1) or not (0).
# TYPE vigil_health gauge
vigil_health{level="alive"} 1
vigil_health{level="ready"} 1
# EOF
```

---

### `GET /v1/alerts`

**Required access level:** `read`

List the alerts of the plan, what they are subscribed to, and the last
status of the checks they watch.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `names` | string | Comma-separated alert names. Omit for all. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": [
    {
      "name": "ops-hook",
      "url": "env:OPS_WEBHOOK_URL",
      "format": "webhook",
      "on-check": ["web-alive"],
      "on-service": ["web"],
      "on-vigild": ["layers"],
      "check-status": [{"check": "web-alive", "status": "down"}]
    }
  ]
}
```

`url` is the value as written in the layer (an `env:VAR` reference is not
resolved). `format` is `webhook`, `alertmanager`, `cloud-events` or
`otlp-logs`. `on-service` and `on-vigild` are omitted when empty.
`check-status` has one entry per `on-check` check; `status` is `null` until
the check reported a first event.

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/alerts
```

---

### `POST /v1/vigild`

**Required access level:** `admin`

Control the vigild daemon itself.

**Request body:**

```json
{ "action": "stop" }
```

or

```json
{ "action": "restart" }
```

**Actions:**

| Action | Description |
|---|---|
| `stop` | Gracefully stop all services and exit the daemon |
| `restart` | Stop all services and re-execute vigild in-place (same PID, new boot-id) |

The response is sent before the daemon begins shutting down.

**Response** `200 OK`

```json
{ "type": "sync", "status-code": 200, "status": "OK", "result": null }
```

**Example:**

```bash
# Stop the daemon
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d '{"action":"stop"}' \
  http://localhost/v1/vigild

# Restart the daemon in-place
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d '{"action":"restart"}' \
  http://localhost/v1/vigild
```

---

### `GET /v1/identities`

**Required access level:** `admin`

List named identities.

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `names` | string | Comma-separated names to filter. Omit for all. |

**Response** `200 OK`

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": [
    {
      "name": "ops-user",
      "access": "admin",
      "local": { "user-id": 1000 },
      "source": "api"
    },
    {
      "name": "log-relay",
      "access": "read",
      "basic": { "password-hash": "$6$6lRcqNf3pZIIeBI6$kJXN…" },
      "source": "layer",
      "layer": "001-auth"
    }
  ]
}
```

`source` is `api` (added with `POST /v1/identities`, lost on restart) or
`layer` (from a layer file; `layer` names the file without extension).
Password hashes are returned, never passwords; this is why the endpoint
needs `admin`.

---

### `POST /v1/identities`

**Required access level:** `admin`

Add or update identities.

**Request body:**

```json
{
  "identities": {
    "ops-user": {
      "access": "admin",
      "local": { "user-id": 1000 }
    },
    "ci-deploy": {
      "access": "write",
      "basic": { "password-hash": "$6$rounds=5000$salt$hash..." }
    },
    "prometheus": {
      "access": "metrics",
      "tls": { "ca-cert": "-----BEGIN CERTIFICATE-----\n..." }
    }
  }
}
```

**Access levels:** `open` | `metrics` | `read` | `write` | `admin`

**Auth types:**

- `local` — matches Unix socket connections by caller UID.
  `user-id` is optional; omit to allow any local user.
- `basic` — HTTP Basic Auth. `password-hash` must be a SHA-512-crypt string
  (`$6$...`). Generate with `openssl passwd -6 <password>`.
- `tls` — matches TLS connections by client certificate CA.
  `ca-cert` is the PEM-encoded CA certificate itself (in a layer file,
  `tls.ca` names a file instead).

An existing identity of the same name is replaced.

**Response** `200 OK`

```json
{ "type": "sync", "status-code": 200, "status": "OK", "result": null }
```

**Errors:** `409` when a name belongs to an identity from a layer file
(read-only over the API); the message names the layer file.

**Example — local identity:**

```bash
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d '{"identities":{"ops":{"access":"admin","local":{"user-id":1000}}}}' \
  http://localhost/v1/identities
```

**Example — basic auth identity:**

```bash
HASH=$(openssl passwd -6 mysecretpassword)
curl --unix-socket /run/vigil/vigild.sock \
  -X POST -H "content-type: application/json" \
  -d "{\"identities\":{\"deploy\":{\"access\":\"write\",\"basic\":{\"password-hash\":\"$HASH\"}}}}" \
  http://localhost/v1/identities
```

---

### `DELETE /v1/identities`

**Required access level:** `admin`

Remove identities by name.

**Request body:**

```json
{
  "identities": ["ci-bot", "old-user"]
}
```

Unknown names are skipped. `409` when a name belongs to an identity from a
layer file; then nothing is removed.

**Response** `200 OK` — returns the list of actually removed names:

```json
{
  "type": "sync",
  "status-code": 200,
  "status": "OK",
  "result": ["ci-bot"]
}
```

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock \
  -X DELETE -H "content-type: application/json" \
  -d '{"identities":["ci-bot"]}' \
  http://localhost/v1/identities
```

---

## Non-API endpoints

| Path | Description |
|---|---|
| `GET /docs` | Swagger UI (interactive API explorer) |
| `GET /openapi.json` | OpenAPI 3.1 specification (JSON); also in the repository as [openapi.yaml](../specs/openapi.yaml) |
