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
| `--socket <PATH>` | `VIGIL_SOCKET` | `<path-prefix>/vigild.sock` | Unix socket path, **at most 107 bytes** ([socket path limit](installation.md#socket-path-limit-107-bytes)). Ignored when `--url` is set. |
| `--socket-uid <UID>` | `VIGIL_SOCKET_UID` | — | Also trust a daemon socket owned by this UID. By default `vigil` only talks to a vigild running as root or as your own UID and refuses any other listener. |
| `--url <URL>` | `VIGIL_URL` | — | Base URL of vigild's HTTPS API (e.g. `https://host:8443`). Overrides `--socket`. vigild has no plain HTTP listener; `http://` only makes sense through a proxy or tunnel that adds TLS. |
| `-k`, `--insecure` | `VIGIL_INSECURE` | false | Skip TLS certificate verification. Only effective with `--url https://...`. The variable takes `true` or `false`. |
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
| `-h`, `--help` | Print help. |
| `-V`, `--version` | Print version. |

### Transport selection

`--url` takes precedence over `--socket`. Both env vars can coexist; `VIGIL_URL` wins.

```bash
# Unix socket (default — inside the container)
vigil services list

# Explicit socket path
vigil --socket /run/vigil/vigild.sock services list

# HTTPS with self-signed cert (skip verification); without credentials only
# the open endpoints work (checks, health, system-info)
vigil --url https://10.0.0.5:8443 --insecure checks list

# HTTPS with a trusted server CA and Basic Auth
vigil --url https://vigild.internal:8443 --cacert /etc/vigil/ca.pem \
      --user deploy:secret services list

# HTTP Basic Auth
vigil --url https://10.0.0.5:8443 --insecure --user deploy:secret services list

# mTLS client certificate
vigil --url https://10.0.0.5:8443 --insecure \
      --cert /etc/vigil/client.crt \
      --key  /etc/vigil/client.key \
      services list

# HTTPS through a corporate MITM proxy
vigil --url https://vigild.internal:8443 --user deploy:secret \
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
version:      4.0.0
boot-id:      77984f0e-1f78-421b-b941-dc08b61f45d0
http-address: /run/vigil/vigild.sock
```

With `--tls-addr` an `https-address` line follows.

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

A service that is being stopped shows `inactive` already, while its process
may still run until `kill-delay`; `vigil services stop` returns once it is
gone.

#### `services start [NAME...]`

Start one or more services (empty = all). The command returns once they
are ready (`active`), a oneshot once its run has ended; a service that does
not get there is reported with the reason and exit code 1.

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
The command returns once the processes are gone.

#### `services restart [NAME...]`

Restart one or more services. Equivalent to stop followed by start; returns
once they are ready again.

```bash
vigil services restart myapp
vigil services restart        # restart all
```

#### `services signal SIGNAL NAME...`

Send a signal to the process groups of running services
(`POST /v1/signals`), e.g. to make nginx re-read its configuration without a
restart. The signal name is written as in `stop-signal`: `SIGHUP`,
`SIGUSR1`, `SIGUSR2`, …

```bash
vigil services signal SIGHUP nginx
vigil services signal SIGUSR1 haproxy php-fpm
```

```
Sent SIGHUP to nginx.
```

Every named service must be running (`starting`, `active` or `stopping`);
otherwise none of them gets the signal and the command fails:

```
Error: not running: "php-fpm"
```

A signal to vigild itself (`SIGHUP`, `SIGUSR1`, `SIGUSR2`) still goes to all
running services, see [Signal
forwarding](../operator-guide/container-setup.md#signal-forwarding).

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
Check                    Level    Status   Failures   Next run   Last error
--------------------------------------------------------------------------------------------
myapp-alive              alive    up       0/3        7s
postgres-ready           ready    down     3/3        2s         exit status 2: localhost:5432 - no response
debug-probe              alive    inactive 0/3        -
```

**Status values:**

| Status | Meaning |
|---|---|
| `up` | Healthy |
| `down` | Failed `threshold` times in a row |
| `inactive` | `startup: disabled` (the default for checks): the check does not run. `vigil checks run` still runs it once |

**Next run:** time until the next run; `pending` during the initial `delay`,
`-` for an inactive check.

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
log ring buffer (`GET /v1/logs`); `-f` then streams new lines from
`GET /v1/logs/follow` (SSE).

```bash
vigil logs                      # last 100 lines, all services
vigil logs myapp -n 50          # last 50 lines from myapp
vigil logs -f                   # follow live stream (Ctrl+C to stop)
vigil logs -f myapp sidecar     # follow specific services
```

**Flags:**

| Flag | Description |
|---|---|
| `-n`, `--lines <N>` | Number of buffered lines to show (default: 100) |
| `-f`, `--follow` | Stream new lines after showing the buffered ones, until Ctrl+C. |

Each line is formatted as (UTC):

```
2026-11-08 21:57:22.412 [web] [stdout] started
2026-11-08 21:57:22.413 [job] [stderr] oops
```

> Services with `logs-forward: passthrough` bypass the ring buffer
> entirely — their output will not appear in `vigil logs`.

See [Logging](logging.md) for details on `logs-forward`, buffer sizing, and
the raw SSE API.

---

### `plan`

Show the plan merged from all layers as one layer file (YAML), with secrets
masked as `***` (`GET /v1/plan`, access `read`; the rules are in the
[REST API reference](../api-reference/rest-api.md#get-v1plan)).

```bash
vigil plan
```

```yaml
services:
  web:
    command: /usr/local/bin/web
    startup: enabled
    environment:
      DB_PASSWORD: '***'
alerts:
  ops:
    url: https://hooks.slack.com/***
    on-check:
    - '*'
```

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
- A running service (`starting`, `active` or `backoff`) whose configuration
  changed is restarted with the new one: stopped, then started again. The
  running services that need it via `requires` are restarted too, each once
  the service it requires is ready again. A change of only `summary` or
  `description` restarts nothing. The log line `plan reloaded` lists the
  restarted services (`restarted=["db", "api"]`).
- A stopped service gets the new configuration at its next start. A
  scheduled job re-arms a changed `schedule` at once; a run in progress
  finishes with the old configuration.

---

### `alerts`

#### `alerts list [NAME...]`

List the alerts of the plan, the status of the checks they watch, and what
they are subscribed to (`GET /v1/alerts`).

```bash
vigil alerts list
vigil alerts list ops-hook
```

Example output:

```
Alert                    Format         Status   On
----------------------------------------------------------------------
ops-hook                 webhook        down     web-alive, service:web
audit                    cloudevents    unknown  vigild:layers
```

**Status:** `down` when one of the watched checks is down, `unknown` when
none of them has reported yet (or the alert watches no check), otherwise
`up`. **On:** the checks (`on-check`), services (`service:NAME`) and
vigild events (`vigild:EVENT`) the alert fires on. See
[Alerts](configuration.md#alerts).

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
version:      4.0.0
boot-id:      77984f0e-1f78-421b-b941-dc08b61f45d0
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

Each level includes everything of the levels above it.

| Level | Adds |
|-------|------|
| `open` | `GET /v1/checks`, `/v1/health`, `/v1/system-info`, `/docs`, `/openapi.json` — no credentials needed |
| `metrics` | `GET /v1/metrics` |
| `read` | The other `GET` endpoints: services, logs, alerts, changes |
| `write` | Changes: service actions, `replan`, signals, `checks/refresh` |
| `admin` | `POST /v1/vigild` (stop, restart) and `/v1/identities` (list, add, remove) |

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

Add or update a local (Unix-socket UID) identity. `--access` defaults to
`read`; without `--uid` any local user matches.

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

At least one name is required.

```bash
vigil identities remove ci-bot
vigil identities remove ci-bot ops-user
```

---

## `vigil-log-relay`

`vigil-log-relay` is a standalone binary that reads ndjson from a source and
forwards it line by line to a TCP sink (Filebeat, Fluent Bit, Logstash, …)
or to stdout. It is typically supervised by `vigild` as a regular service;
see [Logging](logging.md) for complete setups.

Every option can also be set with the environment variable shown. Boolean
variables take `true` or `false`. A repeatable regex option takes one regex
from its variable; combine several with `|`.

### Source (exactly one)

| Flag | Env var | Description |
|---|---|---|
| `--source-socket PATH` | `SOURCE_SOCKET` | Read the stream over a Unix socket, e.g. vigild's `<path-prefix>/vigild.sock`. **At most 107 bytes** ([socket path limit](installation.md#socket-path-limit-107-bytes)) |
| `--source-url URL` | `SOURCE_URL` | Read the stream from an HTTP(S) URL, e.g. `https://vigild:8443/v1/logs/follow?format=ndjson` |
| `--kubernetes` | `SOURCE_KUBERNETES` | Stream the logs of Running pods via the Kubernetes API |

There is no default source: the relay exits with `specify exactly one
source: --kubernetes, --source-url, or --source-socket` when none or more
than one is given.

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--source-path PATH` | `SOURCE_PATH` | `/v1/logs/follow?format=ndjson` | HTTP path requested over `--source-socket` (only there; `--source-url` carries its own path) |

### Source connection (`--source-url`, `--source-socket`)

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--source-connect-timeout MS` | `SOURCE_CONNECT_TIMEOUT` | `10000` | TCP connect timeout (`0` = none) |
| `--source-read-timeout MS` | `SOURCE_READ_TIMEOUT` | `0` | Reconnect when no data arrives for this long (`0` = disabled) |
| `--source-idle-timeout MS` | `SOURCE_IDLE_TIMEOUT` | `0` | Reconnect when no complete line arrives for this long (`0` = disabled); detects hung streams |
| `--source-keepalive-interval SECS` | `SOURCE_KEEPALIVE_INTERVAL` | `0` | TCP keepalive: seconds between probes (`0` = off); 3 unanswered probes close the connection |
| `--source-keepalive-timeout SECS` | `SOURCE_KEEPALIVE_TIMEOUT` | `0` | Seconds of silence before the first probe (`0` = 3 × interval) |

### Source TLS and authentication (`--source-url` only)

| Flag | Env var | Description |
|---|---|---|
| `--source-cacert PATH` | `SOURCE_CACERT` | PEM CA certificate(s) to verify the source |
| `--source-insecure` | `SOURCE_INSECURE` | Skip certificate verification (e.g. vigild's self-signed certificate) |
| `--source-user-file PATH` | `SOURCE_USER_FILE` | File with Basic Auth credentials `user:password`, read on every (re)connect so a rotated Secret is picked up. Use a vigild identity of access `read` |
| `--source-cert PATH` | `SOURCE_CERT` | PEM client certificate for mTLS (requires `--source-key`) |
| `--source-key PATH` | `SOURCE_KEY` | PEM private key for `--source-cert` |

These options are rejected with `--kubernetes` (TLS and authentication for
the Kubernetes API come from the kubeconfig or the ServiceAccount) and, apart
from timeouts, with `--source-socket`.

### Source proxy (`--source-url`, `--kubernetes`)

| Flag | Env var | Description |
|---|---|---|
| `--source-proxy URL` | `SOURCE_PROXY` | HTTP or HTTPS proxy (overrides `HTTP_PROXY` / `HTTPS_PROXY`) |
| `--source-proxy-insecure` | `SOURCE_PROXY_INSECURE` | Skip certificate verification of an HTTPS proxy |
| `--source-proxy-cacert PATH` | `SOURCE_PROXY_CACERT` | PEM CA certificate(s) of the proxy |
| `--source-no-proxy LIST` | `SOURCE_NO_PROXY` | Comma-separated hosts that bypass the proxy: hostnames, domain suffixes (`.corp`), IPv4 and IPv6 CIDRs. Only for `--source-url`; with `--kubernetes` there is one API server, so leave `--source-proxy` unset instead |

### Source reconnect

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--source-reconnect-delay MS` | `SOURCE_RECONNECT_DELAY` | `500` | First retry delay; doubles each retry. Triggers: connection refused, timeout, HTTP non-2xx, read error. A clean end of the stream resets it |
| `--source-reconnect-max MS` | `SOURCE_RECONNECT_MAX` | `30000` | Retry delay ceiling |
| `--source-reconnect-retries N` | `SOURCE_RECONNECT_RETRIES` | `0` | Consecutive failures before the relay exits (`0` = unlimited); vigild then restarts it via `on-failure: restart` |

### Kubernetes (`--kubernetes`)

The relay uses a kubeconfig (`KUBECONFIG` or `~/.kube/config`) when there is
one, otherwise the in-cluster ServiceAccount. It needs `get`, `list` and
`watch` on `pods` and `get` on `pods/log` in the namespace; see
`examples/kubernetes-pod-logs/k8s/rbac.yaml`.

Each log line is emitted as an ndjson object:

```json
{"timestamp":"2026-03-21T21:09:29.722Z","namespace":"my-ns","pod":"myapp-abc","stream":"stdout","message":"..."}
```

With `--container` a `container` field is added:

```json
{"timestamp":"2026-03-21T21:09:29.722Z","namespace":"my-ns","pod":"myapp-abc","container":"sidecar","stream":"stdout","message":"..."}
```

`stream` is `stdout` or `stderr` when the API server accepts the `stream=`
parameter, otherwise `output` (one combined stream: older clusters, the
feature gate is off, or `--no-stream-param` is set).

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--namespace NS` | `NAMESPACE` | `default` | Namespace to watch |
| `--pod-selector SELECTOR` | `POD_SELECTOR` | _(all pods)_ | Label selector, e.g. `app=myapp` |
| `--container NAME\|all\|n1,n2,...` | `CONTAINER_SELECTOR` | _(default container)_ | Container(s) to stream: `all`, or a comma-separated list (a pod gets those it has; one with none is skipped). Omit to let Kubernetes pick the default container |
| `--exclude-pod REGEX` | `EXCLUDE_POD` | — | Skip pods whose name matches (repeatable, OR-combined) |
| `--tail-lines N` | `TAIL_LINES` | `0` | Emit the last N lines of each pod when its stream starts, then go live (`0` = disabled) |
| `--since-seconds SECS` | `SINCE_SECONDS` | `10` | Start this far back on (re)connect, to cover the reconnect gap; ignored when `--tail-lines` is set |
| `--watch-interval SECS` | `WATCH_INTERVAL` | `10` | How quickly a stream is restarted after the API server closed it (about every 5 minutes). Pods are discovered by events, not by this interval |
| `--max-log-requests N` | `MAX_LOG_REQUESTS` | `0` | Max concurrent pod log streams (`0` = unlimited) |
| `--no-stream-param` | `NO_STREAM_PARAM` | `false` | Never try the `stream=Stdout/Stderr` query parameter; use the combined stream |

**`--no-stream-param`**: The `stream=` parameter exists since K8s 1.32 but only
works with the feature gate `PodLogsQuerySplitStreams`, which is off by default;
some distributions (e.g. OpenShift) also forbid it via admission control. On
K8s ≥ 1.32 the relay tries it first; when the API server answers
`"stream: Forbidden: may not be specified"`, it logs one warning and uses the
combined stream for all pods from then on. The flag skips that first attempt.

### Line filters

Both flags are repeatable (OR-combined). A line is forwarded when it matches
an include pattern (or none is given) and no exclude pattern.

| Flag | Env var | Description |
|---|---|---|
| `--include REGEX` | `INCLUDE` | Forward only lines that match |
| `--exclude REGEX` | `EXCLUDE` | Drop lines that match (applied after `--include`) |

With `--kubernetes` the patterns are matched against the log message; with
`--source-url` and `--source-socket` against the raw ndjson line as vigild
sends it:

```bash
# Only forward lines whose message contains ERROR or WARN
vigil-log-relay \
  --source-socket /tmp/vigil/vigild.sock \
  --include '"message":"[^"]*(ERROR|WARN)' \
  --tcp-sink-host 127.0.0.1 \
  --tcp-sink-port 5170
```

The patterns use Rust regex syntax. With `--multiline` they apply to the
whole joined record.

### Lines

Nothing is dropped for being long; see
[Logging](logging.md), "Line limits" in the vigil-log-relay section.

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--max-line-bytes BYTES` | `MAX_LINE_BYTES` | `131072` | Longest message (1024 to 67108864), as vigild's `--log-max-line-bytes`. A longer pod log line, or a joined record that would grow past it, is forwarded in parts. A record from vigild (socket or URL mode) may be 6 × this + 4 KiB; a longer line is forwarded in parts (no valid JSON) with a warning |
| `--multiline PRESET` | `MULTILINE` | — | Join the lines of a stack trace into one record: `java`, `python` or `indent` (rules as vigild's [`logs-multiline`](configuration.md#logs-multiline)) |
| `--multiline-start REGEX` | `MULTILINE_START` | — | A matching line (message) starts a new record, others belong to the one before |
| `--multiline-continue REGEX` | `MULTILINE_CONTINUE` | — | A matching line (message) belongs to the record before it |
| `--multiline-timeout DURATION` | `MULTILINE_TIMEOUT` | `1s` | How long a joined record waits for a further line (`10ms` to `10s`; `200ms` before 4.0.1) |

At most one of `--multiline`, `--multiline-start` and
`--multiline-continue`. They work in every source mode: pod logs are
joined per pod and container, records from vigild per `service` and
`stream` on their `message` (the joined record keeps the other fields of
its first record), other lines as text.

### Sink

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--stdout-sink` | `STDOUT_SINK` | `false` | Write the lines to stdout instead of TCP; the relay's own log goes to stderr |
| `--send-buffer-lines N` | `SEND_BUFFER_LINES` | `8192` | Lines kept while the sink is down or slow (64 to 1048576); more are dropped, summed up in one warning per second |
| `--tcp-sink-host HOST` | `TCP_SINK_HOST` | `127.0.0.1` | TCP sink host |
| `--tcp-sink-port PORT` | `TCP_SINK_PORT` | `5170` | TCP sink port |
| `--dest-connect-timeout MS` | `DEST_CONNECT_TIMEOUT` | `10000` | TCP connect timeout (`0` = none) |
| `--dest-write-timeout MS` | `DEST_WRITE_TIMEOUT` | `0` | Reconnect when a write stalls this long (`0` = disabled) |
| `--dest-idle-timeout MS` | `DEST_IDLE_TIMEOUT` | `0` | Reconnect when nothing was written for this long (`0` = disabled) |
| `--dest-keepalive-interval SECS` | `DEST_KEEPALIVE_INTERVAL` | `0` | TCP keepalive: seconds between probes (`0` = off) |
| `--dest-keepalive-timeout SECS` | `DEST_KEEPALIVE_TIMEOUT` | `0` | Seconds of silence before the first probe (`0` = 3 × interval) |
| `--dest-reconnect-delay MS` | `DEST_RECONNECT_DELAY` | `500` | First reconnect delay; doubles each retry |
| `--dest-reconnect-max MS` | `DEST_RECONNECT_MAX` | `30000` | Reconnect delay ceiling |

### Health check endpoint

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--healthcheck HOST:PORT` | `HEALTHCHECK` | `127.0.0.1:9091` | Listen address for `GET /healthz` (`200 ok` / `503 stale`); port `0` picks a free port (logged at startup). The relay does not start if the address cannot be bound |
| `--healthcheck-max-age SECS` | `HEALTHCHECK_MAX_AGE` | `30` | Seconds without a tick before `/healthz` returns 503. Kubernetes mode ticks once per watch cycle (keep it ≥ 3 × `--watch-interval`); the HTTP modes tick every 30 s |

### Logging and other flags

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--log-format text\|json` | `VIGIL_LOG_FORMAT` | `text` | Format of the relay's own log |
| `--debug` | `RELAY_DEBUG` | `false` | Debug logging with timestamps |
| `--worker-threads N` | `VIGIL_LOG_WORKER_THREADS` | `2` (1 with a single CPU) | Worker threads of the async runtime; raise it only when watching very many pods |
| `-h`, `--help` | | | Print help |
| `-V`, `--version` | | | Print version |
