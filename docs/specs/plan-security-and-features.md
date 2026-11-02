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
- **Subdirectories** such as `notify/` are created by vigild with mode 0700.
- **Creating the last component:** if the last path component is missing and
  its parent exists, vigild creates it with mode 0700 and then checks it as
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

**Fix:**

- Without identities, only the Unix socket grants admin, and only if the peer
  UID is root or vigild's euid (as in pebble).
- Over TLS without identities, only endpoints that need no auth (`Open`) are
  accessible, e.g. health and metrics.
- When started with `--tls-addr` but no identities, vigild warns that the TLS
  API is effectively read-only.
- Migration guide: anyone using the TLS API without identities today needs a
  `basic` or `tls` identity in v4.

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

## Phase 4: templating

- **Syntax:** `{{.VAR}}`, `{{.VAR:-default}}`, `{{file "/p"}}`, `{{mem 75%}}`, `{{cpu 50%}}`.
- **Where:** `command` and the values of `environment`.
- **When:** only a syntax check when the plan is loaded. Values are resolved
  at spawn time, so `{{file}}` is read fresh on every (re)start of a service.
- **Parser:** new module `crates/vigild/src/template/`, hand-written, no regex
  or template engine. Everything is resolved in a single pass over the
  original text (that was gopherd's template bug). Fuzzing with proptest.
- **`file`:** regular files only, at most 64 KiB, a trailing newline is
  stripped. Resolved values never appear in logs or the plan API.
  - Symlinks must be allowed: Kubernetes secret volumes consist of symlinks
    (`key -> ..data/key`, `..data -> ..2026_11_02_...`). No `O_NOFOLLOW` on
    the path, but `fstat` after opening to make sure it is a regular file.
- **`mem` / `cpu`:** cgroup v2 `memory.max` / `cpu.max`, otherwise
  `/proc/meminfo` / `sched_getaffinity`. On OpenShift these reflect the pod
  limits.
- **`.VAR`:** sources are the service's `environment` plus vigild variables
  allowed through `pass-env`.

### Phase 4b: watching files (replacement for stakater Reloader)

**Situation with ESO:** ESO updates the Kubernetes Secret. Then:

- **Secret as a volume (without `subPath`):** the kubelet updates the file in
  the pod after about 1-2 minutes (sync period plus cache). A `{{file}}` reads
  the new value on the service's next start.
- **Secret as an env variable (`env`/`envFrom`):** never updated except by a
  pod restart. Reloader is still needed for that.
- **Volume with `subPath`:** never updated.

Phase 4 alone only picks up the new value when the service restarts for some
other reason. Replacing Reloader needs a trigger:

- **New field per service:**
  ```yaml
  watch-files:
    paths: [/etc/secrets/db-password]   # in addition to all {{file}} paths
    on-change: restart                  # restart | signal:SIGHUP | ignore
    interval: 30s
  ```
- **Detection by polling** (hash or inode and mtime every `interval`), not
  inotify. Reason: the kubelet swaps the `..data` symlink atomically, and
  inotify on the file does not reliably notice. Polling is simple and also
  works on NFS and overlayfs.
- **Debounce:** ESO and the kubelet update several keys one after another.
  Only react once the content has been stable for one polling round.
- **Advantage over Reloader:** only the affected service is restarted, not the
  whole pod; with `signal:SIGHUP` nothing is restarted if the application can
  reload.
- Each change emits an alert event and increments the metric
  `vigil_service_file_reloads_total`.

### Phase 4c: layers from a ConfigMap, automatic `replan`

**Goal:** instead of a vigil operator (deliberately rejected: it would
duplicate Kubernetes and needs a CRD, controller, RBAC and API access), cover
the common case: mount the layers as a ConfigMap and let vigild pick up
changes itself. This removes the need for `kubectl exec … vigil replan` after
updating the ConfigMap.

- **New vigild option:** `--watch-layers <interval>` / `VIGIL_WATCH_LAYERS`
  (e.g. `30s`, off by default).
- **Detection by polling** as in phase 4b, sharing the code: a hash over names
  and contents of all `*.yaml` files in the layers directory. Same reason as
  there: the kubelet swaps the ConfigMap's `..data` symlink atomically. Files
  starting with `..` (`..data`, `..2026_…`) are ignored.
- **Debounce:** only react once the hash has been stable for one polling round.
- **Validate before replanning:** parse and check the new layers first. On
  error the running plan stays active, an error is logged and an alert event
  (`layers-invalid`) is emitted; no retry until the content changes again.
- **Replan** reuses the existing `vigil replan` logic, i.e. only changed
  services and checks are restarted.
- Metrics: `vigil_layers_reload_total{result="ok|invalid"}` and the time of
  the last successful reload.
- Docs in `orchestrators.md`: ConfigMap example without `subPath` (with
  `subPath` the kubelet never updates the files).

## Releases

```
S1, S4 (backport, without path-prefix) ────────► v3.2.2

S1, S2, S4, S5, prefix ──┐
S3 rest, phase 1 (exit) ─┤
phase 2 (readiness) ─────┼──► v4.0
phase 3 (oneshot/cron) ──┤
phase 4 + 4b + 4c ───────┘    (phase 4 after S2)
```

Order of work: the 3.2.2 backport first, then on `v4`:
S1 error case → S4 + path-prefix → S5 → S2 → S3 rest → phase 1 → phase 2 →
phase 3 → phase 4 → 4b → 4c.

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
  startup, abort on failure. vigild creates the last component (mode 0700)
  if it is missing.
- Everything must run on OpenShift without write access to `/run` and
  without capabilities.
- The new `Starting` status and OpenAPI changes ship in v4.
- No vigil operator; ConfigMap-based layers with auto-replan instead (4c).

## Open decisions

None.
