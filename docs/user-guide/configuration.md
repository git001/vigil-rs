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

Startup ordering constraints. All referenced service names must exist in the
merged plan.

- `after: [a, b]` — this service starts after `a` and `b` reach `Active`
- `before: [c]` — this service starts before `c` (i.e. `c` has `after: [this]`)
- `requires: [a]` — if `a` is not `Active` when this service starts, this
  service transitions to `Error`

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

Format for pushed log lines. Currently `ndjson` (default and only option):
one JSON object per line, same schema as `GET /v1/logs/follow?format=ndjson`.

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
      url: http://localhost:8080/healthz
      headers:
        Authorization: "Bearer secret"

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
  url: http://localhost:8080/healthz
  headers:
    X-Health-Token: secret
```

The check passes if the HTTP response status is 2xx. The `url` field supports
`http://` and `https://` (TLS verification is skipped for localhost).

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

## Duration format

All duration fields (e.g. `kill-delay`, `period`, `timeout`, `backoff-delay`,
`backoff-limit`, `delay`) accept strings like:

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
    after:
      - postgres
    requires:
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
      ca-cert: /etc/vigil/certs/monitoring-ca.pem   # path also accepted
```

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
