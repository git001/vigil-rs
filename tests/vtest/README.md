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

## How it works

VTest2 provides three key primitives used here:

- **`process`** — spawns a binary as a subprocess, capturing stdout/stderr; supports `-start`, `-kill`, `-wait`
- **`client -connect <path>`** — opens an HTTP/1.1 connection to a Unix socket
- **`shell`** — runs a POSIX sh snippet for setup, teardown, polling loops, or output assertions

The macros passed via `-D` hold paths to the binaries:

| Macro | Binary |
|-------|--------|
| `${vigild}` | vigild daemon |
| `${vigil}` | vigil CLI |
| `${vigil_log_relay}` | vigil-log-relay |

`${tmpdir}` is a per-test temporary directory provided automatically by VTest2.

## Patterns and gotchas

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
