# vigil-rs v4 plan: security fixes and new features

Status: 2026-11-03, based on v3.2.1 (`4d78710`).
Background: feature comparison with gopherd and its commit `75e38f4`
("close trust-boundary gaps between PID 1 and services").

**Target version: v4.0.** The new features and changed defaults
(`pass-env`, new `Starting` status, exit codes) make this a major release.

## Principle: everything must run on OpenShift

Under OpenShift (`restricted-v2` SCC):

- vigild runs with a random UID (e.g. `1000680000`) and GID 0.
- There are no capabilities, so no `CAP_SETUID` or `CAP_SETGID`.
- `/run` is a root-owned tmpfs and not writable
  (see `docs/operator-guide/orchestrators.md`).
- With `readOnlyRootFilesystem`, only `emptyDir` mounts are writable.

Consequences for every new file and socket:

- **One directory for everything: `--path-prefix` / `VIGIL_PATH_PREFIX`.**
  Everything vigild creates at runtime goes there, never hard-coded to `/run`.
  There is no fallback chain; the path is explicit.
  - Default: `/run/vigil`. On OpenShift e.g. `/data/my-vigil` or `/tmp/vigil`.
  - Contents:
    ```
    <prefix>/vigild.sock                control socket (default for --socket)
    <prefix>/notify/<svc>-<rand>.sock   sd_notify (phase 2)
    ```
  - `--socket` remains as an override. Its default is now
    `<prefix>/vigild.sock`, for the `vigil` client as well.
- **Checked at startup; vigild aborts with a clear message on failure:**
  - The directory must exist and must not be a symlink (`lstat`).
  - Owned by vigild's effective UID.
  - Not writable by others (no `o+w`). `g+w` is allowed because OpenShift
    images commonly use `chmod g=u` for GID 0.
  - Writable for vigild (checked with `access(W_OK)`).
  - The error names the path, the owner and mode found, and what is expected.
- **Subdirectories** such as `notify/` are created by vigild with mode 0711
  (see phase 2: services running as another user must reach their socket).
- **Creating the last component:** if the last path component is missing and
  its parent exists, vigild creates it with mode 0711 and then checks it as
  above. Deeper missing levels are an error. Reason: an `emptyDir` mount point
  is root-owned with mode 0777 and would be rejected as a prefix itself; this
  way `emptyDir` on `/data` works with `--path-prefix /data/my-vigil` without
  an initContainer.
- **No required capabilities:** no feature may depend on capabilities. The
  user-switching code only runs when `user` or `group` is configured.

## Phase 0: security fixes

### S1 Privileges only partly dropped when switching user

**Problem:** `crates/vigild/src/service/actor/spawn.rs` and
`crates/vigild/src/check/probe/exec.rs` only called `setgid` and `setuid`.

- Root's supplementary groups were kept, because `setgroups` was never called.
- With `user:` but no `group:`, `resolve_gid` returned `None`
  (`crates/vigild/src/process_util/identity.rs`), so no `setgid` happened at
  all and the primary GID stayed 0.

**Affected:** only setups where vigild runs as root and `user:` is configured
(Docker, Podman, VMs).

**OpenShift is not affected.** No `user:` is set there, so the `pre_exec`
branch never runs and every process keeps the assigned UID. GID 0 is intended
there: OpenShift expects image files to be owned and writable by group 0. With
`user:` set, `setuid` already fails with `EPERM`.

**Fix:** shared helpers `process_util::resolve_credentials` and
`process_util::apply_credentials` for services and exec checks.

- Only run when `user` or `group` is configured. Without them `setgroups` is
  never called, because it needs `CAP_SETGID` and would break OpenShift.
- Without `group`, the user's primary GID from the passwd database is used.
- The group list is resolved before fork (`getgrouplist`). `pre_exec` only
  calls `setgroups`, `setgid` and `setuid`, because nothing may be allocated
  there.
- If vigild is unprivileged and the target UID/GID equal its own (common for
  rootless setups), nothing is changed and no error occurs.
- After `setuid`, verify that `setuid(0)` fails.

**Tests:** unit tests for credential resolution; an integration test as root
checks `id -G` in the child; a test without root and without `user:` makes
sure no credential syscall is made.

**Status:** done in v3.2.2. For a numeric `user-id` without passwd entry and
without group, 3.2.2 only logs a warning and uses GID = UID. On the `v4`
branch this is now an error (done).

### S2 Child processes inherit vigild's environment

**Problem:** neither services nor exec probes call `env_clear()`. vigild's
environment (tokens, passwords) ends up in every child process.

**Decision:** `pass-env` defaults to `false`.

**Fix:**

- Three separate fields per service:
  ```yaml
  pass-env: false                         # default
  pass-env-glob: [DATABASE_URL, "APP_*", "*_APP_*_LAL"]
  pass-env-regex: ['^K8S_[A-Z]+_URL$', 'TOKEN_(A|B)']
  ```
  - `pass-env: false`: `env_clear()` plus `PATH`, `HOME`, `USER`, `LOGNAME`
    and `TZ`, plus everything matched by `pass-env-glob` or `pass-env-regex`,
    plus `environment:`.
  - `pass-env: true`: vigild's entire environment. Setting `pass-env-glob` or
    `pass-env-regex` as well is a validation error, since they would have no
    effect.
  - `pass-env-glob`: YAML list. `*` (any number of characters) and `?` (one
    character) anywhere; the glob must match the **whole** name. Small
    hand-written matcher, no crate needed.
  - `pass-env-regex`: YAML list (a single string with `A|B` also works). Uses
    the `regex` crate, already a workspace dependency. Each expression is
    anchored as `^(?:…)$` so that `TOKEN` does not accidentally match
    `MY_TOKEN_SECRET`; use `.*TOKEN.*` for partial matches.
  - Glob and regex matches are combined. Expressions are compiled when the
    plan is loaded; invalid ones are an error.
  - Overlay merge: lists are appended (like `after`), `pass-env` overrides.
  - `vigil plan` shows the patterns. Which variables are actually passed is
    only logged at debug level, as names, never with values.
- Exec checks in a service context get exactly the same environment.
- vigild calls `prctl(PR_SET_DUMPABLE, 0)` at startup. This needs no
  capabilities and only affects vigild itself.

**Why this matters on OpenShift/Kubernetes:**

- vigild and all services run with the same UID there. Without
  `PR_SET_DUMPABLE=0` every service can read `/proc/1/environ`, so
  `env_clear()` alone does not protect anything. With `PR_SET_DUMPABLE=0`,
  `/proc/1/*` is owned by root and no longer readable for the same UID.
- Variables from the Deployment (`env:`, `envFrom:` from Secrets, including
  ESO) reach vigild first. With the `false` default, services **no longer see
  them automatically**. That is what the allowlists are for, and the v4
  migration guide must say so prominently.

**Status:** done on the `v4` branch (`process_util/env.rs`, validation in
`overlord/plan.rs`, `PR_SET_DUMPABLE` in `main.rs`). The
`examples/kubernetes-pod-logs` layer now passes the variables
vigil-log-relay reads, including `KUBERNETES_SERVICE_*` for the in-cluster
API connection.

### S3 The reaper steals exit statuses from tokio

**Problem:** `crates/vigild/src/reaper.rs` calls `waitpid(-1)` on every
`SIGCHLD`. It can therefore collect vigild's own services and probes before
tokio does. tokio then gets `ECHILD`, and
`crates/vigild/src/service/actor/handlers.rs` turns that into
"failure, code 1". Phase 1 cannot be reliable without a fix.

**Original fix idea** (as gopherd does with `reaper.Registry`):

- The reaper becomes the only caller of `wait`.
- Spawn and registration happen under one lock
  (`Mutex<HashMap<Pid, oneshot::Sender<WaitStatus>>>`), otherwise a child can
  die before it is registered.
- Registered PIDs receive their status through the oneshot channel; unknown
  ones are discarded.
- The service actor waits on that channel instead of `Child::wait()`. Exec
  probes use the same path.

**Status:** largely fixed by PR #1 (daveman1010221), shipped in v3.2.2. The
reaper keeps collected statuses for 60 s and `reaper::wait_child` picks them
up on `ECHILD`. Exit codes are reliable now; phase 1 no longer depends on the
registry rework.

Open for v4:

- Ignore statuses recorded before the child was spawned. Otherwise, with PID
  reuse within 60 s, a stale orphan status could be returned.
- The registry rework is now only needed for signals: between the reaper
  collecting a child and the actor processing it, tokio still considers the
  PID valid, so `kill` could hit a reused PID.

### S4 Harden the control socket

- **Server** (`crates/vigild/src/server.rs`, `bind_unix`):
  - Check with `lstat` and refuse symlinks and non-socket files.
  - Bind under a short temporary name, `chmod`, then `rename` into place, so
    the socket never exists at its final path with looser permissions. The
    process-wide umask is not touched, because concurrently spawned services
    would inherit it.
  - If a stale node at the path cannot be removed, vigild aborts instead of
    only warning. The socket is bound before any service starts.
  - In v4 the socket lives under `--path-prefix` and is checked by its rules.
- **Client** (`vigil`, Unix transport): after connecting, check
  `SO_PEERCRED` and refuse a listener that is neither root nor our own euid
  (`--socket-uid` trusts one more UID). On OpenShift that is the same random
  UID, so it works.

**Status:** done in v3.2.2 (`bind_unix`, `--socket-mode` defaulting to 600,
`vigil --socket-uid`). Done on the `v4` branch: `--path-prefix` with the
directory check (`crates/vigild/src/path_prefix.rs`).

Found while smoke-testing and fixed on `v4`: the temporary socket name from
3.2.2 (`.vigild.sock.<pid>.tmp`) is longer than the final one, so a path close
to the 107-byte limit failed to bind. It is now `.<pid hex>.tmp`, with a clear
error for paths that are too long.

### S5 Without identities every caller is admin, including over TLS

**Problem:** `crates/vigild/src/api/auth/mod.rs` grants admin to every caller
as long as no identity is configured. This includes the TLS listener over
TCP: with `--tls-addr` and no identities, anyone who can reach the port
controls all services.

**Decision:** fix in v4 only, no backport.

**Fix (as implemented on `v4`):**

- Without identities, only the Unix socket grants admin, and only if the peer
  UID is root or vigild's euid (as in pebble). Other callers get `open`.
- TLS callers without a matching identity get `--tls-anonymous-access` /
  `VIGIL_TLS_ANONYMOUS_ACCESS`: `metrics` (default) or `open`. Logs,
  services and all state changes need an identity over TLS. (`read` as an
  anonymous level was rejected: logs may contain sensitive data.)
- `GET /v1/checks` is `open`, so Kubernetes `httpGet` probes keep working.
- Every TLS request carries a `TlsConnection` marker for the auth layer.
- At startup with `--tls-addr`, vigild logs the anonymous access level.
- Migration guide: anyone using the TLS API without identities today needs a
  `basic` or `tls` identity in v4.
- **vigil-log-relay:** `--source-url https://<vigild>/v1/logs/follow` needs
  a `read` identity now. The relay gained `--source-user-file` /
  `SOURCE_USER_FILE` (basic auth, re-read on every reconnect) and
  `--source-cert`/`SOURCE_CERT` + `--source-key`/`SOURCE_KEY` (mTLS).

**Status:** done on the `v4` branch, tested end to end (anonymous TLS
caller, relay with basic auth and with mTLS).

### Not affected

`O_CLOEXEC` and the pipe leak from the gopherd commit do not affect vigil-rs;
Rust `std` and tokio already handle them. The new features below must keep it
that way.

## Phase 1: real exit codes (needs S3)

- `EventKind::ProcessExited` gets `ExitInfo { code: Option<i32>, signal: Option<i32> }`.
- New daemon option `exit-code: fixed | propagate`. With `propagate`, vigild
  exits with the service's code, or `128+sig` for a signal.
  **Default in v4: `propagate`.** `fixed` (the old 0/10) stays for migration.
- Optional per service: `success-exit-codes: [0, 143]` and `exit-code-map: {143: 0}`.
- The `stop_all()` + `process::exit` paths in
  `crates/vigild/src/overlord/events.rs` and `crates/vigild/src/overlord/mod.rs`
  are merged into a single `shutdown(code)` function.
- Also: last exit code in the API and status, metric
  `vigil_service_last_exit_code`, `vigil services -o json`.

**Status:** done on the `v4` branch, together with the S3 rest (reaped
statuses older than the spawn are ignored). As implemented:

- `--exit-code propagate|fixed` / `VIGIL_EXIT_CODE`, default `propagate`.
- `shutdown_code()` in `service/mod.rs` is the single rule for vigild's exit
  code (service exits and `on-check-failure`); `Overlord::shutdown()` is the
  single place that stops everything and exits.
- `failure-shutdown` never exits 0 (falls back to 10); check failures have
  no process code and use 0/10.
- `exit-code-map` is applied before `success-exit-codes`; signals count as
  `128 + signal`.
- API `last-exit` (raw `code`/`signal`), CLI column `Last-Exit` and
  `-o json` / `VIGIL_OUTPUT`, metric `vigil_service_last_exit_code`.
- Docs fix: README and container-setup.md already claimed real exit-code
  propagation for v3, which only ever exited 0 or 10.

## Phase 2: readiness and sd_notify

- **Current state:** `Starting` only lasts a moment, because
  `crates/vigild/src/service/actor/spawn.rs` sets `Active` immediately.
  `after_deps_running` only waits for `is_running()`. `CheckLevel::Ready` is
  displayed but not used for anything.
- **New fields per service:** `ready: process | notify | check` (default
  `process`), `ready-check: <name>`, `ready-timeout: 30s`.
- **States:** `Starting` lasts until readiness is reached, then `Active`.
  Dependent services wait for `Active`. On timeout the `on-failure` policy
  applies.
- **sd_notify, without an extra crate:**
  - One `UnixDatagram` per service at
    `<path-prefix>/notify/<svc>-<64-bit random>.sock`, mode 0600, owned by the
    service UID (without user switching that is vigild's own UID, so no
    `chown`). A filesystem path instead of an abstract name avoids the
    squatting problem gopherd had.
  - The path goes into `NOTIFY_SOCKET`, also with `pass-env: false`.
  - Received via `recvmsg` with `SCM_CREDENTIALS`. Only messages from PIDs in
    the service's process group are accepted.
  - Handled: `READY=1`, `STATUS=` (shown in the API) and `STOPPING=1`.
    Watchdog comes later.
- **API (breaking, hence v4):** `ServiceStatus` gets `Starting`. Bump the
  OpenAPI version to v4. `vigil` v4 and `vigil-log-relay` must know the new
  value.

**Status:** done on the `v4` branch. As implemented (maintainer decisions
from 2026-11-03 marked *decided*):

- `ready`, `ready-check`, `ready-timeout` (`0s` = wait forever), validated at
  load and replan (`ready-check` must name an existing check that is not
  `startup: disabled`; `ready-check` without `ready: check` is an error).
- `after` / `requires` / `before` wait for `Active`; a dependency that is
  still `Starting` blocks.
- *Decided:* on `ready-timeout` the service is stopped (`stop-signal`, then
  `kill-delay`) and the exit always counts as a failure, so `on-failure`
  applies; `exit-code-map` cannot turn it into a success and a propagated
  shutdown exits 10.
- *Decided:* vigild creates `<path-prefix>` (if missing) and `notify/` with
  mode 0711 instead of 0700, so a service with `user:` can reach its socket.
  Sockets are 0600 and chowned to the service user; listing the directory
  is impossible.
- Notify socket per start (new random name, removed on exit); messages
  carry a generation number, stale ones are dropped. Sender check: main PID
  or its process group; a sender that already exited (`systemd-notify`) is
  accepted only with the service's UID. Truncated datagrams are dropped,
  passed file descriptors closed.
- `NOTIFY_SOCKET` inherited by vigild is never passed to services.
- `ready: check`: the check actor reports every pass with the probe's start
  time; only probes started after the spawn count. Alerts are unchanged
  (they still fire on status changes only).
- API: `starting`, `status-message`, `ExitInfo` schema; OpenAPI and utoipa
  version `4`. CLI shows `starting` and a `Message` column when set.
- `vigil-log-relay` does not use `ServiceStatus`, nothing to change there.

## Phase 3: oneshot and cron (builds on phase 2)

- `type: simple | oneshot`
  - A oneshot counts as "ready" on exit 0; dependent services start afterwards.
  - No restart after success. A failure goes through the normal `on-failure`
    policy with backoff.
  - New final state `Done` (also new in the API).
- `schedule: "*/5 * * * *"`, only for `oneshot`:
  - Crate `croner` with the `jiff` feature (maintainer decision), i.e. `jiff`
    instead of `chrono` for time zones and next-run computation. 5 or 6
    fields, correct DST handling. Check which croner version provides the
    feature before implementing.
  - An extra `sleep_until(next)` branch in the actor's `select!`.
  - Overlapping runs: default `skip` while the previous run is still active.
  - Time zone UTC, optional `schedule-tz`.
  - Hot reload recomputes the next run.
  - No state on disk (no "last run" file). Runs missed during a pod restart
    are not caught up.
- Metrics and alerts: result and time of the last run.

**Status:** done on the `v4` branch. As implemented (maintainer decisions
from 2026-11-03 marked *decided*):

- `type: simple | oneshot`, `schedule`, `schedule-tz`; croner 4.0.2 with
  `jiff` (no chrono feature), 5 or 6 fields (seconds first), no year field,
  Vixie OR semantics for day-of-month/day-of-week. Validated at load and
  replan.
- *Decided:* time zones only from the system tzdb (`jiff` with
  `tzdb-zoneinfo`, no bundled database). Alpine images need `tzdata`; an
  unknown zone rejects the layer.
- *Decided:* scheduled jobs default to `on-failure: ignore`; unscheduled
  oneshots keep `restart`.
- *Decided:* `startup: enabled` only arms the schedule, the job does not run
  at vigild start; `vigil start` runs it by hand.
- A oneshot is `starting` while it runs and `done` after success; `after` /
  `requires` / `before` accept `active` or `done`. `ready*` and
  `on-success: restart` are rejected for oneshots.
- Overlapping runs are skipped. The next run is recomputed from the wall
  clock on every actor loop iteration. DST verified: a nonexistent time runs
  once at the shifted time, a doubled time runs once.
- API `done` and `next-run`; CLI `Next-Run` column; metrics
  `vigil_service_last_run_timestamp_seconds`,
  `vigil_service_last_run_success`, `vigil_service_runs_total{result}`
  (counter, so failures between scrapes are not lost) and
  `vigil_service_running` (also `1` while a oneshot runs or a service waits
  for readiness, where `vigil_service_active` is `0`).
- Fix found on the way: `replan` never passed changed service configs to
  existing actors (the docs promised "takes effect on the next restart").
  Actors now get `UpdateConfig`; the new config applies from the next start,
  a changed schedule is re-armed immediately.
- Found in the smoke test: `replan` re-ran oneshots in `done` (autostart
  starts every enabled service that is not running). Autostart now skips
  `done` oneshots, so a migration is not repeated on every config change
  (important for phase 4c).

## Phase 4: templating

- **Syntax:** minijinja (Jinja2), the same as `body-template` of alerts, not
  gopherd's `{{.VAR}}` dialect (decided 2026-11-03: one template syntax in
  vigil, no own parser): `{{ env.VAR | default("x") }}`, `{{ file("/p") }}`,
  `{{ mem("90% - 64MiB") }}`, `{{ cpu("100% - 1") }}`.
- **Where:** `command` and the values of `environment` (services only, not
  exec checks).
- **When:** only a syntax check when the plan is loaded. Values are resolved
  at spawn time, so `{{ file }}` is read fresh on every (re)start of a service.
- **Undefined variables are errors** (decided): the start fails;
  `default(...)` or `is defined` handle optional ones.
- **Single pass:** minijinja renders once; substituted values are never
  evaluated again (gopherd's template bug cannot happen).
- **`command`:** split into arguments first (template tags are kept intact),
  then each argument is rendered on its own, so a value is always exactly one
  argument; an argument that renders empty is dropped (decided 2026-11-04,
  optional flags via `{% if %}`). Only values containing `{{` or `{%` are rendered, so shell
  `${#VAR}` (Jinja comment syntax) keeps working.
- **`env` sources:** in `environment` values the variables vigild passes
  (base set + pass-env); in `command` the child's final environment.
- **`file`:** absolute paths, regular files only, at most 64 KiB, UTF-8
  without NUL, one trailing newline is stripped, optional default for a
  missing file. An empty file (or only a newline) is an error too, or gives
  the default (decided 2026-11-04: an empty secret is almost always a
  mistake); an empty `env-file` stays valid (optional override files). Resolved values never appear in logs or the API.
  - Symlinks must be allowed: Kubernetes secret volumes consist of symlinks
    (`key -> ..data/key`, `..data -> ..2026_11_02_...`). No `O_NOFOLLOW` on
    the path, but `fstat` after opening to make sure it is a regular file.
- **`mem` / `cpu`:** smallest cgroup v2 `memory.max` of the own cgroup and
  its ancestors (v1 `memory.limit_in_bytes`), capped by `MemTotal`, as MiB.
  CPUs: `sched_getaffinity`, capped by the cgroup quota rounded up
  (`cpu.max` / `cpu.cfs_quota_us`), at least 1. On OpenShift these reflect
  the pod limits; without limits the node is used and a warning logged.

- **`env-file`** (added 2026-11-03 at the user's request: Vault Agent and
  ESO templates usually write one file with `KEY=value` lines, not one file
  per secret): list of files read on every start, between pass-env and
  `environment`; `export `, quotes and comments allowed; leading `-` =
  optional; at most 1 MiB. Visible to templates.
- **Exec checks:** a check with `service-context` used to copy the service's
  `environment` unrendered; now the service's env-file and environment and
  the check's environment go through the same builder (`template::child_env`)
  on every run. The check's `command` (`sh -c`) is deliberately not a
  template.

**Status (2026-11-03):** implemented in `crates/vigild/src/template/`
(minijinja with `UndefinedBehavior::Strict`, functions `file`, `mem`, `cpu`),
template-aware `parse_command_templated`, syntax check in plan validation.
Tested with unit tests (fake cgroup trees for v1/v2/nested/unlimited, K8s
symlink secret layout), actor tests (real process, undefined variable,
values not leaked into errors) and a smoke test (secret rotation via
`..data` swap picked up on restart). Fix found on the way: API errors and
change errors only showed the outermost context (`service "app"`), now the
whole chain.

### Phase 4b: watching files (replacement for stakater Reloader)

**Situation with ESO:** ESO updates the Kubernetes Secret. Then:

- **Secret as a volume (without `subPath`):** the kubelet updates the file in
  the pod after about 1-2 minutes (sync period plus cache).
- **Secret as an env variable (`env`/`envFrom`):** never updated except by a
  pod restart. Reloader is still needed for that.
- **Volume with `subPath`:** never updated.

**Decided (2026-11-03/04):**

- **Opt-in** (user, 2026-11-04: "vigild must be able to run without file
  access once it runs"): only with a `watch-files` block. Then by default the
  files read at the last start (`{{ file() }}` and `env-file`, recorded with
  a hash of the content actually used) are watched; a change restarts the
  service.
- ```yaml
  watch-files:
    on-change: restart   # restart | signal (= SIGHUP) | signal:SIGUSR1 | ignore
    interval: 30s
    paths: [/etc/app.conf]   # if set: ONLY these paths, the read files are not watched
  ```
- **Detection by content hash** (polling, not inotify): the kubelet swaps
  the `..data` symlink atomically, inotify on the file does not reliably
  notice; polling also works on NFS and overlayfs. Missing/created files
  count as changes, a transient read error does not.
- **Debounce:** react only once the content has been stable for one more
  polling round.
- Only `starting`/`active` services react; oneshot jobs are not watched (a
  `watch-files` block there is rejected); exec checks re-render anyway.
- Metric `vigil_service_file_reloads_total{service}`, log line with paths.
- **Hanging file systems:** all file reads (templates/env files at start,
  watched files, exec check environments) run on the blocking pool with a
  10 s timeout (`template::off_runtime`). A watch with a read still hanging
  starts no new one (at most one stuck thread per service). Found on the
  way: since phase 4 template rendering read files synchronously inside the
  async actor, so a dead NFS mount could block runtime workers and with them
  all of vigild (API, reaping).
- **No alert event** for a file-change reload (decided by the user; the
  generic alerts of phase 4b2 leave it out on purpose).

**Status (2026-11-04):** implemented (`service/watch.rs`, actor
`watch.rs`, `Renderer::take_reads`). Tests: debounce, revert within a round,
kubelet symlink swap, explicit paths, real process restart and SIGHUP via the
actor loop, validation. Smoke test with vigild: `..data` swap and a rewritten
env file each caused one restart with the new value, `touch` none, nginx-style
`signal:SIGUSR1` reload without restart. Found on the way (not fixed yet):
`merge_service` always copies `startup` from the overlay, so a layer that
does not set `startup` resets it to `disabled`.

### Phase 4b2: alerts from every component (before 4c)

**Goal:** alerts were tied to checks (`on-check`, payload always
`check`/`status`). Services and vigild itself could not report anything,
which 4c needs for invalid layers. Now every component sends a generic
alert event; delivery (queue, retry, proxy, TLS) stays as it is.

- **Event:** `source` (`check` | `service` | `vigild`), `name`, `event`,
  `firing` (problem or resolved), `message`. Components report through
  the channels they already have to the overlord, which owns the
  `AlertSender`.
- **Deduplication per `(source, name)`:** a firing event is sent when the
  event differs from the last one sent (a crash loop gives one `backoff`,
  then one `error` when vigild gives up); a resolved event only after a
  firing one was sent.
- **Subscription (additive, existing configs stay valid):** `on-check`
  as before, new `on-service` and `on-vigild`; `"*"` means all names.
  ```yaml
  alerts:
    ops:
      url: https://hooks.example.com/vigil
      on-check: [db-alive]
      on-service: ["*"]
      on-vigild: [layers]
  ```
- **Service events, one per failure, named after the consequence:**
  `backoff` (vigild restarts it), `error` (vigild gives up, or the start
  failed), `failed` (anything else: oneshot or cron run failed,
  `on-failure: ignore`, shutdown policy). The message carries the detail,
  e.g. `exited with code 3, restarting in 2s` or `did not become ready
  within 30s`. Stops requested by the user or by vigild are no failure.
- **Resolved for services:** once the service has been `active` without
  interruption for `healthy-time` (new service field, default `10s`,
  allowed `1s` to `1h`), or right after a successful oneshot or cron run.
  A crash within `healthy-time` is the same problem, no new event.
- **Checks of an unhealthy service stay quiet:** a check belongs to a
  service when the service names it in `ready-check` or
  `on-check-failure`, or the check names the service in its new field
  `service` (for checks without such a reference). `service-context` does
  not count: it only lends the environment, the check may test something
  else.
  While such a service is not healthy (starting, within `healthy-time`,
  backoff, error, stopped), the check's status is recorded but no alert is
  sent: the service alert already says what is wrong. Once the service is
  healthy again, a check that is still down is reported then; one that
  recovered meanwhile is not reported at all.
- **vigild events:** `layers` with `invalid` (4c); the next valid reload
  resolves it. File-change reloads (4b) stay without an alert.
- **Formats:** webhook gets `source`, `name`, `event`, `message` in
  addition to `status` (`down`/`up`); check events keep `check`.
  Check events are named `failed`/`recovered`, every resolved event is
  `recovered`. Alertmanager: `alertname` is the name, new label `source`;
  `event` and `message` are annotations, because Alertmanager identifies an
  alert by its labels and a resolve must carry the labels of the problem it
  ends. CloudEvents: type `io.vigil.<source>.<event>` (check events keep
  `io.vigil.check.failed`/`recovered`). OTLP: new attributes, body is the
  message. `body-template` gets the new fields as variables (`check` is
  undefined for other sources).
- The metric `vigil_alert_fire_count{alert}` counts events of all sources.
- API `/v1/alerts` and `vigil alerts list` show `on-service`/`on-vigild`
  (CLI column `On`: `web, service:*, vigild:layers`).

**Status (2026-11-04):** implemented (`alert/event.rs`, actor `health.rs`,
overlord `healthy` set). Tests: deduplication per event, independent
sources, held checks and recheck, all four formats, actor health events
(backoff with exit code, healthy after healthy-time, oneshot, failed start,
ready timeout), overlord hold-back, validation. VTest `v0023`: a service
failing once sends exactly `backoff` and `recovered`, its failing check
stays quiet (without `service:` the check alerts, verified).

### Phase 4c: layers from a ConfigMap, automatic `replan`

**Goal:** instead of a vigil operator (deliberately rejected: it would
duplicate Kubernetes and needs a CRD, controller, RBAC and API access), cover
the common case: mount the layers as a ConfigMap and let vigild pick up
changes itself. This removes the need for `kubectl exec … vigil replan` after
updating the ConfigMap.

- **New vigild option:** `--watch-layers <interval>` / `VIGIL_WATCH_LAYERS`
  (e.g. `30s`, off by default; allowed `1s` to `24h`).
- **Detection by polling** as in phase 4b, sharing the code: a hash over names
  and contents of all `*.yaml` files in the layers directory. Same reason as
  there: the kubelet swaps the ConfigMap's `..data` symlink atomically. Files
  starting with `..` (`..data`, `..2026_…`) are ignored.
- **Debounce:** only react once the hash has been stable for one polling round.
- **Validate before replanning:** parse and check the new layers first. On
  error the running plan stays active, an error is logged and the alert event
  `vigild`/`layers`/`invalid` is sent (phase 4b2); no retry until the content
  changes again.
- **Replan** reuses the existing `vigil replan` logic, i.e. only changed
  services and checks are restarted.
- Metrics: `vigil_layers_reload_total{result="ok|invalid"}` and the time of
  the last successful reload.
- Docs in `orchestrators.md`: ConfigMap example without `subPath` (with
  `subPath` the kubelet never updates the files).
- A manual `vigil replan` with invalid layers sends the same alert (decided
  by the user): both go through `reload_layers`.

**Status (2026-11-06):** implemented (`overlord/watch_layers.rs`; alert,
metrics and `off_runtime` in `Overlord::reload_layers`). The watcher sends
the normal replan command, so manual and automatic replans behave the same.
Found on the way: `load_plan` read the layer files synchronously inside the
overlord, so a hanging volume blocked vigild's control loop; it now runs
off the runtime with the 10s timeout. Metric names as registered:
`vigil_layers_reloads_total{result}` and
`vigil_layers_last_reload_timestamp_seconds`. Tests: debounce, hash of
names and contents without the kubelet's `..` entries, missing directory,
invalid layers keep the plan and alert once, recovery. VTest `v0024`: a new
layer is applied without replan, a broken one sends `invalid` and keeps the
running service, removing it sends `recovered`.

## Releases

```
S1, S4 (backport, without path-prefix) ────────► v3.2.2

S1, S2, S4, S5, prefix ──┐
S3 rest, phase 1 (exit) ─┤
phase 2 (readiness) ─────┼──► v4.0
phase 3 (oneshot/cron) ──┤
phase 4 + 4b + 4b2 + 4c ┘    (phase 4 after S2)
```

Order of work: the 3.2.2 backport first, then on `v4`:
S1 error case → S4 + path-prefix → S5 → S2 → S3 rest → phase 1 → phase 2 →
phase 3 → phase 4 → 4b → 4b2 → 4c.

For every phase: unit tests, VTest2 integration tests following the existing
patterns, a test under OpenShift conditions (random UID, GID 0, `/run` not
writable, `readOnlyRootFilesystem` with an `emptyDir` on `/tmp`), updates to
`docs/user-guide/configuration.md`, `docs/operator-guide/orchestrators.md` and
`docs/specs/openapi.yaml`, coverage not below 92 %, plus a v3 → v4 migration
guide. Breaking commits carry a `BREAKING CHANGE:` footer with migration
steps; it feeds the changelog's "Breaking Changes" section.

## Decided

- Target version v4.0; **all phases ship in v4.0**.
- S1 and S4 additionally backported to v3.2.2 (released).
- `pass-env` defaults to `false`; allowlists via the separate fields
  `pass-env-glob` and `pass-env-regex`.
- Exit codes: `propagate` is the default in v4.
- A single `--path-prefix` directory for all runtime files, checked at
  startup, abort on failure. vigild creates the last component (mode 0711)
  if it is missing.
- Everything must run on OpenShift without write access to `/run` and
  without capabilities.
- The new `Starting` status and OpenAPI changes ship in v4.
- No vigil operator; ConfigMap-based layers with auto-replan instead (4c).
- S5: anonymous TLS access is `metrics` by default (or `open`), never
  `read`; `/v1/checks` is `open`; vigil-log-relay authenticates via basic
  auth file or mTLS. Every new option gets an environment variable, like all
  options of vigild, vigil and vigil-log-relay.
- Proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`) are
  not part of the default base set: proxy URLs may carry credentials. Pass
  them per service, e.g. `pass-env-regex: ['(?i)(https?|all|no)_proxy']`.

## Open decisions

None.
