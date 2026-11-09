# Migrating from v3 to v4

vigil-rs v4 changes defaults that v3 got wrong for containers (vigild's
environment went to every service, anonymous TLS callers were admin, a
shutdown did not wait for the services), and it reads layer files strictly.
Most setups need a few changes. This guide compares v3.2.2 with v4.0 by
topic: what changed, and what to do. The
[CHANGELOG](../../CHANGELOG.md) lists every change with its commit.

## Checklist

Work through these in order; each links to its section.

1. Runtime files: set `--path-prefix` where v3 used `--socket` outside
   `/run/vigil` ([Start-up](#start-up-and-runtime-files)).
2. Start v4 once with your layer files and fix what it rejects; the error
   names the file and the key ([Layer files](#layer-files)).
3. Give services the environment variables they need: they no longer get
   vigild's ([Environment](#environment-of-services-checks-and-alerts)).
4. Create identities for everything that uses the TLS listener, for example
   vigil-log-relay ([Access control](#access-control)).
5. Decide on vigild's exit code: the service's (new default) or 0/10
   ([Start-up](#start-up-and-runtime-files)).
6. Check API clients and scripts: new states, actions that wait, error
   bodies, `vigil services` exit codes ([API and CLI](#api-and-cli)).
7. Raise `terminationGracePeriodSeconds` where services have long
   `kill-delay`s: vigild now waits for them ([Service behaviour](#service-behaviour)).
8. Update PromQL that joins on check metrics ([Metrics](#metrics)) and
   vigil-log-relay settings ([vigil-log-relay](#vigil-log-relay)).

## Start-up and runtime files

**One directory for all runtime files.** `--path-prefix`
(`VIGIL_PATH_PREFIX`, default `/run/vigil`) holds the socket and the
`notify/` sockets. vigild creates it (mode 0711) and refuses to start if it
cannot use it, even when `--socket` points elsewhere. The socket defaults to
`<path-prefix>/vigild.sock`.

```diff
 # OpenShift: /run is not writable
-vigild --layers-dir /etc/vigil/layers --socket /tmp/vigild.sock
+vigild --layers-dir /etc/vigil/layers --path-prefix /tmp/vigil
```

The release image sets `VIGIL_PATH_PREFIX=/tmp/vigil`. Keep the prefix at
most 78 bytes long if a service uses `ready: notify`; longer prefixes are
rejected when the layers load
([socket path limit](installation.md#socket-path-limit-107-bytes)).

**Exit code.** When a policy shuts vigild down (`on-failure: shutdown` and
the like), vigild now exits with the service's exit code (after
`exit-code-map`, `128 + signal` for a signal). v3 exited with 0 or 10.
To keep 0 and 10, start vigild with `--exit-code fixed`
(`VIGIL_EXIT_CODE=fixed`).

**`--log-format`** accepts only `text` and `json` (vigild and
vigil-log-relay); v3 fell back to text for any other value.

## Environment of services, checks and alerts

**Services and exec checks no longer get vigild's environment.** In
Kubernetes that environment holds everything from `env` and `envFrom`,
Secrets included. A service now gets `PATH`, `TZ`, `HOME`, `USER`,
`LOGNAME`, its `environment`, its `env-file` files, and the vigild variables
it asks for:

```yaml
services:
  app:
    command: /usr/local/bin/app
    pass-env-glob: [APP_*, DATABASE_URL]   # pass these vigild variables
    # pass-env: true                       # or everything, as in v3
```

Proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`) are
not passed by default either, as their URLs may carry credentials; pass them
with `pass-env-regex: ['(?i)(https?|all|no)_proxy']`.
See [`environment` / `pass-env`](configuration.md#environment--pass-env--pass-env-glob--pass-env-regex).

**Templates.** A value of `command` or `environment` (also of an exec
check's `environment`) that contains `{{` or `{%` is now a template,
rendered on every start; an undefined variable fails the start. Values
without these markers are unchanged. Keep a literal `{{` with
`{% raw %}…{% endraw %}`.
See [templates](configuration.md#templates-in-command-and-environment).

**Alerts and `env:`.**

- An alert sees only the vigild variables its `pass-env-glob` or
  `pass-env-regex` allows (or all with `pass-env: true`). Add the variables
  each alert uses.
- `env:NAME` is resolved when the layers load, not when an alert is sent.
  A variable that is unset or not allowed makes the layer invalid; v3 sent
  an empty value and logged a warning.
- `env:` must be the whole value. `"Bearer env:TOKEN"` never worked (v3
  sent the text as it is); put the whole value into the variable.
- `HTTPS_PROXY` and the other proxy variables apply to an alert only when
  its `pass-env-glob` allows them. HTTP checks no longer use them at all;
  set `proxy.url` on the check.

```diff
 alerts:
   alertmanager-prod:
     url: "env:ALERTMANAGER_URL"
     format: alertmanager
     on-check: [myapp-alive]
+    pass-env-glob: [ALERTMANAGER_*, CLUSTER_NAME]
     headers:
-      Authorization: "Bearer env:ALERTMANAGER_TOKEN"
+      Authorization: "env:ALERTMANAGER_AUTH"   # holds "Bearer <token>"
     labels:
       cluster: "env:CLUSTER_NAME"
```

## Layer files

**Unknown keys are errors.** v3 ignored keys it did not know, so a typo or
a key in the wrong spelling had no effect, silently. v4 does not load such a
layer and names the file and the key. Keys are kebab-case:

| v3 (often ignored without a word) | v4 |
|---|---|
| `send_info_fields`, `retry_attempts`, `retry_backoff` | `send-info-fields`, `retry-attempts`, `retry-backoff` |
| alert `tls_ca` / `tls-ca`, `proxy_ca` / `proxy-ca` | `tls: {ca: …}` (one path or a list) |
| alert `tls_insecure` / `tls-insecure` | `tls: {insecure: true}` |
| alert `proxy: "http://…"`, `no_proxy` / `no-proxy` | `proxy: {url: "http://…", no-proxy: "…"}` |
| HTTP check `ca`, `insecure` | `tls: {ca: …}`, `tls: {insecure: true}` |
| HTTP check `insecure-proxy` | removed; put the proxy's CA into `tls.ca` |
| `timeout` under `http:` | `timeout` of the check |

```diff
 alerts:
   otlp-prod:
     url: http://otel-collector:4318/v1/logs
     format: otlp-logs
     on-check: [myapp-alive]
-    proxy: "http://corp-proxy:3128"
-    no_proxy: "localhost, .internal"
-    tls_ca: /etc/vigil/certs/internal-ca.pem
+    proxy:
+      url: http://corp-proxy:3128
+      no-proxy: "localhost, .internal"
+    tls:
+      ca: /etc/vigil/certs/internal-ca.pem
```

Paths in `tls:` must be absolute. An unreadable CA file or an invalid proxy
URL stops loading the layers.

**`override` works.** v3 ignored `override: replace`; a later layer always
merged. Check the layers that set it. A layer that merges into a service or
check and does not set `startup` (or `level`) now keeps the earlier value;
v3 reset it to `disabled` (`alive`).

**Checked when the layers load** (v3 loaded them and failed later, or never):

- every service has a `command` after all layers are merged;
- `after`, `before`, `requires` and `service-context` name existing
  services, and dependencies have no cycle (the error shows it);
- every check has exactly one of `http`, `tcp` or `exec`;
- durations are whole numbers with `ms`, `s`, `m` or `h` (`1m30s`), see
  [Duration format](configuration.md#duration-format); a check `period`
  between 1s and 24h; `backoff-delay` above 0, `backoff-limit` not below it,
  `backoff-factor` at least 1.0; `timeout`, `threshold` and
  `retry-attempts` above 0; `alerts.max-queue-depth` at least 1 and
  `max-queue-time` at least 1s;
- a `body-template` parses;
- a numeric `user-id` without an entry in `/etc/passwd` comes with `group`
  or `group-id` (v3 warned and used the UID as GID).

**Identities in layer files** are new: `identities:` in a layer, for
example the read identity of vigil-log-relay from a ConfigMap and a
Secret. See [Identities](configuration.md#identities).

## Access control

- **TLS:** without a matching identity a TLS caller has the level of
  `--tls-anonymous-access`: `metrics` (default) or `open`, never more. v3
  made every caller admin while no identity existed. Remote control
  (`vigil --url …`) and log shipping need a `basic` or `tls` identity.
- **Unix socket:** while no identity was added over the API, root and
  vigild's own UID are admin (bootstrap); other UIDs are `open`. Identities
  from layer files do not end the bootstrap.
- **`/v1/checks`** is `open`, so probes need no identity.
- `POST /v1/identities` rejects keys other than `user-id` in `local`.

```yaml
# Layer file: vigil-log-relay reads logs over TLS with Basic Auth.
identities:
  log-relay:
    access: read
    basic:
      password-hash-file: /run/secrets/vigil/log-relay.hash
```

See [Default access](configuration.md#default-access-caller-without-a-matching-identity).
A service that calls the API itself needs an identity too:
[Let a service call the API](configuration.md#let-a-service-call-the-api).

## Service behaviour

**New states.** `starting` (runs, not ready yet) and `done` (a `oneshot`
that finished). With the default `ready: process` a service is `active` as
soon as it runs, as in v3. `after`, `requires` and `before` wait for
`active` (or `done`).

**Shutdown waits.** On SIGTERM, `vigil vigild stop` or a shutdown policy,
vigild stops the services in the start order reversed and waits for each up
to its `kill-delay` before SIGKILL. v3 signalled all at once and exited at
once, so the container runtime killed the services. A shutdown can now take
the sum of the `kill-delay`s along the longest dependency chain: keep
`terminationGracePeriodSeconds` above `--shutdown-delay` plus that sum.
See [Orchestrators](../operator-guide/orchestrators.md).

**Replan restarts changed services.** `vigil replan`, `POST /v1/replan` and
`--watch-layers` restart a running service whose configuration changed (in
more than `summary` or `description`), then the running services that
`require` it. v3 applied the change at the next start. Change a service's
configuration together with a planned restart.

**HTTP checks open a new connection on every run** (DNS, TCP, TLS
handshake), so a run also proves that a new connection works. Expect one
connection per run on the checked endpoint.

**Long lines.** A line of a service's output longer than 128 KiB becomes
several log entries (v3 kept any length in one). Raise it with
`--log-max-line-bytes` (up to 1 MiB).

## API and CLI

- **Actions wait.** `POST /v1/services` (and `vigil services start`,
  `stop`, `restart`) answers when the action is done: a start once the
  services are ready (a oneshot once its run ended), a stop once the
  processes are gone. v3 answered once the command was sent. Give API
  clients a timeout of `ready-timeout` or `kill-delay` plus a margin. The
  change is `doing` meanwhile.
- **`vigil services` exits with 1** when the action fails (the change ends
  in `error`, with the reason per service); v3 exited with 0.
- **New values:** service states `starting` and `done`, the field
  `next-run`, the change status `doing`. Clients must accept them.
- **Errors are JSON.** A wrong `Content-Type` (415), invalid JSON (400), an
  unknown field or wrong type (422) and a bad query parameter (400) answer
  with the error envelope (`{"type":"error","status-code":…,"message":…}`)
  instead of plain text. A caller without access gets 403 before any of
  these.
- **Unknown parameters are rejected.** `GET /v1/logs`, `/v1/logs/follow`,
  `/v1/services`, `/v1/checks`, `/v1/alerts` and `/v1/identities` answer 400
  for a query parameter they do not know (v3 ignored it, so `service=web`
  returned all services); request bodies with an unknown field are 422.
- `vigil identities remove` needs at least one name.

## Metrics

`vigil_check_up`, `vigil_check_success_count_total` and
`vigil_check_failure_count_total` have a second label, `level` (`alive` or
`ready`), so they are new series. `vigil_check_up{check="db"}` still
matches; recording rules and joins that list the labels (`on(check)`,
`by(check)`) need `level` added or ignored.
See [Metrics](../operator-guide/metrics.md).

## vigil-log-relay

- Its own log messages go to **stderr** (v3: stdout). Kubernetes collects
  both; a pipe such as `vigil-log-relay … | logger` needs `2>&1`.
- **`--dest-read-timeout` is now `--dest-write-timeout`**
  (`DEST_WRITE_TIMEOUT`): it always was the timeout of a write to the TCP
  sink. The relay refuses to start while `DEST_READ_TIMEOUT` is set.
- A pod log line or record longer than `--max-line-bytes` (default
  128 KiB, up to 64 MiB) is forwarded in several records.
- Reading from vigild over TLS needs an identity with access `read`:
  `--source-user-file` (Basic Auth) or a client certificate.
- Every option has an environment variable; see the
  [CLI reference](cli-reference.md#vigil-log-relay).
