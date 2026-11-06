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
    command: /usr/local/bin/myapp --config /etc/myapp.yaml --workers {{ cpu() }}  # templates, see below
    type: simple                       # simple | oneshot (default: simple)
    schedule: '0 3 * * *'              # cron pattern, type: oneshot only
    schedule-tz: Europe/Vienna         # time zone for schedule (default: UTC)
    working-dir: /var/lib/myapp        # working directory (default: /)
    user: myapp                        # run as this username
    user-id: 1000                      # or by UID
    group: myapp                       # primary group
    group-id: 1000                     # or by GID
    environment:
      FOO: bar
      DATABASE_URL: postgres://localhost/mydb
      DB_PASSWORD: "{{ file('/etc/secrets/db-password') }}"  # read on every start
    pass-env: false                    # inherit vigild's whole environment? (default: false)
    pass-env-glob: [APP_*, LOG_LEVEL]  # pass these vigild variables (wildcards * and ?)
    pass-env-regex: ['^K8S_[A-Z]+_URL$']  # pass vigild variables matching (anchored)
    env-file: [/vault/secrets/db, -/etc/app/optional.env]  # KEY=value files, read on every start (- = optional)
    watch-files:                       # react to changed files (off without this block)
      on-change: restart               # restart (default) | signal (= SIGHUP) | signal:SIGUSR1 | ignore
      interval: 30s                    # polling interval (default: 30s)
      paths: [/etc/myapp.yaml]         # watch ONLY these (default: the files read at start)

    # --- Startup ---
    startup: enabled                   # enabled | disabled (default: disabled)
    after:                             # start after these services are Active
      - database
      - cache
    before:                            # start before these services
      - frontend
    requires:                          # fail if these services are not Active
      - database
    ready: process                     # process | notify | check (default: process)
    ready-check: myservice-alive       # check that signals readiness (ready: check)
    ready-timeout: 30s                 # stop + on-failure if not ready in time (0s = wait forever)
    healthy-time: 10s                  # healthy after running for this time without a crash (default: 10s, 1s..1h)

    # --- Stop behaviour ---
    stop-signal: SIGTERM               # signal sent on stop (default: SIGTERM)
    kill-delay: 10s                    # SIGKILL sent after this if still running

    # --- Exit policies ---
    on-success: restart                # what to do when the exit counts as success
    on-failure: restart                # what to do otherwise
    success-exit-codes: [0]            # exit codes that count as success (default: [0])
    exit-code-map: {143: 0}            # treat SIGTERM (128+15) as exit 0, see below

    # --- Health-check-triggered actions ---
    on-check-failure:
      myservice-alive: restart         # restart when check 'myservice-alive' goes Down

    # --- Restart backoff ---
    backoff-delay: 500ms               # delay before the first restart (default: 500ms)
    backoff-factor: 2.0                # multiply the delay by this after each restart
    backoff-limit: 30s                 # largest delay; reaching it means giving up (default: 30s)

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

`command` and the values of `environment` may contain
[templates](#templates-in-command-and-environment).

#### Templates in `command` and `environment`

`command` and the values of `environment` are templates in the
[minijinja](https://docs.rs/minijinja) (Jinja2) syntax, the same as
[`body-template`](#body-template) of alerts. They are resolved on **every
start** of the service, so a rotated secret file or a changed limit is picked
up on the next (re)start. With [`watch-files`](#watch-files) vigild also
restarts the service when a file it read changes.

```yaml
services:
  api:
    command: /usr/local/bin/api --port {{ env.PORT | default("8080") }} --workers {{ cpu("100% - 1") }}
    environment:
      PORT: "9090"
      DB_PASSWORD: "{{ file('/etc/secrets/db-password') }}"
      GOMEMLIMIT: "{{ mem('90% - 64MiB') }}MiB"
```

| Template | Result |
|---|---|
| `{{ env.DB_HOST }}` | Value of the environment variable `DB_HOST` (see below which variables are visible). An undefined variable is an **error**: the service does not start. |
| `{{ env.LOG_FLAG \| default("--loglevel=info") }}` | The value of `LOG_FLAG` (e.g. `--loglevel=debug`), or, if `LOG_FLAG` is not defined, exactly the string `--loglevel=info`. An empty value counts as defined; `default("--loglevel=info", true)` also replaces an empty value. |
| `{% if env.DEBUG is defined %}--verbose{% endif %}` | Conditions, as in Jinja: adds `--verbose` only if `DEBUG` is set. In `command`, an argument that renders to nothing is dropped, so no empty argument is passed. |
| `{{ file("/etc/secrets/db-password") }}` | Content of the file, without one trailing newline (e.g. `MY-pw1!`). A missing or **empty** file is an error: the service does not start (an empty secret is almost always a mistake). |
| `{{ file("/etc/config/log-flag", "--loglevel=info") }}` | The content of the file (e.g. `--loglevel=debug`), or, if the file is missing or empty, exactly the string `--loglevel=info`. Any other problem (not readable, not a regular file, …) is still an error. |
| `{{ file("/etc/secrets/db-password", "") or file("/etc/secrets-legacy/db-password") }}` | A fallback **file**: the legacy file is read only if the first one is missing or empty. Chains work too: `file("/etc/config/log-flag", "") or file("/etc/defaults/log-flag", "") or "--loglevel=info"`. Do not write `file("/etc/secrets/db-password", file("/etc/secrets-legacy/db-password"))`: arguments are always evaluated, so the legacy file would be required even when the first one exists. |
| `{{ mem("75%") }}` | 75 % of the memory limit, as a whole number of **MiB**. |
| `{{ mem("90% - 64MiB") }}` | 90 % minus 64 MiB. Units: `B`, `KiB`, `MiB`, `GiB`, `KB`, `MB`, `GB`. |
| `{{ mem("512MiB") }}` | A fixed amount, in MiB. |
| `{{ cpu() }}` | Number of CPUs available to the container. |
| `{{ cpu("50%") }}` / `{{ cpu("100% - 1") }}` | A share of them (rounded), or minus a number of CPUs. Never less than 1. |

**Fallback values** (`default(…)`, the second argument of `file(…)`, the last
part of an `or` chain) are returned **exactly as written**, `=`, spaces and
quotes included; in `command` such a value is always one argument. So
`--port={{ env.PORT | default("8080") }}` and
`{{ env.PORT_FLAG | default("--port=8080") }}` both give `--port=8080`.

**Variables (`env`):**

- In `environment` values: the variables vigild passes to the service
  (the base set and `pass-env` / `pass-env-glob` / `pass-env-regex`) and the
  [`env-file`](#env-file) variables, *not* the other `environment` entries.
  vigild's whole environment is never visible.
- In `command`: the service's final environment, i.e. including the resolved
  `environment` entries (`--port {{ env.PORT }}` with `environment: {PORT: "9090"}`).

**`file`:** the path must be absolute. Symlinks are followed (Kubernetes
secret volumes consist of them); the target must be a regular file of at most
64 KiB, UTF-8 and without NUL bytes.

**`mem` / `cpu`:** the memory limit is the smallest cgroup limit of the
container (cgroup v2 `memory.max`, else v1 `memory.limit_in_bytes`), at most
the RAM of the machine. The CPU count is the number of CPUs the process may
run on, limited by the cgroup CPU quota rounded up (`cpu.max`, else
`cpu.cfs_quota_us`). In Kubernetes and OpenShift these are the container's
`resources.limits`; see [orchestrators](../operator-guide/orchestrators.md#memory-and-cpu-templates)
for containers without limits. A `mem` result below 1 MiB is an error.

**Rules:**

- Only values containing `{{` or `{%` are templates; everything else is used
  as is. Shell constructs such as `${#WORKERS}` are therefore unaffected, unless
  the same value also contains a template. Then write them inside
  `{% raw %}…{% endraw %}`: `sh -c 'echo "user {{ env.DB_USER }}, {% raw %}${#DB_PASSWORD}{% endraw %} chars"'`.
- In `command`, each argument is rendered on its own after the command has
  been split into arguments. A substituted value is always exactly one
  argument, even if it contains spaces or quotes. A template tag
  (`{{ … }}`, `{% … %}`) may contain spaces and quotes, but a block such as
  `{% if %}…{% endif %}` must stay within one argument (quote it). An
  argument that renders to an empty string is dropped (like an empty unquoted
  expansion in a shell), so `{% if env.DEBUG is defined %}--verbose{% endif %}`
  is an optional flag.
- Rendering is a single pass: a value that itself contains `{{` is inserted
  literally and never evaluated.
- In YAML, quote a value that **starts** with `{{` or `{%`
  (`DB_PORT: "{{ env.DB_PORT | default('5432') }}"`); unquoted, YAML reads `{` as the start of a map.
- The syntax is checked when layers are loaded and on `replan`; a syntax
  error makes the layer invalid. Undefined variables, missing files and
  similar problems are only detected at start: the service goes to `error`
  and the reason is logged.
- Resolved values are never logged and never shown by the API, which returns
  the templates as written.
- `command` and `environment` of services and `environment` of exec checks are
  templates; an exec check's `command` is not (see [Exec check](#exec-check)).

#### `type` / `schedule` / `schedule-tz`

| `type` | Meaning |
|---|---|
| `simple` | A long-running process. **Default.** |
| `oneshot` | A job that runs to completion (migration, backup, cache warm-up, …) |

A `oneshot` service is `starting` while it runs. When it exits successfully
(see `success-exit-codes`) it ends in the state **`done`** and is not
restarted; services ordered after it (`after`, `requires`, `before`) start
only then. A failure goes through `on-failure` as usual (default `restart`
with backoff, so a migration is retried). `vigil start <name>` runs it again;
`replan` does not (a finished migration is not repeated on every config
change).

```yaml
services:
  migrate:
    type: oneshot
    command: /app/migrate
    startup: enabled
  app:
    command: /app/server
    startup: enabled
    after: [migrate]              # starts once migrate is done
```

**`schedule`** runs a `oneshot` periodically (cron replacement):

```yaml
services:
  backup:
    type: oneshot
    command: /usr/local/bin/backup
    startup: enabled              # arms the schedule
    schedule: '0 3 * * *'         # daily at 03:00
    schedule-tz: Europe/Vienna    # default UTC
```

- Pattern: 5 fields (`minute hour day-of-month month day-of-week`) or 6 with
  seconds first (`*/30 * * * * *`). Ranges, lists, steps, names (`MON`,
  `JAN`) and the usual cron extensions (`L`, `#`, `W`) are supported. As in
  Vixie cron, day-of-month and day-of-week are OR-ed when both are set.
  Parsing is done by [croner](https://github.com/hexagon/croner-rust); its
  README describes the full pattern syntax
  ([API docs](https://docs.rs/croner/4.0.2/croner/)). croner also accepts a
  7th field for the year (e.g. `0 0 3 * * * 2027`); vigild rejects such
  patterns, use 5 or 6 fields.
- `startup: enabled` **arms** the schedule; the job does not run when vigild
  starts, only at the scheduled times. With `startup: disabled` the schedule
  is not armed. `vigil start <name>` always runs the job once by hand.
- A run that is still active when the next one is due is **skipped** (no
  overlapping runs).
- Default `on-failure` for scheduled jobs is `ignore`: a failed run is
  recorded (status, `last-exit`, metrics) and the next run comes at the next
  scheduled time. Set `on-failure: restart` to retry with backoff.
- `schedule-tz` needs the system time zone database
  (`/usr/share/zoneinfo`). On Alpine-based images install it with
  `apk add tzdata`; without it vigild rejects the layer. Daylight saving
  time is handled: a time that does not exist (spring forward) or exists
  twice (fall back) still runs once.
- `replan` re-arms a changed schedule immediately.
- There is no state on disk: runs missed while vigild was not running (e.g.
  during a pod restart) are not caught up.
- The next run is shown as `next-run` in the API and in the `Next-Run` column
  of `vigil services list`. Metrics: `vigil_service_last_run_timestamp_seconds`
  and `vigil_service_last_run_success` describe the last run,
  `vigil_service_runs_total{result="success|failure"}` counts all runs and
  `vigil_service_running` is `1` while a job runs. Example alerts:
  `increase(vigil_service_runs_total{result="failure"}[1h]) > 0` (a run
  failed) and `vigil_service_running == 1 and time() - vigil_service_last_run_timestamp_seconds > 3600`
  (a job hangs).

Not allowed with `type: oneshot`: `ready`, `ready-check`, `ready-timeout` (a
oneshot is ready when it is done) and `on-success: restart` (use `schedule`).
`schedule` requires `type: oneshot`.

#### `user` / `user-id` / `group` / `group-id`

Run the service as another user. This needs vigild to run as root (or
with `CAP_SETUID`/`CAP_SETGID`).

- Nothing set: the service runs with vigild's own identity and no
  credential change is attempted. This is the normal case on OpenShift.
- `user` or `user-id` without a group: the user's **primary group from
  `/etc/passwd`** is used. A numeric `user-id` without a passwd entry
  needs an explicit `group` or `group-id`, otherwise the service fails to
  start (3.2.x only warned and used GID = UID).
- The supplementary groups are always reset to the user's groups from
  `/etc/group` (or only the primary group if the user has no passwd
  entry). The child never keeps root's groups.
- Exec checks running in a service context use the same rules.

#### `environment` / `pass-env` / `pass-env-glob` / `pass-env-regex`

A service does **not** inherit vigild's environment by default. vigild's
environment often holds secrets (for example Kubernetes `env`/`envFrom` from
Secrets) that a service does not need. A service gets:

1. A minimal base: `PATH` (vigild's, or a standard path if unset), `TZ` if
   set, and `HOME`, `USER`, `LOGNAME`. With `user`/`user-id` these describe
   the target user from `/etc/passwd`; otherwise they are vigild's.
2. vigild variables selected by `pass-env-glob` or `pass-env-regex`.
3. Variables from [`env-file`](#env-file) files.
4. `environment`, which overrides everything above.

| Field | Meaning |
|---|---|
| `pass-env` | `true` passes vigild's entire environment (the behaviour before v4); `pass-env-glob`/`pass-env-regex` then have no effect. Default `false`. |
| `pass-env-glob` | List of variable names. `*` matches any characters, `?` exactly one; the pattern must match the **whole** name (`APP_*` does not match `MY_APP_X`). |
| `pass-env-regex` | List of regular expressions, each anchored to the whole name (`TOKEN` does not match `MY_TOKEN`; write `.*TOKEN.*` for that). |

Glob and regex matches are combined. Invalid regular expressions make the
layer invalid: vigild refuses to start, and `vigil replan` keeps the running
plan. `pass-env: true` together with a list in the same service or exec check
loads, but vigild logs a warning: the list has no effect and should be
removed. In layer merges the lists are appended and `pass-env` is overridden.

An exec check with `pass-env: true` gets vigild's whole environment even when
its `service-context` uses `pass-env-glob` / `pass-env-regex`; the service
keeps its lists, so there is no warning for that.

Which variables were passed is logged at debug level, as names only.

vigild also marks itself non-dumpable at startup, so services running with
the same UID (e.g. on OpenShift) cannot read vigild's environment through
`/proc/<pid>/environ` either.

```yaml
# Kubernetes: the Deployment sets DATABASE_URL, DB_PASSWORD, APP_* and LOG_LEVEL.
services:
  api:
    command: /usr/local/bin/api
    pass-env-glob: [DATABASE_URL, DB_PASSWORD, APP_*, LOG_LEVEL]
  worker:
    command: /usr/local/bin/worker
    pass-env-glob: [APP_*]       # no database credentials for the worker
```

#### `env-file`

Files with one `KEY=value` per line, read on **every start** of the service
(so a secret rotated by the Vault Agent or ESO is picked up on the next
restart, which [`watch-files`](#watch-files) can trigger). Typical sources: a [Vault Agent](https://developer.hashicorp.com/vault/docs/agent-and-proxy/agent/template)
template, an External Secrets `target.template`, docker-style `.env` files.

```yaml
services:
  api:
    command: /usr/local/bin/api --db-user {{ env.DB_USER }}
    env-file:
      - /vault/secrets/db              # must exist, otherwise the start fails
      - -/etc/api/overrides.env        # leading "-": optional
    environment:
      DATABASE_URL: "postgres://{{ env.DB_USER }}@db:5432/app"
```

```sh
# /vault/secrets/db
# comments and empty lines are ignored
export DB_USER="app"        # "export " is optional
DB_PASSWORD='p@ss$word'     # single quotes: literal
GREETING="line1\nline2"     # double quotes: \" \\ \n \t \$
PLAIN=value with spaces     # unquoted: up to the end of the line, # included
```

- Paths must be absolute. Files are read in order; a later file overrides an
  earlier one, and `environment` overrides all of them.
- Unquoted values run to the end of the line (trailing whitespace removed); a
  `#` is part of the value. After a closing quote only whitespace or a
  `# comment` may follow. No variable expansion, no multi-line values.
- The variables are visible to [templates](#templates-in-command-and-environment)
  in `environment` and `command` (`{{ env.DB_USER }}`).
- Symlinks are followed; the target must be a regular file of at most 1 MiB,
  UTF-8 without NUL bytes. A missing file (without `-`), an unreadable file or
  a syntax error fails the start; the error names the file and line, never a
  value.
- Exec checks with `service-context` read the service's `env-file` too.
- In layer merges the lists are appended.

#### `watch-files`

With `watch-files`, vigild notices when files a service depends on change
and reacts, e.g. after the kubelet updated a Secret volume or the Vault Agent
renewed credentials. This replaces tools like stakater Reloader, without
restarting the whole pod.

**Off unless configured:** without a `watch-files` block vigild reads a
service's files only when it starts it, and never touches them while it
runs. With the block (even an empty `watch-files: {}`), every file the
service read at its last start through [`{{ file() }}`](#templates-in-command-and-environment)
or [`env-file`](#env-file) is checked every 30 seconds by default, and a
change **restarts** the service, which then reads the new content.

```yaml
services:
  api:
    command: /usr/local/bin/api
    environment:
      DB_PASSWORD: "{{ file('/etc/secrets/db-password') }}"
    watch-files: {}              # defaults: watch /etc/secrets/db-password, restart on change

  nginx:
    command: nginx -g 'daemon off;'
    watch-files:
      on-change: signal          # = SIGHUP: nginx reloads its config, no restart
      paths: [/etc/nginx/nginx.conf, /etc/nginx/tls/tls.crt]
```

| Field | Meaning |
|---|---|
| `on-change` | `restart` (default): stop the service and start it again. `signal`: send **`SIGHUP`** to the service's process group, no restart. `signal:<NAME>`: send that signal instead, e.g. `signal:SIGUSR1` or `signal:SIGUSR2`. `ignore`: do not watch (same as no block). |
| `interval` | How often the files are checked ([duration](#duration-format)). Default `30s`, allowed **`1s` to `24h`**: every check reads and hashes all watched files, so a shorter interval would only put load on the node or the file server. |
| `paths` | Absolute paths to watch. **If set, only these files are watched** and the files read by `file()` / `env-file` are not. Use it for files the application reads itself (its config, TLS certificates). Without `paths`, the files read at the last start are watched. `paths: []` watches nothing. |

- **Plain `signal` means `SIGHUP`.** Write `signal:SIGUSR1` (any signal name
  as for [`stop-signal`](#stop-signal)) for applications that reload on
  another signal.
- A change is detected by **content**: vigild compares a hash of each file
  with the content the service got at its start (for `paths`: with the
  content when the process started), so a `touch` or an mtime change alone
  does nothing, while a replaced symlink (the kubelet's `..data` swap) is
  noticed. A file that is deleted or created also counts as a change; a file
  that cannot be read for a moment (e.g. permission denied) does not.
- vigild reacts only once the new content has been the same for one more
  check (debounce): Secret updates often change several keys one after
  another, and should cause one restart, not several. With the default
  interval the reaction comes 30–60 s after the change.
- With `on-change: signal` the process keeps running, so the next change is
  compared with the content at the time of the signal.
- Only a running service (`starting` or `active`) reacts. A stopped service
  reads the current files on its next start anyway.
- Not for `type: oneshot`: every run reads its files anew (a `watch-files`
  block there makes the layer invalid).
- Each reaction is logged with the paths (never the content) and counted in
  the metric `vigil_service_file_reloads_total{service}`.
- Exec checks need no watching: they resolve their templates on every run.

**Hanging file systems:** all file reads (templates and env files at start,
watched files) run outside vigild's event loop and are given up after
**10 seconds**. A read on a dead NFS hard mount cannot be cancelled by any
process; vigild therefore stops waiting for it, logs a warning, and starts no
further read of the same files until the hanging one returns. A start whose
files cannot be read in time fails (`error`, reason in the log); the rest of
vigild (other services, the API, reaping) keeps working.

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
  (that is, after they are ready, see [`ready`](#ready--ready-check--ready-timeout))
- `before: [c]` — syntactic sugar for `after` from the reverse direction:
  `B before: [A]` is equivalent to `A after: [B]`. B must be running before A starts.
- `requires: [a]` — implies `after` ordering **and** a runtime stop cascade:
  if `a` leaves the running state (Inactive, Backoff, Error), this service is
  stopped automatically. Use this for hard dependencies where the dependent
  service cannot function without the required one.

#### `ready` / `ready-check` / `ready-timeout`

When a started service counts as ready. Until then its status is `starting`,
and services that wait for it (`after`, `requires`, `before`) are not started.

| Value | Ready when |
|---|---|
| `process` | the process has been spawned. **Default** (v3 behaviour) |
| `notify` | the service sends `READY=1` to `$NOTIFY_SOCKET` (systemd's sd_notify protocol) |
| `check` | the check named in `ready-check` passes for the first time after the process was spawned |

```yaml
services:
  db:
    command: /usr/local/bin/db-server
    ready: notify                 # db-server calls sd_notify("READY=1")
  migrate:
    command: /usr/local/bin/migrate
    after: [db]                   # starts only when db sent READY=1
  web:
    command: /usr/local/bin/web
    ready: check
    ready-check: web-http         # first pass of this check makes web active
    ready-timeout: 1m

checks:
  web-http:
    startup: enabled              # the check must run
    period: 2s                    # short period = fast readiness
    http:
      url: http://localhost:8080/healthz
```

**`ready-timeout`** (default `30s`, `0s` = wait forever): if the service is not
ready in time, vigild stops it (`stop-signal`, then SIGKILL after
`kill-delay`). This exit always counts as a failure, so `on-failure` applies
(restart with backoff, shutdown, …), regardless of `success-exit-codes` and
`exit-code-map`. With `--exit-code propagate`, a resulting shutdown exits with
`10`, because the stop signal's code says nothing about the service.

**`ready: check`:** only probes that *started* after the process was spawned
count, so an earlier pass (e.g. of a previous process) cannot make a new
process ready. The check must exist and must not be `startup: disabled`. The
check keeps running afterwards as usual. Its `delay` and `period` decide how
quickly readiness is detected.

**`ready: notify`:**

- For every start vigild creates a datagram socket
  `<path-prefix>/notify/<service>-<random>.sock` (mode 0600, owned by the
  service user) and passes its path in `NOTIFY_SOCKET`, also with
  `pass-env: false`. The socket is removed when the process exits, so a
  leftover process of an earlier start cannot report into a later one.
- Understood messages: `READY=1`, `STATUS=<text>` (shown as `status-message`
  in the API and in `vigil services list`), `STOPPING=1` (logged). Other
  fields are ignored; file descriptors (`FDSTORE=1`) are closed. Watchdog
  messages are not supported yet.
- Only messages from the service's main process or its process group are
  accepted (checked via `SCM_CREDENTIALS`). A sender that has already exited
  when vigild reads the message (e.g. a short-lived `systemd-notify` helper)
  is accepted only if it ran as the service's UID.
- vigild never passes on a `NOTIFY_SOCKET` it got itself (e.g. from systemd);
  services without `ready: notify` do not see the variable at all.
- Directories: vigild creates `<path-prefix>` (if missing) and
  `<path-prefix>/notify` with mode 0711: services running as another `user`
  can reach their socket, but nobody can list the directory. If you create
  `<path-prefix>` yourself and use `ready: notify` together with `user:`,
  give it the search bit for others (`chmod 711`).

Senders: any sd_notify implementation works, e.g. `sd_notify(3)` from
libsystemd, the `sd-notify` crate, Python's `socket` module, or
`systemd-notify --ready` (if installed in the image).

#### `healthy-time`

How long a service must stay `active` without interruption before it counts
as healthy again. Default: **10s**; allowed **`1s` to `1h`**. It only
affects [alerts](#service-events): a service alert resolves once the service
is healthy, and the checks of a service send no alerts while it is not
healthy. A crash within `healthy-time` belongs to the same problem and sends
no new alert, so a crash loop does not flap between firing and resolved.

Oneshot jobs do not use `healthy-time`: a successful run makes them healthy.

```yaml
services:
  api:
    command: /usr/local/bin/api
    healthy-time: 30s   # slow warm-up: count as healthy after 30s
```

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
| `shutdown` | Stop all services and exit vigild with the service's exit code (see below) |
| `failure-shutdown` | Stop all and exit with a failure code: the service's non-zero code, or 10 (useful in `on-success`) |
| `success-shutdown` | Stop all and exit with code 0 (useful in `on-failure`) |

**vigild's exit code** depends on `--exit-code` / `VIGIL_EXIT_CODE`:

| Policy | `propagate` (default) | `fixed` (v3 behaviour) |
|---|---|---|
| `shutdown` | the service's code (after `exit-code-map`); `128 + signal` if it was killed by a signal | `0` on success, `10` on failure |
| `failure-shutdown` | the service's code if non-zero, else `10` | `10` |
| `success-shutdown` | `0` | `0` |

For `on-check-failure` there is no process exit code: `shutdown` and
`failure-shutdown` exit with `10`, `success-shutdown` with `0`.

#### `success-exit-codes` / `exit-code-map`

Control which exits count as success (and therefore pick `on-success` or
`on-failure`) and which code vigild propagates.

```yaml
services:
  worker:
    command: /usr/local/bin/worker
    exit-code-map: {143: 0}       # SIGTERM (128+15) counts as a clean exit
    success-exit-codes: [0, 2]    # 2 = "nothing to do" is also a success
    on-success: shutdown
    on-failure: shutdown
```

**Signals become exit codes.** A process killed by a signal has no exit code
of its own. vigild converts it the same way a shell does (`$?`):
**128 + signal number**.

| Signal | Number | Code |
|---|---|---|
| SIGHUP | 1 | 129 |
| SIGINT | 2 | 130 |
| SIGKILL | 9 | 137 |
| SIGTERM | 15 | 143 |

So `exit-code-map: {143: 0}` means "a process that died from SIGTERM counts
as a clean exit". This is useful for programs that do not handle SIGTERM
(e.g. many JVM apps or shell scripts without a `trap`): without the mapping,
every regular stop is a failure and the container ends with 143.

**Order of evaluation** when a process ends:

1. Take the exit code, or `128 + signal` if it was killed by a signal.
2. Apply `exit-code-map` (e.g. 143 → 0).
3. Check the mapped code against `success-exit-codes` to pick `on-success` or
   `on-failure`.
4. If that policy shuts vigild down, the mapped code is what vigild exits with
   (with `--exit-code propagate`).

`last-exit` in the API and the `Last-Exit` column always show the raw result
(`"signal": 15` in the API, `SIGTERM` in the CLI), before the mapping.

- `success-exit-codes` replaces the default `[0]`. If you list codes, include
  `0` yourself when it should still be a success.
- In layer merges, `exit-code-map` entries are merged and a non-empty
  `success-exit-codes` replaces the previous list.

The last exit of every service is shown by `vigil services list` (column
`Last-Exit`), in the API (`last-exit` in `GET /v1/services`) and as the metric
`vigil_service_last_exit_code`.

#### `on-check-failure`

Map of `<check-name>: <policy>`. When the named check transitions to `Down`,
the policy is applied to this service. Same values as `on-success`/`on-failure`.

#### Backoff

When a service fails and `on-failure` is `restart`, vigild waits before each
restart, longer every time:

```
delay_n = min(backoff-delay * backoff-factor^n, backoff-limit)
```

Once the delay has grown to `backoff-limit`, vigild gives up at the next
failure: the service goes to `error` (alert event `error`, see [service
events](#service-events)) and stays there until you start it again.

Example with the defaults (`500ms`, `2.0`, `30s`): restarts after
`500ms → 1s → 2s → 4s → 8s → 16s`, the 7th failure in a row puts the service
into `error`.

The sequence starts again from `backoff-delay` when the service has been
running for [`healthy-time`](#healthy-time) (or a oneshot run succeeded), and
on a manual `vigil start` or `vigil restart`. So only failures in a row count,
not failures days apart.

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
| `merge` | Fields set in this layer override the same fields of earlier layers; fields this layer does not set keep their earlier value (also `startup`, and `level`/`startup` of checks). Lists (`after`, `before`, `requires`, `environment`) are merged. **Default.** |
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

    # --- Alerts ---
    service: myservice                 # quiet while this service is not healthy

    # --- Check type (exactly one) ---
    http:
      url: https://localhost:8080/healthz
      headers:
        Authorization: "Bearer secret"
        Content-Type: "application/json"
      no-follow-redirects: true              # do not follow HTTP redirects (default: true)
      insecure: false                        # skip TLS verification (default: false)
      insecure-proxy: false                  # skip TLS verification for HTTPS proxy (default: false)
      ca: /etc/vigil/certs/internal-ca.pem  # custom CA for TLS verification (optional)
      # success-statuses: [200, 204, 301]   # default: any 2xx

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

How often the check runs. Default: **10s**; allowed **`1s` to `24h`**. Every
period runs a probe (an exec check starts a process), so a shorter period
would only flood the pod or the checked endpoint. A value outside the range
makes the layer invalid.

#### `timeout`

How long to wait for the check to complete before counting it as a failure.
Default: **3s**.

#### `threshold`

Number of consecutive failures required before the check is declared `Down` and
`on-check-failure` actions are triggered. Default: **3**.

#### `service`

The service this check tests. While that service is not healthy (see
[`healthy-time`](#healthy-time)), the check sends no alerts: the service alert
already says what is wrong. Its status is still recorded; once the service is
healthy again, a check that is still down alerts then, one that recovered
meanwhile does not alert at all. See [Checks of an unhealthy
service](#checks-of-an-unhealthy-service).

Only needed for checks the service does not name itself: a check in the
service's `ready-check` or `on-check-failure` belongs to it anyway. The
service must be defined.

#### HTTP check

```yaml
http:
  url: https://localhost:8080/healthz
  headers:
    Authorization: "Bearer secret"
    Content-Type: "application/json"
  no-follow-redirects: true               # do not follow HTTP redirects (default: true)
  insecure: true                          # skip TLS certificate verification
  insecure-proxy: true                    # skip TLS verification for HTTPS proxy
  ca: /etc/vigil/certs/internal-ca.pem   # PEM CA cert (or chain) to verify the server
  success-statuses: [200, 204, 301]       # optional; default: any 2xx
```

The check passes if the HTTP response status is 2xx (default), or one of the
codes listed in `success-statuses`. The `url` field supports `http://` and
`https://`.

| Field | Default | Description |
|-------|---------|-------------|
| `url` | — | HTTP or HTTPS URL to request |
| `headers` | `{}` | Extra request headers (repeatable key/value map) |
| `no-follow-redirects` | `true` | Do not follow HTTP redirects. When `true`, the probe sees the raw redirect response (e.g. 301, 302, 303) instead of the final destination. |
| `insecure` | `false` | Skip TLS certificate verification (self-signed certs) |
| `insecure-proxy` | `false` | Skip TLS certificate verification for the HTTPS proxy itself. See note below. |
| `ca` | — | PEM file with CA certificate(s) to verify the server's TLS. Supports chain files with multiple concatenated PEM blocks. |
| `success-statuses` | `[]` | Explicit list of status codes that count as success. Empty = any 2xx. Example: `[200, 204, 301]` |

`insecure` and `ca` are mutually exclusive in intent — `ca` verifies with a
custom root, `insecure` skips verification entirely.

> **Note on `insecure-proxy`:** The HTTP client library (reqwest) does not
> support separate TLS configurations for proxy and target connections.
> Setting `insecure-proxy: true` therefore disables TLS certificate
> verification for **all** connections in that check client (proxy and target
> server). Use only when the proxy uses a self-signed or untrusted certificate.

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

`service-context` inherits the environment variables (including `env-file`),
user, group, and working directory from the named service. Individual fields
(`user`, `user-id`, `group`, `group-id`, `working-dir`, `environment`) override
the inherited values.

[Templates](#templates-in-command-and-environment) in the service's and the
check's `environment` are resolved on every check run, the same way as for the
service; the check's values can use the service's (`{{ env.DB_USER }}`). The
check's `command` is a shell script (`sh -c`) and is **not** a template: use
`$VAR` there, so that a value can never become shell code.

Exec checks follow the same environment rules as services (see
[`pass-env`](#environment--pass-env--pass-env-glob--pass-env-regex)): they
accept `pass-env`, `pass-env-glob` and `pass-env-regex` as well. The check's
`pass-env` overrides the service context's; the glob and regex lists are
combined with the service context's lists.

---

## Alerts

Alerts send HTTP(S) notifications for events of checks, services and vigild
itself. An alert subscribes to them with `on-check`, `on-service` and
`on-vigild`. They fire on **changes only** — not on every check cycle or
every restart — and support four wire formats to integrate with common
observability stacks.

```yaml
alerts:
  ops:
    url: https://hooks.example.com/vigil
    on-check: [website-alive]   # check down / recovered
    on-service: ["*"]           # every service: failures / healthy again
    on-vigild: [layers]         # invalid layers with --watch-layers
```

Every event has a source (`check`, `service`, `vigild`), a name (of the check
or service; `layers` for vigild), an event name, a status (`down` for a
problem, `up` for its end) and a message. Events are deduplicated per source
and name: a problem is sent when it differs from the last one sent, its end
only after a problem was sent.

### Check events

| Transition | Alert sent? |
|---|---|
| First check result: Up | **No** — suppressed to avoid spurious recovery on startup |
| First check result: Down | **Yes** — firing |
| Up → Down | **Yes** — firing |
| Down → Up | **Yes** — resolved / recovered |
| Down → Down | **No** — deduplicated |
| Up → Up | **No** — deduplicated |

Event names: `failed` (down) and `recovered` (up).

### Service events

One event per failure, named after what vigild does next:

| Event | When | Example message |
|---|---|---|
| `backoff` | The process failed and vigild restarts it | `exited with code 3, restarting in 2s` |
| `error` | vigild gives up (backoff limit reached), or the start failed | `failed to start: …` |
| `failed` | Any other failure: a oneshot or cron run failed, `on-failure: ignore`, a shutdown policy | `did not become ready within 30s` |
| `recovered` | The service is healthy again: `active` for [`healthy-time`](#healthy-time), or a oneshot run succeeded | `service api is healthy again` |

A crash loop sends one `backoff`, then one `error` when vigild gives up; a
crash within `healthy-time` sends nothing new. A process that exits with
success (e.g. a service restarted by `on-success: restart`) and a stop
requested by you or by vigild (`vigil stop`, replan, `watch-files`) are no
failure.

With a shutdown policy (`on-failure: shutdown`) vigild exits right after the
failure; the alert is queued, but may not be delivered.

### Checks of an unhealthy service

When a service crashes, the checks testing it fail too. To send one alert
instead of several, a check stays quiet while a service it belongs to is not
healthy: starting, within `healthy-time`, in backoff or error, or stopped. Its
status is still recorded. Once the service is healthy again, a check that is
still down alerts then; one that recovered meanwhile does not alert at all.

A check belongs to a service when the service names it in `ready-check` or
`on-check-failure`, or the check names the service in
[`service`](#service). A check without such a link always alerts. A check that
belongs to a service that is never started (e.g. `startup: disabled`) never
alerts.

### vigild events

| Name | Event | When |
|---|---|---|
| `layers` | `invalid` | `--watch-layers` found layers that do not load; the running plan stays active |
| `layers` | `recovered` | The next reload succeeded |

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

    on-check: [website-alive]   # checks that trigger this alert ("*" = all)
    on-service: [api, worker]   # services whose failures trigger it ("*" = all)
    on-vigild: [layers]         # vigild events ("*" = all)

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
    # Variables: source, name, event, status, message, check, timestamp,
    # labels (map), info (map)
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
when any of the listed checks transitions between `Up` and `Down`. `"*"`
means every check.

#### `on-service`

List of service names whose [failures and recovery](#service-events) trigger
this alert. `"*"` means every service. Names of services that do not exist
(yet) are allowed.

#### `on-vigild`

List of [vigild events](#vigild-events) that trigger this alert: `layers` or
`"*"`. Other names make the layer invalid.

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
| `alertmanager` | Alertmanager `labels` map (+ `alertname`, `source` and, for checks, `check` added automatically) | `annotations` map (+ `event`, `message`) |
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
| `source` | string | `"check"`, `"service"` or `"vigild"` |
| `name` | string | Name of the check or service; `"layers"` for vigild |
| `event` | string | Event name, e.g. `"failed"`, `"backoff"`, `"recovered"` |
| `status` | string | `"down"` (firing) or `"up"` (recovery) |
| `message` | string | Detail, e.g. `"exited with code 3, restarting in 2s"` |
| `check` | string | Name of the check for check events; undefined (renders empty) otherwise |
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
  "source": "check",
  "name": "website-alive",
  "event": "failed",
  "status": "down",
  "message": "check website-alive failed",
  "check": "website-alive",
  "timestamp": "2026-03-21T12:00:00Z",
  "labels": { "env": "production" },
  "info":   { "k8s_namespace": "prod" }
}
```

`status` is `"down"` on firing, `"up"` on recovery. `check` is only present
for check events; a service event looks like this:

```json
{
  "source": "service",
  "name": "api",
  "event": "backoff",
  "status": "down",
  "message": "exited with code 3, restarting in 2s",
  "timestamp": "2026-03-21T12:00:00Z",
  "labels": { "env": "production" },
  "info":   { "k8s_namespace": "prod" }
}
```

When `body-template` is set, vigild renders the template and sends the result
instead. This lets you produce any JSON shape required by the target system
(Slack, Teams, PagerDuty, n8n, Zapier, …). See [`body-template`](#body-template)
for the full variable reference.

#### `alertmanager` — Prometheus Alertmanager v2

```json
[{
  "labels":      { "alertname": "website-alive", "source": "check", "check": "website-alive", "env": "production" },
  "annotations": { "event": "failed", "message": "check website-alive failed", "runbook": "https://wiki.example.com/…" },
  "startsAt":    "2026-03-21T12:00:00Z",
  "endsAt":      "0001-01-01T00:00:00Z"
}]
```

On recovery `endsAt` is set to the current time — this is the native
Alertmanager mechanism for marking an alert as resolved. Alertmanager tells
alerts apart by their labels, so `event` and `message` are annotations: the
resolved event (`recovered`) has the same labels as the problem it ends.

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
    "source": "check",
    "name": "website-alive",
    "event": "failed",
    "status": "down",
    "message": "check website-alive failed",
    "check": "website-alive",
    "labels": { "env": "production" },
    "info":   { "k8s_namespace": "prod" }
  }
}
```

`type` is `io.vigil.<source>.<event>`: `io.vigil.check.failed` and
`io.vigil.check.recovered` for checks, e.g. `io.vigil.service.backoff` or
`io.vigil.vigild.invalid` for the other sources. `data.check` is only present
for check events.

#### `otlp-logs` — OpenTelemetry OTLP HTTP/JSON

Compatible with `POST /v1/logs` on any OpenTelemetry Collector.
`severityNumber` is `17` (ERROR) on Down and `9` (INFO) on Up. The body is
the message. Attributes: `vigil.source`, `vigil.name`, `vigil.event`, for
check events also `check.name` and `check.status`, plus `labels` and
`send_info_fields`.

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
| `open` | `GET /v1/system-info`, `GET /v1/checks`, API docs — no authentication required |
| `metrics` | `GET /v1/metrics` (Prometheus/OpenMetrics) |
| `read` | All other `GET` endpoints |
| `write` | `read` + service/check control (`POST /v1/services`, `POST /v1/checks`) |
| `admin` | `write` + identity management (`POST /v1/identities`, `DELETE /v1/identities`) |

### Default access (caller without a matching identity)

| Connection | Effective level |
|------------|----------------|
| Unix socket, UID 0 (root) or daemon UID, no identities configured yet | `admin` (bootstrap) |
| Unix socket, any other UID, or once identities exist | `open` |
| TLS listener | `--tls-anonymous-access`: `metrics` (default) or `open` |

Logs, services and every state change need an identity over TLS. For a
log shipper such as vigil-log-relay, create an identity with `access: read`
(Basic Auth or TLS client certificate).

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
