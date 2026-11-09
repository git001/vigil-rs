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
  reuse within 60 s, a stale orphan status could be returned. **Done**
  (`reaper::wait_child(child, spawned_at)`).
- The registry rework is now only needed for signals: between the reaper
  collecting a child and the actor processing it, tokio still considers the
  PID valid, so `kill` could hit a reused PID. **After v4.0** (decided
  2026-11-09: the risk needs a full round of PIDs within milliseconds, the
  rework touches how every service and exec probe waits). Documented as a
  known limitation in `docs/operator-guide/container-setup.md`.

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

### Phase 4d: identities in layer files (before the release)

**Goal:** identities only existed in memory (added over the API) and were
lost on every restart; `identities:` in a layer file was silently ignored.
Decided by the user (2026-11-06): layer files may define identities, so they
survive a restart and can come from a ConfigMap (e.g. the read identity of
vigil-log-relay).

Decided by the user (2026-11-06):
- **Two sources side by side.** Identities from layer files are read-only
  over the API: `POST` or `DELETE` with the name of a layer identity is
  rejected ("identity 'log-relay' is defined in layer 001-auth.yaml").
  `vigil identities list` shows a `Source` column (`layer` / `api`).
- **Replan** (also `--watch-layers`) replaces the layer identities as a
  whole: one removed from the layers loses its access at once. Identities
  added over the API are not touched.
- **Several layers:** a later layer file replaces the whole identity; there
  is no `override:` for identities (mixing auth methods would be dangerous),
  so `override` there is an unknown key and an error.
- **Secrets from files:** `basic.password-hash-file` and `tls.ca` (see
  "Central file and TLS handling"),
  like `env-file` and `--source-user-file`. Setting both the inline value and
  the file is an error. Files are read when loading (off the runtime, with
  the file I/O timeout); a read error fails the load. `--watch-layers`
  includes their content in its hash, so a rotated Kubernetes Secret is
  picked up.
- **Bootstrap:** only identities added over the API end the Unix-socket
  bootstrap admin (root and vigild's UID). Layer identities alone, e.g. only
  a read identity for vigil-log-relay, keep it; the socket's default mode
  600 limits it to those two UIDs anyway. The docs say so explicitly.

Implemented on 2026-11-06:
- `vigil-types`: `Layer.identities` / `Plan.identities` (`IdentityConfig`,
  `deny_unknown_fields`), `Identity.source` (`api` | `layer`) and
  `Identity.layer`.
- `vigild`: `plan::load()` checks the identities and reads their files
  (`identity::load_layer_identities`); the overlord owns the identity store
  and replaces the layer identities on every load (`set_layer`); `POST` /
  `DELETE` on a layer identity answer `409`; the bootstrap checks
  `api_is_empty()`; `--watch-layers` hashes the files the layers refer to
  too (`WatchedFiles`: `tls:` files of checks, alerts and identities,
  `password-hash-file`).
- Identity `tls:` takes only `ca` (`IdentityTls`), read with
  `vigil_tls::PemFile::layer`; `local` is `LocalIdentity` with
  `deny_unknown_fields`; `password-hash-file` is read with `vigil-files`
  (absolute path, missing or empty is an error).
- `overlord::spawn` loads the layers with `off_runtime` at startup too.
- An API identity with the name of a layer identity is replaced by the
  layer one (with a warning).
- CLI: `vigil identities list` shows `Source` and every auth method
  (`basic` was shown as `-` before), sorted by name.
- Tests: unit tests, `api_handlers/layer_identities.rs`, vtest v0026
  (basic auth over TLS, read-only over the API, rotated secret, removal by
  replan).

Before 4d (same day): `env:VAR` in alerts (`url`, `headers`, `labels`,
`send-info-fields`, `proxy`, `no-proxy`) is resolved and checked at load
time; an unset variable is a hard error instead of an empty string at send
time. Alerts see vigild's environment under the same rules as services:
`pass-env`, `pass-env-glob`, `pass-env-regex` (default: only the base
variables), with separate errors for "not set" and "not passed". The proxy
variables (`HTTPS_PROXY`, …, `NO_PROXY`) are used only when the alert's
policy passes them; reqwest's own lookup of them is switched off.

### Central file and TLS handling (before 4d)

**Goal:** vigild, vigil and vigil-log-relay each had their own PEM loaders
and file reads, and the HTTP check, the alerts and (in 4d) the identities
each had their own TLS keys (`ca`, `tls-ca`, `proxy-ca`, `insecure`,
`tls-insecure`, `insecure-proxy`). Decided by the user (2026-11-06):

- **Two crates.** `vigil-files` reads every configured file with the same
  rules (absolute paths in layer files, regular files only, a size limit,
  errors without the content) and, with the feature `tokio`, off the async
  runtime with the 10s timeout (dead NFS mounts). `vigil-tls` loads CA
  bundles, certificates and keys and applies them to HTTP clients. Command
  line options keep their names.
- **One layer schema.** HTTP checks and alerts take a `tls:` block (`ca`:
  one path or a list, `cert`, `key`, `insecure`) and a `proxy:` block
  (`url`, `no-proxy`). Identities (4d) take `tls:` with `ca` only; any other
  key there is an error, as `insecure` would bypass authentication.
- **Proxy TLS comes from `tls:`.** reqwest has no separate TLS settings for
  the proxy, so a proxy's own CA goes into `tls.ca`; `insecure-proxy` is
  removed (it also switched off the check of the server).
- **HTTP checks without `proxy:` connect directly**; vigild's proxy
  variables are not used. Alerts keep using the variables their `pass-env*`
  rules pass when they have no `proxy:` block.
- **Clients are built when loading.** The client of an HTTP check is built
  with the plan (off the runtime on a replan), like the alert clients; an
  unreadable `tls.ca` makes the layer invalid instead of failing every probe.
  Every replan hands the new client to running check actors without
  restarting them (status and failure count stay), so a rotated CA or
  client certificate takes effect with the next replan.
- `no-proxy` everywhere uses the matcher of `vigil-types`, so vigild's
  alerts and checks accept CIDR ranges like the CLI and the relay.

## Phase 5: debugging and runtime control (after the Pebble/gopherd comparison)

Decided by the user on 2026-11-07 after comparing vigil-rs v4 with Pebble
v1.33 and gopherd v1.3.5. Goal: find out quickly why a check fails, give
Kubernetes a real health endpoint, control single services more finely, and
make replan do what phase 4c assumed. Steps in this order; each is one
commit (or a small series) with unit tests, a vtest where it runs end to
end, docs, OpenAPI, and an entry here. All of phase 5 is part of v4.0,
before the release; the user confirmed 5b (a refresh counts like a
scheduled run), 5c (502 when unhealthy) and 5f (requires-dependents are
restarted too).

### 5a: why a check fails

Today a failed probe logs `check failed failures=1 threshold=3` at `warn`;
the reason (HTTP status, connection error, exit code) is only at `debug`,
and an exec check's output is dropped.

- Probes return the reason, not just pass/fail: `Err("http 503")`,
  `Err("connect 127.0.0.1:5432: connection refused")`, `Err("timed out after
  3s")`, `Err("exit status 2")`.
- The `check failed` warning carries the reason; so does the `check is down`
  line and the alert (`message`).
- Exec checks keep the last lines of stdout/stderr (bounded, e.g. 4 KiB) and
  add them to the reason. Nothing is logged on success.
- `GET /v1/checks` gets `last-error` (the reason of the last failed run,
  cleared by a pass); `vigil checks list` shows it.

Done: probes return `Result<(), String>`. HTTP reasons are `HTTP <status>`
or `GET <url>: <innermost cause>` (reqwest's "error sending request" wrapper
dropped), TCP `connect <addr>: <error>`, exec `exit status N` / `killed by
signal N` / `timed out after …` plus the output tail. stdout and stderr
share one pipe; output still in the pipe is read for at most 200 ms after
the exit, so a background process holding it open cannot stall the check.
The log quotes the reason (`error="…"`), so multi-line output stays on one
log line. A held-back down check keeps its reason for the recheck alert.
vtest v0027.

### 5b: run a check now

- `POST /v1/checks/refresh {"name": "db-ready"}` (Pebble's path and body):
  runs the check once at once and answers with the check info and `error`
  (the reason, absent on success). Access: `write`.
- The run counts like a scheduled one (failures, threshold, status, alerts),
  as in Pebble. A check with `startup: disabled` is run once without
  changing any state.
- CLI: `vigil checks run NAME` prints the result and the reason; exit code 0
  on pass, 1 on failure, so it fits scripts.

Done: response `{"info": CheckInfo, "error": "…"}` (Pebble's shape). The
overlord only hands the request to the check actor (`check::Cmd::Refresh`)
and never waits for the probe; the actor runs it between scheduled runs,
also during the initial `delay`. A disabled check runs once in a task of
its own (`check::run_once_detached`). Unknown check 404, unknown body
fields rejected. The docs' CLI examples used commands that do not exist
(`vigil services`, `vigil start NAME`); fixed to `services list`,
`services start NAME` etc.

### 5c: `GET /v1/health`

- Pebble's semantics: `?level=alive|ready` and `?names=a,b`. `ready`
  includes `alive` checks. A check counts when it matches and is running;
  disabled checks are ignored. 200 `{"healthy": true}` if none is down,
  otherwise 502 `{"healthy": false}` (502 as in Pebble).
- Access: `open`, like `/v1/checks`, so Kubernetes probes over TLS work
  without credentials.
- The check field `level` gets a meaning through this; the docs say so.
- The kubernetes-pod-logs Deployment switches its probes to
  `/v1/health?level=alive` and `?level=ready`.

Done. Deviations from Pebble, both so that a broken probe cannot pass
unnoticed: a name in `names` that is not a check is 404 (Pebble ignores
it), and an unknown query parameter or level is 400. A check during its
initial `delay` counts as up. The kubernetes-pod-logs probes comment that
vigild's own `on-check-failure: restart` acts first; orchestrators.md lost
the `insecureSkipTLSVerify` probe field (not a Kubernetes field) and
explains how to size the probe against a check's period x threshold.
Added on request: a load warning for a running `alive` check (set or by
default) whose http/tcp probe targets another host than this container
(not localhost, *.localhost, loopback or unspecified); exec checks are not
inspected. The three examples that watch https://example.com/ use
`level: ready`. Flow chart `docs/user-guide/check-levels.dot` →
`images/check-levels.svg` (which level, what /v1/health counts, what the
probes do), linked from configuration.md (`level`) and orchestrators.md.

### 5d: send a signal to single services

- `POST /v1/signals {"signal": "SIGHUP", "services": ["nginx"]}` (Pebble's
  path and body). The signal goes to the service's process group, like
  forwarding does; a service that is not running is an error for that
  service. Access: `write`.
- CLI: `vigil services signal SIGHUP nginx [more…]`.
- Forwarding SIGHUP/SIGUSR1/SIGUSR2 to all services stays as it is.

Done. All or nothing: an empty list is 400, an unknown service 404, a
service without a process (anything but starting, active, stopping) 409,
and then no service gets the signal. Signal names only as in
`stop-signal` (`SIGHUP`; `HUP` is 400). The overlord checks and then sends
from a task of its own; the service actor answers whether it had a
process (`service::Cmd::Signal`). Result `{"signal", "services"}`.
CLI: `vigil services signal SIGHUP nginx`. vtest v0028.

### 5e: `follow` on `/v1/logs`

- `GET /v1/logs?follow=true` behaves like `/v1/logs/follow?format=ndjson`
  (with `services` and `n`). Unknown query parameters on `/v1/logs` become
  a 400 instead of being ignored.
- `/v1/logs` without `follow` keeps its JSON envelope (the vigil CLI uses
  it); `/v1/logs/follow` stays.

Done. `follow=true` subscribes first, then reads the last `n` lines (default
100, `n=0` for none) and streams both as NDJSON; a live line that is already
in that tail is not sent twice. Unknown parameters and a `follow` that is not
`true`/`false` are 400 with a JSON error, checked after the access check.
`/v1/logs/follow` no longer lists `n`, which it never used. vtest v0029.

### 5f: replan restarts changed services

Phase 3 made a changed service config apply from the next start
(`UpdateConfig`); phase 4c then assumed that replan restarts changed
services. Pebble restarts them. With `--watch-layers` a changed ConfigMap
should take effect without a crash or a manual restart.

- On replan, a running service (`starting`, `active`, `backoff`) whose
  `ServiceConfig` changed is restarted: stopped, then started with the new
  config through its readiness. Services that depend on it via `requires`
  are restarted after it, in start order.
- Not restarted: stopped services (they get the config for their next
  start, as today), oneshots in `done`, a scheduled job between runs (its
  schedule is re-armed, as today), and a running scheduled run (applies from
  the next run).
- Changes that do not affect the process (`summary`, `description`) do not
  restart it.
- The `plan reloaded` log line lists the restarted services.
- Breaking for anyone who relied on "applies at the next start":
  `fix(vigild)!` with a migration note.

Done. The overlord picks the services before it applies the new plan
(`replan_restarts`: changed beyond `summary`/`description`, state
`starting`/`active`/`backoff`, not scheduled, plus running
`requires`-dependents, transitively), sends `Stop` and keeps them in
`pending_restart`; when one reports `inactive` it goes through
`start_or_defer`, so a dependent starts once the service it requires is
ready. The overlord never waits for a stop. A `stop` of the service cancels
its pending restart. New module `overlord/restart.rs`; vtest v0030 follows
the log with `/v1/logs?follow=true` and checks that db starts before api.

### 5g: diagram of exit, restart and backoff

`docs/user-guide/service-lifecycle.dot`, rendered by the Diagrams workflow
to `images/service-lifecycle.svg` and linked from configuration.md: process
exit → `exit-code-map` → `success-exit-codes` → `on-success` / `on-failure`
(`restart`, `shutdown`, `success-shutdown`, `failure-shutdown`, `ignore`)
→ backoff (`backoff-delay`, `-factor`, `-limit` → `error`) → `healthy-time`
resets it; plus `on-check-failure`, `ready-timeout` and `kill-delay`. Last,
so it shows the behaviour after 5f.

Done. Four clusters in one column (start, vigild stops it, the process
exits by itself, policy), loops as text ("start again (1.)") instead of
long edges; linked from the `on-success` / `on-failure` section. Found on
the way: configuration.md said `healthy-time` only affects alerts, but it
also starts a new backoff sequence; fixed.

### 5h: check metrics

Added by the user on 2026-11-07 after 5c. Today there are only
`vigil_check_up{check}`, `vigil_check_success_count{check}` and
`vigil_check_failure_count{check}`, documented only under `GET /v1/metrics`
in rest-api.md. Missing: which level is down (a dependency or the own app),
how often and how long a check was down, and how long a probe takes.

- **`level` label** on every check metric (`{check, level}`), so that
  `min by (level) (vigil_check_up)` separates dependencies (`ready`) from
  the own app (`alive`). Decided: a label, not a separate info metric.
  Changes the label set of the three existing metrics: `feat(vigild)!`
  with a migration note for recording rules and joins.
- **`vigil_health{level="alive|ready"}`** gauge 0/1: what
  `GET /v1/health?level=…` would answer, to alert on directly.
- **Outages:** `vigil_check_down_count{check, level}` counter (transitions
  up → down: "how often") and `vigil_check_down_seconds{check, level}`
  counter (time spent down: availability = `1 - rate(…[30d])`). The
  existing failure counter counts single failed runs, not outages.
- **Rough timing** ("start, connect, end"), histograms with
  `{check, level, result="success|error"}`:
  - `vigil_check_duration_seconds`: start to end of the probe. On a
    timeout the run ends at the check's `timeout`, so that value is
    recorded with `result="error"`.
  - `vigil_check_connect_duration_seconds`: start until the connection is
    up. tcp: the connect; http: TCP connect plus TLS handshake, measured
    with reqwest's `connector_layer`; exec: none. HTTP check clients stop
    keeping idle connections (`pool_max_idle_per_host(0)`), so every run
    connects and is measured; a health check should test a fresh connect
    anyway.
  - Buckets 1 ms to 60 s (0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5,
    1, 2.5, 5, 10, 30, 60).
- **TLS certificate:** `vigil_check_tls_cert_expiry_timestamp_seconds{check,
  level}` gauge, the `notAfter` of the server certificate of an https
  check (reqwest `tls_info`), to alert before it expires.
- **Docs:** an operator-guide page "Metrics" with every metric and PromQL
  examples (dependency down, own app down, availability over 30 days, p95
  duration, certificate expires within 14 days); rest-api.md links to it.
- Later, only if needed: separate HTTP phases (DNS, TCP connect, TLS,
  time to first byte).

Done, part 1 (level label, `vigil_health`, outages, Metrics page): the
store knows the running checks and their level (`set_checks` on every
plan), drops the series of removed, disabled or re-levelled checks and
ignores late writes for them. `vigil_check_up` is set when the actor
starts (before its delay), not only after it. `vigil_check_down_seconds`
grows at each run while down and on recovery (lags by up to one period).
`vigil_health` is computed by the overlord from the last observed status
of every running check, on every check event and every plan. vtest v0031.

Done, part 2 (histograms): `CheckClient` (check/probe/connect_timer.rs)
wraps the reqwest client of an HTTP check with a tower layer around the
connector that stores the time of the last connect;
`pool_max_idle_per_host(0)` makes every run connect. tcp measures the
successful connect itself. The actor records the run duration capped at the
timeout. The TLS certificate expiry metric was dropped (user decision
2026-11-07): it needs an X.509 parser, and none is in the workspace.

### Not taken from the comparison

Pebble's exec and files API (Juju-specific, large attack surface) and
notices (alerts cover them); `logs-push-format` stays a documented
placeholder.

## Phase 6: refactor before v4.0

Decided by the user on 2026-11-07: before the v4.0 release, after phase 5.
Two parts, no behaviour change in either.

### 6a: source files of about 200 lines

No Rust source file longer than about 200 lines, test code not counted
(`tests.rs`, `tests/`, everything after `#[cfg(test)]`). On 2026-11-07
(`3896a57`) 25 files are longer; the largest:

| File | Lines | Split |
|---|---|---|
| `vigild/src/overlord/plan.rs` | 526 | `plan/mod.rs` (load, read, `Loaded`), `plan/validate_service.rs`, `plan/validate_check.rs` (with the level warning), `plan/validate_alert.rs`, `plan/dependencies.rs` (names, cycles, warnings) |
| `vigil-log-relay/src/cli.rs` | 515 | option groups as `#[command(flatten)]` structs (source, sink, Kubernetes, TLS, proxy); flags, environment variables and `--help` unchanged |
| `vigild/src/overlord/lifecycle.rs` | 431 | `actors.rs` (`sync_actors`), `autostart.rs` (autostart, dependencies, pending), `reload.rs` (`reload_layers`, `apply_plan`) |
| `vigild/src/main.rs` | 394 | `args.rs` (flags, enums, parsers), the start-up steps of `async_main` in their own functions |
| `vigild/src/overlord/handlers.rs` | 388 | by command: services, checks (query, health, refresh), signals, alerts |
| `vigil-types/src/plan/service.rs` | 337 | the enums and `WatchFiles` apart from `ServiceConfig`, `merge_service` apart |
| `vigil-types/src/api.rs` | 337 | `api/` with one file per area (system, services, changes, checks, alerts, logs, notices), re-exported: paths stay `vigil_types::api::X` |

The others (200–331 lines) are split where the file has natural parts;
a file of 200–220 lines without one stays.

Rules:

- Before splitting a file, check what can be simplified or merged: code
  that two places (or two crates) do each in their own way gets one
  implementation, in a shared module or a workspace crate, as file and PEM
  loading went into `vigil-files` and `vigil-tls` (`fe6be4f`). Candidates
  come up per area; each is named in the diff review.
- No new behaviour. Where merging two implementations aligns a difference
  between them (one of them was wrong or stricter), the commit message
  lists it under "Behaviour changes", as `fe6be4f` did.
- Public paths of `vigil-types` stay (re-exports); no new external
  dependencies.
- Same output before and after: `vigild --help`, `vigil --help` (and its
  subcommands), `vigil-log-relay --help`, and `GET /openapi.json` are
  compared byte for byte.
- One commit per area (`refactor(vigild): …`), each after the user has
  seen the diff; `cargo fmt --check`, clippy, all tests and all vtests
  before each.
- New code from now on goes into small modules from the start.

### 6b: older vtests in native VTest2

v0001–v0026 move to the style of v0027–v0031 (tests/vtest/README.md,
"Native VTest2 first"): `--layers-dir ${vtestdir}/fixtures/vNNNN` instead
of mkdir/cp/replan, `process -expect-text` on vigild's log instead of shell
wait loops, one `client` per request instead of curl, `server` instead of
capture helpers, `filewrite` instead of touch, shell only for the vigil CLI,
symlinks, TLS with files and other things VTest2 cannot do.

- Every assertion of the old test stays (or is replaced by a stricter one);
  the commit message names any that cannot be kept, and why.
- About five tests per commit, each stable in 10 runs.

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
