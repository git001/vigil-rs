# vigild VTest2 Integration Tests

End-to-end tests for the vigild HTTP API and vigil CLI, written in the
[VTest2](https://code.vinyl-cache.org/vtest/VTest2) `.vtc` format.

Each test starts a real vigild process (and sometimes vigil-log-relay) with a
temporary layers directory and Unix socket, exercises the binary under test,
and verifies output.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| `cargo build -p vigild -p vigil -p vigil-log-relay` | Builds all three binaries in `target/debug/` |
| VTest2 binary | Expected at `/datadisk/git-repos/VTest2/vtest`; override with `VTEST=` |

## Running

All tests, from the repository root:

```sh
tests/vtest/run.sh
```

With coverage (unit tests + Rust integration tests + VTest2, combined report):

```sh
tests/vtest/run.sh --coverage          # text summary
tests/vtest/run.sh --coverage --html   # open HTML report
```

Single test (verbose):

```sh
tests/vtest/run.sh -v tests/vtest/v0001_system_info.vtc
```

Custom binary or VTest2 path:

```sh
VTEST=/path/to/vtest \
VIGILD_BIN=/path/to/vigild \
tests/vtest/run.sh
```

## Test files

| File | What it covers |
|------|----------------|
| `v0001_system_info.vtc` | `GET /v1/system-info` — 200, body contains version / boot-id / start-time |
| `v0002_api_lists.vtc` | `GET /v1/services`, `/v1/checks`, `/v1/alerts`, `/v1/logs`, `/v1/identities` — all 200, result field present |
| `v0003_metrics.vtc` | `GET /v1/metrics` — 200, Content-Type contains `openmetrics` |
| `v0004_openapi.vtc` | `GET /openapi.json` — 200, spec contains `"vigil API"`, `info`, `paths` |
| `v0005_replan.vtc` | `POST /v1/replan` before and after writing a layer — service appears in list |
| `v0006_identities.vtc` | Identity CRUD: add alice (bootstrap auth), list (Unix UID auth), remove |
| `v0007_daemon_stop.vtc` | `POST /v1/vigild {"action":"stop"}` — daemon exits cleanly |
| `v0008_logs_query.vtc` | `GET /v1/logs` with `?n=` and `?services=` query parameters |
| `v0009_vigil_cli.vtc` | `vigil` CLI: system-info, services list/start/restart/stop, checks list, alerts list, logs, replan, identities add-local/list/remove |
| `v0010_log_relay_socket.vtc` | `vigil-log-relay --source-socket` — streams ndjson lines from vigild Unix socket |
| `v0011_log_relay_url.vtc` | `vigil-log-relay --source-url` — streams from an HTTP endpoint |
| `v0012_vigil_alerts.vtc` | `vigil alerts list` — unknown / down / up display branches |
| `v0013_vigil_tls_and_reaper.vtc` | vigild TLS listener (`--tls-addr`), `vigil --insecure`, subreaper (`--reaper`) + SIGCHLD handling |
| `v0014_vigil_checks_logs_vigild.vtc` | `vigil checks list` (all four `next_run` formatting branches), `vigil logs` with entries, `vigil vigild status/stop` |
| `v0027_check_failure_reason.vtc` | Why a check fails: `last-error` in `GET /v1/checks` and `vigil checks list`, the reason in the `check failed` log line and in the alert message, cleared by a pass; `vigil checks run` (exit codes, reason, unknown check); `GET /v1/health` (200/502 by level and name, 404, 400) |

## How it works

VTest2 provides three key primitives used here:

- **`process`** — spawns a binary as a subprocess, capturing stdout/stderr; supports `-start`, `-kill`, `-wait`
- **`client -connect <path>`** — opens an HTTP/1.1 connection to a Unix socket
- **`shell`** — runs a POSIX sh snippet for setup, teardown, polling loops, or output assertions

The macros passed via `-D` hold paths:

| Macro | Path |
|-------|------|
| `${vigild}` | vigild daemon |
| `${vigil}` | vigil CLI |
| `${vigil_log_relay}` | vigil-log-relay |
| `${vtestdir}` | this directory (`tests/vtest`), for fixtures and helpers |

`${tmpdir}` is a per-test temporary directory provided automatically by VTest2.

### Fixtures and helpers

Tests are also examples of how to use vigil, so keep what they use readable
on its own:

- `fixtures/`: static input files, e.g. request bodies
  (`${vtestdir}/fixtures/identities/admin-user.json`); see
  [fixtures/README.md](fixtures/README.md) for the test users.
- `helpers/`: small standalone scripts, e.g. `helpers/http_capture.py` (an
  HTTP server that records POST bodies and reports its port in a file),
  `helpers/tcp_capture.py` (a TCP sink, the same for raw TCP; `--port` binds
  a given port again, e.g. to test a reconnect) or
  `helpers/tls_identity.py` (request body for a TLS identity whose CA the
  test creates at runtime).

Prefer a static file over building configs, bodies or scripts inside a
`.vtc` (heredocs, inline JSON, `python3 -c`).

## Patterns and gotchas

### Native VTest2 first

Use VTest2's own commands wherever they fit; `shell` only for what they
cannot do. v0027 and v0028 show all of these:

| Need | Native command |
|---|---|
| A layer directory | `--layers-dir ${vtestdir}/fixtures/vNNNN` (the static fixture itself; no `mkdir`, no `cp`, no replan) |
| Wait until vigild listens, a check is down, a service is ready | `process p1 -winsz 200 400 "… ${vigild} …" -start`, then `process p1 -expect-text 0 0 "API listening"` (vigild logs to stdout, the terminal of `p1`; `-expect-text` waits) |
| Check a log line | `process p1 -match-text 0 0 {check failed .*error="…"}` (a regex; does not wait, so wait with `-expect-text` first) |
| An API request | one `client cN -connect ${tmpdir}/vigild.sock { txreq …; rxresp; expect resp.status == 409; expect resp.body ~ "…" } -run` per request, with a comment, so a failure names the case |
| Receive an alert | `server s1 { rxreq; expect req.body ~ "…"; txresp -hdr "Connection: close" } -start`, URL `http://${s1_addr}:${s1_port}/…`, then `server s1 -wait`; restart s1 with a new spec for the next alert |
| Create a file | `filewrite ${tmpdir}/check_marker ""` |
| Test the vigil CLI | `shell -exit 1 -expect {Error: …} { ${vigil} … }` or `-match {regex}`; no `grep`, no `!` |
| Wait a moment | `delay 0.5` |

Always start vigild with `-winsz 200 400`: `-expect-text` and `-match-text`
search only the emulated terminal screen, which is 24 x 80 by default. At
80 columns a vigild log line (often over 200 characters) wraps and a pattern
no longer matches; with 24 rows early lines such as "API listening" scroll
off.

In `expect … ~` patterns each `.` can stand for one character of a JSON
escape (`\"`, `\n`, `[`), which keeps them readable:
`"not running: ..idle.."`.

### Background process (POSIX sh / dash)

Use this pattern when a process must run in the background but is not started
via VTest2's `process` directive (e.g. when extra flags like `--tls-addr` are
needed):

```sh
cmd >> output.txt 2>/dev/null < /dev/null &
echo $! > ${tmpdir}/pid.txt
```

The `2>/dev/null < /dev/null` prevents the spawned process from inheriting
VTest2's internal pipe file descriptors, which would cause it to block.
`disown` is bash-only and must not be used — VTest2 shells run under `dash`.

### Variable expansion in heredocs

Use an unquoted delimiter (`<<EOF`, not `<<'EOF'`) when VTest2 variables like
`${tmpdir}` need to expand into the heredoc body (e.g. embedding a temp path
into a YAML layer file).

Use a quoted delimiter (`<<'EOF'`) when the body contains literal dollar signs
that must not be expanded (e.g. YAML with no VTest2 variables).

### Alert config: `retry-attempts: 1`

Always set `retry-attempts: 1` on alerts in tests. Without it, the overlord
blocks for ~3 seconds per failed HTTP delivery (3 retries with 1s + 2s
backoff). This delay makes polling loops time out and `vigil alerts list`
return stale data.

### Alert status transitions

`vigil alerts list` shows the last-known status from `AlertSender`, which is
only updated when a `CheckEvent` is received:

- A check with `startup: disabled` never runs → status stays `None` →
  displayed as `unknown`.
- A check that always passes and starts with `initial_status = Up` never
  sends a `CheckEvent` (no Up→Up transition) → status stays `None` →
  displayed as `unknown`.
- To reach `up` in the display: the check must first fail (→ `Down`), then
  pass (→ `Down→Up` transition event). Use an exec check whose command
  initially fails, then succeeds after a marker file is created.

### Checks `next_run_in_secs` branches

The `vigil checks list` formatter has four branches for the time-until-next-run
field. To cover all four in one test:

| Branch | How to trigger |
|--------|----------------|
| `None` → `"pending"` | `delay: 1h` — check stays in its initial-delay loop |
| `< 60 s` → `"Xs"` | `period: 30s, delay: 0s` — after first tick, ~30 s remain |
| `< 3600 s` → `"Xm Xs"` | `period: 2m, delay: 0s` — after first tick, ~120 s remain |
| `≥ 3600 s` → `"Xh Xm"` | `period: 2h, delay: 0s` — after first tick, ~7200 s remain |

With `delay: 0s`, Tokio's interval fires its first tick immediately, so a
`sleep 0.5` after `replan` is sufficient to ensure all three timed checks have
completed their first run.

### VTest2 server macro

When using a VTest2 `server` directive, use `${s1_sock}` (full `ip:port`) as
the URL base, not `${s1_addr}` (IP only, no port).
