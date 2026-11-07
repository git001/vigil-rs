# REST API Reference

vigild exposes an HTTP/1.1 API over:
- A Unix domain socket (always active)
- An optional TLS TCP listener (`--tls-addr`)

The API follows the shape of Canonical Pebble's API for `/v1/services`,
`/v1/checks` and `/v1/system-info`, but it is not a drop-in replacement:
`/v1/logs` returns a JSON envelope, and service actions return a finished
change instead of an asynchronous one. See "vs. Canonical Pebble" in the
README.

Interactive documentation is available at `/docs` (Swagger UI) and
`/openapi.json` on a running daemon.

---

## Authentication and access levels

vigild enforces per-endpoint access control using named identities. The access
levels are ordered from least to most privileged:

| Level | How it is granted | Required by |
|-------|-------------------|-------------|
| `open` | No credentials needed | `GET /v1/system-info`, `GET /v1/checks` |
| `metrics` | Basic Auth, local UID, or TLS client cert | `GET /v1/metrics` |
| `read` | — | All other `GET` endpoints |
| `write` | — | `POST /v1/services`, `POST /v1/checks/refresh`, `POST /v1/replan` |
| `admin` | — | `POST /v1/vigild`, all `/v1/identities` endpoints |

Each endpoint table below lists its **required level**.

**Callers without a matching identity:**

| Connection | Access |
|---|---|
| TLS listener | `--tls-anonymous-access` (`metrics` by default, or `open`) |
| Unix socket, root or vigild's UID, **no identities configured yet** | `admin` (bootstrap: add the first identity this way, e.g. via `kubectl exec`) |
| Unix socket, any other UID, or once identities exist | `open` |

Reading logs or services and anything that changes state over TLS always
requires an identity. Before v4, every caller (including TLS) was `admin`
while the identity store was empty.

**Authentication methods:**

- **Local (Unix socket):** the caller's Unix UID is looked up in the identity
  store. Omit `user-id` in the identity to allow any local user.
- **Basic Auth:** `Authorization: Basic <base64(user:pass)>` — password is
  verified against a SHA-512-crypt hash (`$6$...`).
- **TLS client certificate:** the client cert is verified against the CA stored
  in the identity.

Unauthenticated requests are treated as `open` level. Requests that don't meet
the required level receive `403 Forbidden`.

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
  "status-code": 500,
  "status": "Internal Server Error",
  "message": "human-readable error description"
}
```

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
    "version": "1.0.0",
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
| `replan` | Alias for `POST /v1/replan` |

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

**Change status values:** `doing` | `done` | `error` | `hold`

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

**Check status values:** `up` | `down`

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
  -d '{"name": "db-ready"}' http://localhost/v1/checks/refresh
```

---

### `GET /v1/logs`

**Required access level:** `read`

Return recent log output from service stdout/stderr.

**Query parameters:**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `services` | string | all | Comma-separated service names to filter |
| `n` | integer | 100 | Number of most-recent lines to return |

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

---

### `GET /v1/logs/follow`

**Required access level:** `read`

Stream live log output as Server-Sent Events (SSE).

**Query parameters:**

| Parameter | Type | Description |
|---|---|---|
| `services` | string | Comma-separated service names to filter. Omit for all. |

**Request headers:**

```
Accept: text/event-stream
```

**Response** `200 OK` — `text/event-stream`

Each SSE event carries a JSON-encoded `LogEntry` in its `data` field:

```
data: {"timestamp":"2026-03-18T10:00:05.123Z","service":"myapp","stream":"stdout","message":"hello"}

: ping

data: {"timestamp":"2026-03-18T10:00:15.456Z","service":"myapp","stream":"stdout","message":"world"}
```

Keep-alive comments (`: ping`) are sent every 15 seconds.

**Example:**

```bash
# Follow all logs
curl --unix-socket /run/vigil/vigild.sock \
  -H "Accept: text/event-stream" \
  http://localhost/v1/logs/follow

# Follow myapp only
curl --unix-socket /run/vigil/vigild.sock \
  -H "Accept: text/event-stream" \
  "http://localhost/v1/logs/follow?services=myapp"
```

---

### `POST /v1/replan`

**Required access level:** `write`

Hot-reload all layer YAML files from disk and apply changes.

After replan:
- New services with `startup: enabled` are started.
- Services removed from the plan are stopped.
- Configuration changes take effect on the next restart of each service.

**Request body:** empty or `null`

**Response** `200 OK`

```json
{ "type": "sync", "status-code": 200, "status": "OK", "result": null }
```

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

Metrics exposed:

| Metric | Type | Description |
|---|---|---|
| `vigil_services_count` | gauge | Total number of configured services |
| `vigil_service_info{service}` | gauge (always 1) | Enumerates all configured service names |
| `vigil_service_active{service}` | gauge | `1` if service is active, `0` otherwise |
| `vigil_service_running{service}` | gauge | `1` while a process of the service is alive (`starting`, `active` or stopping), `0` otherwise; unlike `vigil_service_active` also `1` for a running `oneshot` job or a service still waiting for readiness |
| `vigil_service_last_exit_code{service}` | gauge | Exit code of the last process (`128 + signal` if killed by a signal, `-1` if unknown); absent until the service has exited once |
| `vigil_service_last_run_timestamp_seconds{service}` | gauge | Unix time when the last run of a `type: oneshot` service ended |
| `vigil_service_last_run_success{service}` | gauge | `1` if the last run of a `type: oneshot` service succeeded, else `0` |
| `vigil_service_runs_total{service,result}` | counter | Finished runs of a `type: oneshot` service; `result` is `success` or `failure` |
| `vigil_service_file_reloads_total{service}` | counter | Reactions (restart or signal) to a change of a watched file (`watch-files`) |
| `vigil_layers_reloads_total{result}` | counter | Layer reloads (`vigil replan` or `--watch-layers`), `result` is `ok` or `invalid` |
| `vigil_layers_last_reload_timestamp_seconds` | gauge | Unix time of the last successful layer reload (0 = none since start) |
| `vigil_service_start_count_total{service}` | counter | Number of times the service has started |
| `vigil_check_up{check}` | gauge | `1` if check is up, `0` if down |
| `vigil_check_success_count_total{check}` | counter | Number of successful check runs |
| `vigil_check_failure_count_total{check}` | counter | Number of failed check runs |

**Example:**

```bash
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/metrics
```

Sample output:

```
# HELP vigil_services_count Total number of configured services
# TYPE vigil_services_count gauge
vigil_services_count 2
# HELP vigil_service_info Service metadata — label enumerates all configured service names
# TYPE vigil_service_info gauge
vigil_service_info{service="haproxy"} 1
vigil_service_info{service="controller"} 1
# HELP vigil_service_active Whether the service is currently active (1) or not (0)
# TYPE vigil_service_active gauge
vigil_service_active{service="haproxy"} 1
vigil_service_active{service="controller"} 1
# HELP vigil_service_start_count_total Number of times the service has started
# TYPE vigil_service_start_count_total counter
vigil_service_start_count_total{service="haproxy"} 1
vigil_service_start_count_total{service="controller"} 1
# HELP vigil_check_up Whether the health check is up (1) or not (0)
# TYPE vigil_check_up gauge
vigil_check_up{check="check-haproxy"} 1
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
      "local": { "user-id": 1000 }
    },
    {
      "name": "ci-bot",
      "access": "read",
      "local": {}
    }
  ]
}
```

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
  `ca-cert` is a PEM-encoded CA certificate string or file path.

**Response** `200 OK`

```json
{ "type": "sync", "status-code": 200, "status": "OK", "result": null }
```

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
| `GET /openapi.json` | OpenAPI 3.0 specification (JSON) |
