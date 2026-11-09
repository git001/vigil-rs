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
```

vigild reads layer files strictly: a key it does not know (a typo such as
`comand`, `retry_attempts` instead of `retry-attempts`, or a key in the wrong
block) or a value that is not allowed is an error that names the key and
lists the valid ones. At startup vigild then does not start; on `vigil
replan` or `--watch-layers` the running plan stays. Nothing is silently
ignored.

A replan (`vigil replan`, `--watch-layers`) applies a changed plan without
restarting vigild: new services with `startup: enabled` start, removed ones
stop, and running services whose configuration changed are restarted
together with the services that require them. Stopped services get the new
configuration at their next start. See [`vigil replan`](cli-reference.md#replan).

```
Error: parsing "/etc/vigil/layers/001-base.yaml"

Caused by:
    services.app: unknown field `comand`, expected one of `override`, `summary`, `description`, `command`, … at line 3 column 5
```

YAML anchors and aliases work within one layer file: set the anchor where a
value is used first and refer to it later. Merge keys (`<<: *base`) and extra
keys that only hold anchors (such as `x-common:`) are unknown keys and
therefore errors. Anchors do not reach into other layer files.

```yaml
alerts:
  ops:
    url: "env:ALERTMANAGER_URL"
    # &alert-env names this list, so other alerts can reuse it.
    pass-env-glob: &alert-env [ALERTMANAGER_*, ONCALL_*, CLUSTER_NAME]
  oncall:
    url: "env:ONCALL_WEBHOOK_URL"
    # *alert-env inserts the list named above.
    pass-env-glob: *alert-env
```

Layer files can also define identities (`identities:`); see
[Identities](#identities).

---

## Services

### Full reference

The block lists every field. Some of them exclude each other, so it is not a
valid service as it stands: `schedule` needs `type: oneshot`, which in turn
rules out `ready`, `ready-check`, `ready-timeout`, `watch-files` and
`on-success: restart`; `ready-check` needs `ready: check`. The field
reference below has the rules.

```yaml
services:

  myservice:
    # --- Identity ---
    summary: Short description          # for readers of the file; vigil does not show it
    description: Longer description (optional)

    # --- Process ---
    command: /usr/local/bin/myapp --config /etc/myapp.yaml --workers {{ cpu() }}  # required; {{ … }}: see "Templates in command and environment" below the block
    type: simple                       # simple | oneshot (default: simple)
    schedule: '0 3 * * *'              # cron pattern, type: oneshot only
    schedule-tz: Europe/Vienna         # time zone for schedule (default: UTC); only with schedule
    working-dir: /var/lib/myapp        # working directory (default: vigild's working directory)
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
    after:                             # start after these services are ready
      - database
      - cache
    before:                            # these services start after this one is ready
      - frontend
    requires:                          # like after, and stop when one of them stops for good
      - database
    ready: check                       # process | notify | check (default: process)
    ready-check: myservice-alive       # check that signals readiness (only with ready: check)
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
    logs-push-socket: /run/collector/input.sock  # push ndjson to a Unix socket
    logs-push-addr: 127.0.0.1:5170    # push ndjson to a TCP address (both may be set)
    logs-push-format: ndjson          # ndjson (default; currently the only supported format)
    logs-multiline:                    # join the lines of a stack trace into one entry
      preset: java                     # java | python | indent; or start: REGEX, or continue: REGEX
      timeout: 200ms                   # wait this long for a further line (default: 200ms)

    # --- Layer merging ---
    override: merge                    # merge (default) | replace
```

### Field reference

#### `command`

Required: a service without a `command` after all layers are merged makes
the layers invalid. A later layer may leave it out when an earlier one sets
it.

The command is split into arguments like a shell would, but without any
expansion: words are separated by whitespace, `'…'` and `"…"` keep spaces
together, and `\` escapes the next character (not inside `'…'`). Variables,
globs, pipes and redirections are not interpreted. To use shell features,
wrap in `sh -c`:

```yaml
command: sh -c "echo starting && exec /usr/local/bin/myapp"
```

A part in ` [ … ]` (with spaces around the brackets) marks default
arguments; vigild passes them like the other arguments:
`/usr/bin/app --verbose [ --port 8080 ]` runs `app --verbose --port 8080`.

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
with backoff, so a migration is retried). `vigil services start <name>` runs it again;
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
  is not armed. `vigil services start <name>` always runs the job once by hand.
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
- `replan` re-arms a changed schedule immediately. A run in progress is not
  restarted; it finishes with the old configuration.
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
vigild (other services, the API, reaping) keeps working. At most 16 such
reads run at a time; when that many hang, further ones wait until their
time limit runs out. A service or exec check that reads no files (no
`env-file`, no template, no `user`) does not need this at all.

#### `startup`

| Value | Behaviour |
|---|---|
| `enabled` | Service is started automatically when vigild starts (or after `replan`) |
| `disabled` | Service must be started explicitly via `vigil services start` or the API |

Default: `disabled`

#### `after` / `before` / `requires`

Startup ordering and dependency constraints. All referenced service names must
exist in the merged plan, a service cannot name itself, and the services must
not wait for each other in a circle (`a after: [b]`, `b after: [a]`); each of
these makes the layer invalid, and the error names the services involved.

- `after: [a, b]` — this service starts after `a` and `b` reach `Active`
  (that is, after they are ready, see [`ready`](#ready--ready-check--ready-timeout))
- `before: [c]` — syntactic sugar for `after` from the reverse direction:
  `B before: [A]` is equivalent to `A after: [B]`. B must be ready before A starts.
- `requires: [a]` — implies `after` ordering **and** a runtime stop cascade:
  if `a` stops for good (Inactive or Error), this service is stopped
  automatically. Backoff does not trigger it: `a` is restarted on its own
  then, and the dependent keeps running. Use this for hard dependencies where
  the dependent service cannot function without the required one.

The ordering applies when vigild starts services by itself: at startup, on
replan and with the `autostart` action. `vigil services start` starts the
named services at once.

A dependency that autostart does not start (`startup: disabled`, or a
`schedule`) behaves differently per key:

- `after` / `before`: only an ordering among services that start anyway, so
  the dependency is ignored and the service starts without it (unless the
  dependency is starting right now, then it waits for it).
- `requires`: a hard dependency, so the service waits until the dependency
  is started and ready, for example by `vigil services start <dependency>`.
  vigild does not start it on its own.

vigild logs a warning for both cases when it loads the layers, and
`vigil services list` shows `waiting for <dependency>` in the `Message`
column of a waiting service.

The whole decision as a flow chart:
[service-dependencies.svg](../../images/service-dependencies.svg) (rendered
by CI from `docs/user-guide/service-dependencies.dot`).

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

  > [!WARNING]
  > **This needs a `--path-prefix` of at most 78 bytes.** A Unix socket
  > path can be at most 107 bytes long (a Linux limit), and the notify
  > socket adds at least 29 (`/notify/<16 hex digits>.sock`; the service
  > name is left out when it does not fit). With a longer prefix vigild
  > rejects layers that have a `ready: notify` service: it does not start,
  > and a replan keeps the running plan. See
  > [Socket path limit](installation.md#socket-path-limit-107-bytes).
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
as healthy again. Default: **10s**; allowed **`1s` to `1h`**. A healthy
service starts a new backoff sequence: only exits in a row count towards
`backoff-limit`. For [alerts](#service-events): a service alert resolves
once the service is healthy, and the checks of a service send no alerts
while it is not healthy. A crash within `healthy-time` belongs to the same
problem and sends no new alert, so a crash loop does not flap between
firing and resolved.

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

Flow chart: [service-lifecycle.svg](../../images/service-lifecycle.svg)
(start and readiness, how vigild stops a service, how an exit picks
`on-success` or `on-failure`, restart with backoff).

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

`backoff-delay` must be greater than 0, `backoff-limit` at least
`backoff-delay` and `backoff-factor` at least `1.0`; other values make the
layer invalid. Once the delay has grown to `backoff-limit`, vigild gives up at
the next failure: the service goes to `error` (alert event `error`, see [service
events](#service-events)) and stays there until you start it again.

Example with the defaults (`500ms`, `2.0`, `30s`): restarts after
`500ms → 1s → 2s → 4s → 8s → 16s`, the 7th failure in a row puts the service
into `error`.

The sequence starts again from `backoff-delay` when the service has been
running for [`healthy-time`](#healthy-time) (or a oneshot run succeeded), and
on a manual `vigil services start` or `vigil services restart`. So only failures in a row count,
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

A service may set both; its entries then go to both targets. vigild retries
the connection with exponential backoff (500 ms → … → 30 s) if the collector
is not yet ready. On connection loss vigild reconnects automatically; lines
logged while it is disconnected are not pushed.

```yaml
services:

  myapp:
    command: /usr/local/bin/myapp
    logs-forward: disabled          # raw lines go to ring buffer only
    logs-push-socket: /run/collector/input.sock  # vigild connects here

  filebeat:
    command: sh -c 'filebeat run --strict.perms=false -c /etc/filebeat/filebeat.yml 2>/dev/null'
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

#### `logs-multiline`

Joins the lines of a stack trace into one log entry, together with the log
line before it. Exactly one of these sets the rule:

| Key | A line belongs to the entry before it when … |
|---|---|
| `preset: java` | it is indented, starts with `Caused by: `, or names an exception class (`java.lang.IllegalStateException: boom`) |
| `preset: python` | it is indented, is `Traceback (most recent call last):`, names an exception (`KeyError: 'id'`), is one of the sentences between chained exceptions, or is empty |
| `preset: indent` | it starts with a space or a tab |
| `start: REGEX` | it does not match (a matching line starts a new entry) |
| `continue: REGEX` | it matches |

`timeout` (default `200ms`, allowed `10ms` to `10s`) is how long an entry
waits for a further line. An entry is at most `--log-max-line-bytes` long
(default 128 KiB); a longer one is split. A later layer replaces the whole
block. Two rules at once, an invalid regex, an unknown preset or key, and
`logs-forward: passthrough` (vigild does not read the output then) are load
errors.

```yaml
services:
  api:
    command: java -jar /app/api.jar
    logs-multiline:
      preset: java
  worker:
    command: python3 /app/worker.py
    logs-multiline:
      start: '^\d{4}-\d{2}-\d{2} '   # each log line starts with a date
```

See [Logging](logging.md#stack-traces-logs-multiline) for where the joined
entry shows and how it looks.

#### `override`

Controls how this service definition merges with the same service in earlier
layers.

| Value | Behaviour |
|---|---|
| `merge` | Fields set in this layer override the same fields of earlier layers; fields this layer does not set keep their earlier value (also `startup`). **Default.** |
| `replace` | This definition completely replaces the earlier one. |

With `merge`, fields that hold several values combine like this:

| Fields | Merge |
|---|---|
| `after`, `before`, `requires`, `env-file`, `pass-env-glob`, `pass-env-regex` | Appended; an entry that is already there is not added again |
| `environment`, `exit-code-map`, `on-check-failure` | Merged by key; this layer's value wins for a key both set |
| `success-exit-codes` | Replaced as a whole when this layer sets it |
| `watch-files` | Merged per field (`on-change`, `interval`, `paths`) |

There is no way to remove a single value with `merge` (e.g. one entry of
`after`); use `override: replace` and write the whole service.

---

## Checks

Each check has exactly one probe: `http`, `tcp` or `exec`. A check with none
or with several makes the layer invalid.

### Full reference

```yaml
checks:

  myservice-alive:
    # --- Level ---
    level: alive                       # alive | ready (default: alive)
    startup: enabled                   # enabled | disabled (default: disabled: the check does not run)

    # --- Timing ---
    delay: 3s                          # wait before first check (default: 3s)
    period: 10s                        # how often to run the check (default: 10s)
    timeout: 3s                        # per-check timeout (default: 3s, at most period)
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
      tls:                                   # optional, see "TLS and proxy"
        ca: /etc/vigil/certs/internal-ca.pem
      # proxy:                               # optional, see "TLS and proxy"
      #   url: http://proxy.example.com:3128
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

    # --- Layer merging ---
    override: merge                    # merge (default) | replace
```

### Field reference

#### `startup`

| Value | Behaviour |
|---|---|
| `enabled` | The check runs: after `delay`, then every `period` |
| `disabled` | The check does not run. **Default.** |

A disabled check is listed with status `inactive` in `vigil checks list` and
`GET /v1/checks`, and is ignored by `GET /v1/health`, alerts and
`on-check-failure`. `vigil checks run` (`POST /v1/checks/refresh`) still runs
it once, without changing any state. A service cannot use it as its
`ready-check`. A replan that sets a running check to `disabled` stops it.

#### `override`

How this check merges with the same check in earlier layers: `merge`
(default) or `replace`, as for [services](#override). With `merge` a probe
block (`http`, `tcp`, `exec`) that this layer sets replaces the earlier one
as a whole. To switch a check to another probe type (e.g. from `exec` to
`http`), use `override: replace`: a merge would keep the old probe as well,
and a check with two probes is invalid.

#### `level`

| Value | Meaning |
|---|---|
| `alive` | Basic liveness — is the process responsive at all? |
| `ready` | Readiness — is the process ready to serve traffic? |

Both levels run the same way. The level matters for [`GET
/v1/health`](../api-reference/rest-api.md#get-v1health): `?level=alive`
looks only at `alive` checks, `?level=ready` at both. Use it to point a
Kubernetes liveness probe at the `alive` checks and the readiness probe at
all of them.

Choose the level by what a restart can fix:

- **`alive`: the own app in this container**, e.g. Tomcat, Keycloak or
  Spring Boot (`http://localhost:8080/actuator/health/liveness`). When it is
  down, restarting the container may help.
- **`ready`: a dependency on another host**, e.g. PostgreSQL, Kafka,
  another service or an external API. When it is down, a restart fixes
  nothing; with a liveness probe on `?level=alive` every replica would be
  restarted while the database is away. As `ready`, the pod only stops
  getting traffic and keeps running.

```yaml
checks:
  app-alive:                 # own app: a restart can fix it
    level: alive
    startup: enabled
    http:
      url: http://localhost:8080/actuator/health/liveness
  db-ready:                  # dependency: a restart cannot fix it
    level: ready
    startup: enabled
    tcp:
      host: postgres.db.svc.cluster.local
      port: 5432
```

The default is `alive`. vigild warns at load time when a running `alive`
check (set or by default) probes another host than this container
(anything but `localhost`, `*.localhost`, a loopback or unspecified
address):

```
WARN level: alive (the default), but it probes "postgres.db.svc.cluster.local:5432", not this container: while it is down, GET /v1/health?level=alive answers 502 and a liveness probe on it restarts the container, which does not fix a dependency; set level: ready for checks of dependencies check=db-ready
```

Exec checks are not inspected; give them the level of what they test. The
same applies to [`on-check-failure: restart`](#on-check-failure): it helps
for the own app, not for a dependency.

Flow chart: [check-levels.svg](../../images/check-levels.svg) (which level,
what `/v1/health` counts, what the probes do).

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
Default: **3s**; must be greater than 0. A timeout longer than `period` is
cut to `period`, so runs never overlap.

#### `threshold`

Number of consecutive failures required before the check is declared `Down` and
`on-check-failure` actions are triggered. Default: **3**; at least 1.

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
  tls:
    ca: /etc/vigil/certs/internal-ca.pem
  proxy:
    url: http://proxy.example.com:3128
    no-proxy: "localhost, .svc.cluster.local"
  success-statuses: [200, 204, 301]       # optional; default: any 2xx
```

The check passes if the HTTP response status is 2xx (default), or one of the
codes listed in `success-statuses`. The `url` field supports `http://` and
`https://`.

Every run opens a new connection (DNS, TCP, TLS handshake) and closes it
afterwards: the check tests that a client can connect, not only that an
open connection still answers. The time this takes is the metric
`vigil_check_connect_duration_seconds`, see
[Metrics](../operator-guide/metrics.md).

| Field | Default | Description |
|-------|---------|-------------|
| `url` | — | HTTP or HTTPS URL to request |
| `headers` | `{}` | Extra request headers (repeatable key/value map) |
| `no-follow-redirects` | `true` | Do not follow HTTP redirects. When `true`, the probe sees the raw redirect response (e.g. 301, 302, 303) instead of the final destination. |
| `tls` | — | CAs, client certificate and `insecure`, see [TLS and proxy](#tls-and-proxy). |
| `proxy` | — | The proxy, see [TLS and proxy](#tls-and-proxy). Without it the check connects directly: `HTTPS_PROXY` and similar variables in vigild's environment are not used. |
| `success-statuses` | `[]` | Explicit list of status codes that count as success. Empty = any 2xx. Example: `[200, 204, 301]` |

The HTTP client of a check is built when the layers are loaded: a `tls.ca`
that cannot be read or a `proxy.url` that is not a URL makes the layer
invalid. Every replan builds the client again, so a changed file behind
`tls:` takes effect with the next replan (`vigil replan`, or any change in
the layer directory with `--watch-layers`); a running check keeps its status
and failure count.

#### TLS and proxy

HTTP checks and alerts set up their outgoing HTTPS connections with the same
two blocks:

```yaml
tls:
  ca: /etc/vigil/certs/internal-ca.pem    # one path or a list of paths
  cert: /run/secrets/vigil/client.crt     # client certificate (mTLS)
  key: /run/secrets/vigil/client.key
  insecure: false                         # true: no certificate check at all
proxy:
  url: http://proxy.example.com:3128
  no-proxy: "localhost, .svc.cluster.local, 10.0.0.0/8"
```

| Field | Default | Description |
|-------|---------|-------------|
| `tls.ca` | — | PEM files with CA certificates trusted in addition to the system CAs: one path or a list. A file may hold several certificates. |
| `tls.cert` | — | PEM file with the client certificate (and its chain), for servers that require one. Needs `tls.key`. |
| `tls.key` | — | PEM file with the private key of `tls.cert` (PKCS#8, PKCS#1 or SEC1). |
| `tls.insecure` | `false` | Do not verify certificates at all. Only for self-signed test servers. |
| `proxy.url` | — | Proxy URL; required in a `proxy:` block. |
| `proxy.no-proxy` | — | Comma-separated hosts reached without the proxy. An entry matches the host and its subdomains (a leading `.` is ignored); CIDR ranges such as `10.0.0.0/8` match IP addresses. |

All paths must be absolute. The files are read when the layers are loaded;
an error names the setting and the path, never the file content. A later
layer replaces a whole `tls:` or `proxy:` block, never single keys of it.
Unknown keys in either block make the layer invalid.

**A proxy with a certificate from its own CA:** the HTTP client has no
separate TLS settings for the proxy. `tls.ca` and `tls.insecure` apply to the
connection to the proxy and to the server alike, so the proxy's CA goes into
`tls.ca`, next to the server's CA:

```yaml
tls:
  ca: [/etc/vigil/certs/internal-ca.pem, /etc/vigil/certs/proxy-ca.pem]
proxy:
  url: https://proxy.example.com:3129
```

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

The check passes if the command exits with code 0. stdout and stderr do not
appear in the container log. vigild keeps the last 4 KiB of both (in the
order they were written) and adds them to the reason of a failed run, see
[Why a check fails](#why-a-check-fails); the output of a passing run is
dropped.

`service-context` inherits the environment variables (including `env-file`),
user, group, and working directory from the named service, which must exist. Individual fields
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

### Why a check fails

Every failed run has a reason. vigild logs it at `warn` with each failure,
and again when the check goes down:

```
WARN check failed check=db-ready failures=1 threshold=3 error="connect 127.0.0.1:5432: Connection refused (os error 111)"
INFO check is down check=db-ready error="connect 127.0.0.1:5432: Connection refused (os error 111)"
```

| Probe | Example reasons |
|---|---|
| `http` | `HTTP 503 Service Unavailable` (a status that is not a success), `GET http://localhost:8080/healthz: Connection refused (os error 111)`, `GET https://api.internal/healthz: invalid peer certificate: UnknownIssuer`, `GET http://localhost:8080/healthz: timed out after 3s` |
| `tcp` | `connect localhost:5432: Connection refused (os error 111)`, `connect db:5432: timed out after 3s` |
| `exec` | `exit status 1: <output>`, `killed by signal 9`, `timed out after 3s: <output so far>` |

The reason of the last failed run is also:

- in `GET /v1/checks` as `last-error`, until a run passes;
- in the `Last error` column of `vigil checks list` (first line, cut to 60
  characters);
- in the alert message of a check that went down: `check db-ready failed:
  connect 127.0.0.1:5432: Connection refused (os error 111)`.

To try a check at once instead of waiting for its period, run `vigil checks
run db-ready` (`POST /v1/checks/refresh`). It prints the reason and exits
with 1 when the check fails, so it fits scripts; the run counts like a
scheduled one. A check with `startup: disabled` runs once without changing
any state, which helps to test a check before enabling it.

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

Event names: `failed` (down) and `recovered` (up). The message of `failed`
carries the reason of the run that took the check down, see [Why a check
fails](#why-a-check-fails).

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
requested by you or by vigild (`vigil services stop`, replan, `watch-files`) are no
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
  # Default: 256, at least 1
  max-queue-depth: 256

  # Maximum time a delivery job may wait before being silently discarded.
  # Prevents a flood of stale alerts when an endpoint comes back online
  # after a long outage. Default: 60s, at least 1s
  max-queue-time: 60s

  # Every other key is an alert and must be a mapping. A misspelled setting
  # (max-queue-depht: 5) is therefore an error that names the key.

  my-alert:
    # --- Endpoint ---
    # Supports "env:VAR". An unset variable makes the layer invalid.
    url: "env:ALERTMANAGER_URL"              # or a literal URL
    format: alertmanager   # webhook | alertmanager | cloud-events | otlp-logs

    # --- Environment ---
    # vigild's variables this alert may use in env:VAR values and as proxy
    # settings; the same rules as for services (default: only PATH, HOME,
    # USER, LOGNAME and TZ).
    pass-env-glob: [ALERTMANAGER_*, CLUSTER_NAME, KUBERNETES_*, HTTPS_PROXY, NO_PROXY]
    # pass-env-regex: ['ALERT_.*']
    # pass-env: true                         # all of vigild's environment

    on-check: [website-alive]   # checks that trigger this alert ("*" = all)
    on-service: [api, worker]   # services whose failures trigger it ("*" = all)
    on-vigild: [layers]         # vigild events ("*" = all)

    # --- Payload ---
    # Extra HTTP headers (e.g. authentication).
    headers:
      # env:VAR replaces the whole value: ALERTMANAGER_AUTH holds
      # "Bearer eyJhbGciOiJIUzI1NiJ9...".
      Authorization: "env:ALERTMANAGER_AUTH"

    # Labels added to the alert payload.
    # Values prefixed with "env:" come from vigild's environment, resolved
    # when the layers are loaded.
    labels:
      severity: critical
      env: production
      cluster: "env:CLUSTER_NAME"

    # Arbitrary key/value fields included in the alert body.
    # Placement depends on format (annotations in Alertmanager, attributes
    # in OTLP, info object in webhook/CloudEvents).
    # Values prefixed with "env:" come from vigild's environment.
    send-info-fields:
      k8s_namespace: "env:KUBERNETES_NAMESPACE"
      k8s_service:   "env:KUBERNETES_SERVICE_NAME"
      runbook: "https://wiki.example.com/runbooks/website-down"

    # --- TLS (see "TLS and proxy") ---
    tls:
      ca: /etc/vigil/certs/ca.pem

    # --- Proxy ---
    # If omitted, HTTPS_PROXY / ALL_PROXY / HTTP_PROXY (and NO_PROXY) are used
    # when pass-env-glob allows them (same precedence as the vigil CLI).
    proxy:
      url: "env:HTTPS_PROXY"
      no-proxy: "env:NO_PROXY"

    # --- Retry ---
    retry-attempts: 3                # total attempts including first (default: 3)
    retry-backoff: ["1s", "2s"]      # delays between attempts (default: ["1s", "2s"])
                                     # last entry is reused if list is shorter than
                                     # retry-attempts - 1

    # --- Custom body template (webhook format only) ---
    # Jinja2-style template rendered instead of the default webhook payload.
    # The rendered string must be valid JSON.
    # Variables: source, name, event, status, message, check, timestamp,
    # labels (map), info (map)
    # Ignored for formats other than webhook.
    body-template: '{"text": "Check {{ check }} is {{ status }}"}'

    # --- Layer merging ---
    # merge: scalars set here win; on-check, on-service, on-vigild,
    # pass-env-glob and pass-env-regex are appended; headers, labels and
    # send-info-fields are merged by key; retry-backoff is replaced.
    override: merge   # merge (default) | replace
```

### Field reference

#### `url`

Required. HTTP or HTTPS endpoint to POST alerts to. Supports `"env:VAR"`
(see below).

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

A value of the form `"env:VAR"` is replaced by the environment variable `VAR`
of vigild. This applies to `url`, `headers` values, `labels` values,
`send-info-fields` values, `proxy.url` and `proxy.no-proxy`. The prefix replaces the
whole value: for a header such as `Authorization: Bearer …`, the variable
holds the complete value including `Bearer `.

An alert sees vigild's environment under the same rules as a service
([`pass-env`](#environment--pass-env--pass-env-glob--pass-env-regex)): only `PATH`,
`HOME`, `USER`, `LOGNAME` and `TZ`, plus the variables its own
`pass-env-glob` / `pass-env-regex` select; `pass-env: true` shows all of
them. So a Secret in vigild's environment (e.g. `DB_PASSWORD`) cannot end up
in an alert unless the alert allows it.

```yaml
alerts:
  ops:
    url: "env:ALERTMANAGER_URL"
    pass-env-glob: [ALERTMANAGER_*, CLUSTER_NAME]
    headers:
      Authorization: "env:ALERTMANAGER_AUTH"
    labels:
      cluster: "env:CLUSTER_NAME"
```

vigild resolves the values when it loads the layers (at startup and on every
replan). A variable that is not set (or empty), or not allowed, makes the
layer invalid, with the alert, the field and the variable in the error:

```
alert "ops": headers.Authorization: environment variable ALERTMANAGER_AUTH is not set (from "env:ALERTMANAGER_AUTH")
alert "ops": url: environment variable ALERTMANAGER_URL is not passed to the alert; allow it with pass-env-glob or pass-env-regex (from "env:ALERTMANAGER_URL")
```

The same applies to the files an alert names: an unreadable `tls.ca`,
`tls.cert` or `tls.key`, or a `proxy.url` that is not a valid URL, makes the
layer invalid instead of sending without them.

#### `labels` / `send-info-fields`

Both accept a key/value map with optional `env:` resolution.
They differ in placement within the payload:

| Format | `labels` | `send-info-fields` |
|---|---|---|
| `webhook` | `labels` object | `info` object |
| `alertmanager` | Alertmanager `labels` map (+ `alertname`, `source` and, for checks, `check` added automatically) | `annotations` map (+ `event`, `message`) |
| `cloud-events` | `data.labels` object | `data.info` object |
| `otlp-logs` | LogRecord `attributes` | LogRecord `attributes` |

#### `body-template`

A [Jinja2](https://jinja.palletsprojects.com/)-compatible template string
(rendered by [minijinja](https://github.com/mitsuhiko/minijinja)) that replaces
the default webhook payload when `format: webhook` is configured.

The rendered string **must be valid JSON**. A template with a syntax error
makes the layer invalid (checked when the layers load). Errors that only show
when rendering (e.g. an unknown function) and output that is not valid JSON
make vigild log a `WARN` and send the default webhook payload instead — the
alert is never silently dropped.

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
| `info` | object | Resolved `send-info-fields` map |

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

#### `tls` / `proxy`

The same blocks as for HTTP checks, see [TLS and proxy](#tls-and-proxy).
`proxy.url` and `proxy.no-proxy` may be `env:VAR`.

Without a `proxy:` block, the alert uses the first of `HTTPS_PROXY`,
`https_proxy`, `ALL_PROXY`, `all_proxy`, `HTTP_PROXY`, `http_proxy` from
vigild's environment that its `pass-env-glob` / `pass-env-regex` allows (the
same order as the vigil CLI), and likewise `NO_PROXY` / `no_proxy`. With a
`proxy:` block, only its own `no-proxy` applies. Proxy variables the alert does not allow are not used, so an
alert without `pass-env-glob: [HTTPS_PROXY, NO_PROXY]` (or similar) connects
directly.

#### `retry-attempts` / `retry-backoff`

| Field | Default | Description |
|---|---|---|
| `retry-attempts` | `3` | Total number of send attempts (1 = no retry) |
| `retry-backoff` | `["1s", "2s"]` | Delays between attempts as duration strings. If shorter than `retry-attempts - 1`, the last entry is reused. |

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
  "message": "check website-alive failed: HTTP 503 Service Unavailable",
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
  "annotations": { "event": "failed", "message": "check website-alive failed: HTTP 503 Service Unavailable", "runbook": "https://wiki.example.com/…" },
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
    "message": "check website-alive failed: HTTP 503 Service Unavailable",
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
`send-info-fields`.

### Example

```yaml
alerts:

  # Alertmanager — URL and token from environment, custom retry
  alertmanager-prod:
    url: "env:ALERTMANAGER_URL"
    pass-env-glob: [ALERTMANAGER_*, CLUSTER_NAME]
    format: alertmanager
    on-check: [website-alive]
    headers:
      Authorization: "env:ALERTMANAGER_AUTH"      # "Bearer <token>"
    labels:
      severity: critical
      cluster:  "env:CLUSTER_NAME"
    send-info-fields:
      runbook: "https://wiki.example.com/runbooks/website-down"
    retry-attempts: 5
    retry-backoff: ["1s", "2s", "5s", "10s"]

  # OTLP — via corporate proxy
  otlp-prod:
    url: http://otel-collector:4318/v1/logs
    pass-env-glob: [KUBERNETES_NAMESPACE]
    format: otlp-logs
    on-check: [website-alive]
    proxy:
      url: "http://corp-proxy:3128"
      no-proxy: "localhost, .internal, 10.0.0.0/8"
    labels:
      service.name:            vigild
      service.namespace:       "env:KUBERNETES_NAMESPACE"
      deployment.environment:  production

  # Slack — body-template produces the incoming-webhook JSON shape
  slack:
    url: "env:SLACK_WEBHOOK_URL"
    pass-env-glob: [SLACK_WEBHOOK_URL, CLUSTER_NAME]
    format: webhook
    on-check: [website-alive]
    labels:
      cluster: "env:CLUSTER_NAME"
    body-template: >-
      {"text": ":{% if status == 'down' %}red_circle{% else %}large_green_circle{% endif %} *{{ check }}* is *{{ status }}* on `{{ labels.cluster }}`"}

  # Microsoft Teams — MessageCard format via body-template
  teams:
    url: "env:TEAMS_WEBHOOK_URL"
    pass-env-glob: [TEAMS_WEBHOOK_URL, CLUSTER_NAME]
    format: webhook
    on-check: [website-alive]
    labels:
      cluster: "env:CLUSTER_NAME"
    send-info-fields:
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

All duration fields accept strings like these: `kill-delay`, `ready-timeout`,
`healthy-time`, `backoff-delay`, `backoff-limit`, `watch-files.interval`,
`delay`, `period`, `timeout`, `retry-backoff` entries and
`alerts.max-queue-time`.

| Example | Meaning |
|---|---|
| `500ms` | 500 milliseconds |
| `3s` | 3 seconds |
| `1m` | 1 minute |
| `1m30s` | 1 minute 30 seconds |
| `1h` | 1 hour |
| `0s` | zero, where a field allows it (e.g. `delay`, `ready-timeout`) |

Each part is a whole number followed by a unit: `ms`, `s`, `m` or `h`.
Fractions (`1.5s`), other units (`d`, `us`) and a number without a unit
(`0`, `30`) are errors that make the layer invalid; write `0s` for zero.

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
    pass-env-glob: [CLUSTER_NAME, KUBERNETES_NAMESPACE]
    format: alertmanager
    on-check: [postgres-ready, myapp-alive]
    labels:
      severity: critical
      env:      production
      cluster:  "env:CLUSTER_NAME"
    send-info-fields:
      k8s_namespace: "env:KUBERNETES_NAMESPACE"
```

---

## Identities

Identities control who may call the vigild REST API and what they are allowed
to do. They are optional — if no identities are configured the daemon applies
default access rules based on connection type (see below).

Identities come from two sources:

- **Layer files** (`identities:`, as in the examples below). They survive a
  restart and can come from a ConfigMap, e.g. the read identity of
  vigil-log-relay. Over the API they are read-only: `POST` or `DELETE` with
  their name fails with `409` and the layer file that defines them. A replan
  (also `--watch-layers`) replaces them as a whole, so an identity removed
  from the layers loses its access at once. A later layer file replaces a
  whole identity; there is no `override` for identities.
- **The API**: `vigil identities add-local`, or `POST /v1/identities` with a
  body like the examples below (as JSON). vigild keeps them in memory, so they
  are gone after a restart. A layer identity of the same name replaces one
  added over the API.

`vigil identities list` shows the source of each identity:

```
Name                     Access   Auth                     Source
------------------------------------------------------------------------
log-relay                read     basic                    layer 001-auth
ops                      write    local(any)               api
```

vigild checks layer identities when it loads the layers: every identity needs
`access` and at least one of `local`, `basic` or `tls`; a password hash must
be a SHA-512 crypt hash, a CA certificate valid PEM. Errors name the identity
and its layer file:

```
identity "log-relay" (layer "001-auth"): password-hash-file: reading "/run/secrets/vigil/log-relay.hash"
```

### Access levels

Each level includes everything of the levels above it.

| Level | Grants access to |
|-------|-----------------|
| `open` | `GET /v1/system-info`, `GET /v1/checks`, `GET /v1/health`, API docs — no authentication required |
| `metrics` | `GET /v1/metrics` (Prometheus/OpenMetrics) |
| `read` | The other `GET` endpoints: services, logs, alerts, changes |
| `write` | `read` + changes: `POST /v1/services`, `POST /v1/replan`, `POST /v1/signals`, `POST /v1/checks/refresh` |
| `admin` | `write` + `POST /v1/vigild` (stop, restart) and identity management (`GET`, `POST`, `DELETE /v1/identities`) |

### Default access (caller without a matching identity)

| Connection | Effective level |
|------------|----------------|
| Unix socket, UID 0 (root) or daemon UID, no identity added over the API yet | `admin` (bootstrap) |
| Unix socket, any other UID, or once an identity was added over the API | `open` |
| TLS listener | `--tls-anonymous-access`: `metrics` (default) or `open` |

Only identities added over the API end the bootstrap. Layer identities alone,
e.g. just a read identity for vigil-log-relay, keep it: root and vigild's own
UID stay `admin` on the Unix socket. Other UIDs cannot connect anyway while
the socket keeps its default mode `600` (`--socket-mode`). To end the bootstrap from a layer file,
add an identity over the API, or restrict the socket with a `local` identity
of your own and manage the rest from there.

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

In a layer file the hash can also come from a file, e.g. a mounted
Kubernetes Secret, with `password-hash-file` instead of `password-hash`
(exactly one of the two). The path must be absolute; a missing or empty file
makes the layer invalid. Surrounding whitespace such as the trailing
newline of a Secret is removed. With `--watch-layers`, vigild also watches the
content of the file, so a rotated Secret applies without a change to the
layer.

```yaml
identities:

  log-relay:
    access: read
    basic:
      password-hash-file: /run/secrets/vigil/log-relay.hash
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

In a layer file, `tls.ca` names the PEM files with the CA certificates: one
absolute path or a list, each file with one certificate or several. It is
the same `ca` as in the `tls:` block of HTTP checks and alerts (see
[TLS and proxy](#tls-and-proxy)), but the only key allowed here: `insecure`
would let any certificate in, and `cert` and `key` have no meaning for an
identity. The files are read when the layers are loaded; `--watch-layers`
watches them, so a rotated CA takes effect with the next replan.

```yaml
identities:

  ci-pipeline:
    access: write
    tls:
      ca: /etc/vigil/certs/ci-ca.pem

  prometheus:
    access: metrics
    tls:
      ca: [/etc/vigil/certs/monitoring-ca.pem, /etc/vigil/certs/monitoring-ca-next.pem]
```

Over the REST API, an identity carries the CA as a PEM string in
`tls.ca-cert` instead, see the [API reference](../api-reference/rest-api.md).

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
      ca: /etc/vigil/certs/monitoring-ca.pem
```
