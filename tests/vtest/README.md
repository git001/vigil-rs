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
| `v0004_openapi.vtc` | `GET /openapi.json` — 200, spec contains `"vigil API"`, `info`, `paths`; `GET /docs` — the Swagger UI page that loads `/openapi.json` |
| `v0005_replan.vtc` | `POST /v1/replan` before and after the layers symlink points to a layer — service appears in list |
| `v0006_identities.vtc` | Identity CRUD: add alice (bootstrap auth), list (Unix UID auth), remove |
| `v0007_daemon_stop.vtc` | `POST /v1/vigild {"action":"stop"}` — daemon exits cleanly |
| `v0008_logs_query.vtc` | `GET /v1/logs` with `?n=` and `?services=` query parameters |
| `v0009_vigil_cli.vtc` | `vigil` CLI: system-info, services list/start/restart/stop, checks list, alerts list, logs, replan, identities add-local/list/remove |
| `v0010_vigil_log_relay.vtc` | `vigil-log-relay --source-socket` — forwards ndjson lines from the vigild Unix socket to a TCP sink; a second run configured only by environment variables (`--stdout-sink`, `INCLUDE`/`EXCLUDE`) |
| `v0011_vigil_log_relay_source_url.vtc` | `vigil-log-relay --source-url` — forwards an HTTP ndjson stream to a TCP sink |
| `v0012_vigil_alerts.vtc` | `vigil alerts list` — unknown / down / up display branches; a check with `startup: disabled` is `inactive` in `vigil checks list` and `GET /v1/checks` |
| `v0013_vigil_tls_and_reaper.vtc` | vigild TLS listener (`--tls-addr`), `vigil --insecure`, subreaper (`--reaper`) + SIGCHLD handling |
| `v0014_vigil_checks_logs_vigild.vtc` | `vigil checks list` (all four `next_run` formatting branches), `vigil logs` with entries, `vigil vigild status/stop` |
| `v0015_logs_push_tcp_reconnect.vtc` | `logs-push-addr`: vigild reconnects to the TCP sink after a disconnect |
| `v0016_vigil_alerts_body_template.vtc` | `body-template`: a webhook alert sends the rendered template instead of the default payload |
| `v0017_mtls.vtc` | mTLS client certificate authentication |
| `v0018_vigil_auth.vtc` | `vigil` CLI: local, Basic auth and mTLS authentication |
| `v0019_vigild_json_log_format_signals_restart.vtc` | `--log-format json`, SIGHUP/SIGUSR1/SIGUSR2 forwarding, daemon restart |
| `v0020_metrics_dynamic.vtc` | Metrics follow runtime state: `service_active`, `alert_fire_count` |
| `v0021_vigil_logs_follow.vtc` | `vigil logs`: `--follow`, service filter, `-n` limit, empty output |
| `v0022_vigil_https_transport.vtc` | `vigil` CLI over HTTPS: post, post_void, delete, follow_logs |
| `v0023_service_alerts.vtc` | Service alerts: backoff, recovered after `healthy-time`; checks of an unhealthy service stay quiet |
| `v0024_watch_layers.vtc` | `--watch-layers`: changed layers apply without replan; an invalid layer raises an alert |
| `v0025_shutdown_exit_codes.vtc` | Shutdown policies end vigild with the documented exit code |
| `v0026_layer_identities.vtc` | Identities in layer files: read-only over the API, rotated secret, removed by replan |
| `v0027_check_failure_reason.vtc` | Why a check fails: `last-error` in `GET /v1/checks` and `vigil checks list`, the reason in the `check failed` log line and in the alert message, cleared by a pass; `vigil checks run` (exit codes, reason, unknown check); `GET /v1/health` (200/502 by level and name, 404, 400) |
| `v0028_service_signals.vtc` | `vigil services signal` / `POST /v1/signals`: SIGHUP reaches a trapping service that keeps running; all or nothing with a stopped service; unknown service and signal name |
| `v0029_logs_follow.vtc` | `GET /v1/logs?follow=true`: the last `n` lines, then new lines as NDJSON; unknown parameters are 400 on `/v1/logs` and `/v1/logs/follow` |
| `v0030_replan_restart.vtc` | Replan restarts a running service whose config changed, then the services that require it |
| `v0031_check_metrics.vtc` | Check metrics: `level` label, `vigil_health` per level, outages, time down, run durations |
| `v0032_api_request_errors.vtc` | Invalid JSON bodies (415/400/422, also an unknown field) and unknown query parameters (400) get the error envelope, and only after the access check (403 first) |
| `v0033_vigild_log_output.vtc` | vigild's own log written to a file (as with `podman logs`): no ANSI colours, `pid` as a number, `path` without quotes, in text and JSON |
| `v0034_load.vtc` | Load: 9 clients at once (lists, logs, metrics, signals, replans, restarts, check runs, a log follower) against 21 services and 22 checks per second; no 5xx, no hang, vigild still answers, at most 19 threads and 96 MB RSS |
| `v0035_odd_service_output.vtc` | Service output that is not clean text: invalid UTF-8 becomes `�`, a 40000-byte line comes in 3 entries, the service keeps running (no SIGPIPE, one start) |
| `v0036_logs_multiline.vtc` | `logs-multiline`: a Java and a Python stack trace (with "Caused by", chained tracebacks) are one entry each with their log line; on stdout only the first line has the `[service]` prefix |
| `v0037_relay_multiline.vtc` | `vigil-log-relay --multiline java` on vigild's ndjson stream: a service without `logs-multiline` prints a trace, the relay writes it as one record with the joined message |
| `v0038_shutdown_delay.vtc` | `--shutdown-delay`: after SIGTERM `/v1/health` answers 503 (not `level=alive`) while the services keep running; a second SIGTERM stops at once |
| `v0039_relay_framing.vtc` | `vigil-log-relay --source-url` on a chunked stream mixing plain ndjson-style lines and SSE: `data:` lines are forwarded with and without the space, `event:`/`id:`/`retry:` and `:` comments are dropped, an invalid byte becomes `�` |
| `v0040_relay_filter_multiline.vtc` | `--include`/`--exclude` with `--multiline-start` on a text stream: the filter decides on the joined record, so a match in a continuation line keeps or drops the whole stack trace |
| `v0041_relay_max_line.vtc` | `--max-line-bytes 1024`: a 12000-byte line comes in two parts with one warning, a stack trace past 1024 bytes in two records; with `--include` the later parts follow the decision on the first |
| `v0042_relay_reconnect.vtc` | Reconnects: `--source-user-file` is read again per connection (rotated password), `--source-idle-timeout`, a successful connect resets the failure counter, the third failure in a row ends the relay (`--source-reconnect-retries 3`, exit code 1) |
| `v0043_relay_proxy.vtc` | Plain HTTP through a proxy: `--source-proxy` and `HTTP_PROXY` send the absolute-form request to the proxy, `--source-no-proxy` connects directly |
| `v0044_relay_tls.vtc` | The relay on vigild's TLS stream: `--source-cacert` or `--source-insecure` with a client certificate (mTLS identity); without a certificate 403, without the CA no connection |
| `v0045_relay_tcp_sink_reconnect.vtc` | The TCP sink stops and comes back on the same port: the relay reconnects and not one numbered line is missing |
| `v0046_relay_send_buffer.vtc` | `--send-buffer-lines 64` with the sink down: 430 of 494 lines are dropped and reported in one summed-up warning, not one per line |
| `v0047_shutdown_order.vtc` | Shutdown in the start order reversed: proxy (after app), then app, then db (required by app), each once the one before has exited (1s each, longer than the old 0.5s to SIGKILL); vigild exits after the last |
| `v0048_service_actions_wait.vtc` | `POST /v1/services` answers when the action is done: a start once the service is ready (meanwhile `GET /v1/changes/1` shows `doing`), a oneshot start at the end of its run (failed: `error` with the reason, `vigil` exits 1), a stop once the process is gone |
| `v0049_oneshot_run_timeout.vtc` | `run-timeout` stops a oneshot run that takes too long (SIGTERM ignored: SIGKILL after `kill-delay`); the run fails with `did not finish within 500ms` and counts in `vigil_service_runs_total{result="failure"}`; a run within its limit ends `done` |
| `v0050_plan.vtc` | `GET /v1/plan` and `vigil plan`: the plan merged from two layers, secrets masked (environment values, a header, a query value, a webhook path, a password hash), `command` and `labels` as written |

## How it works

VTest2 provides three key primitives used here:

- **`process`** — spawns a binary as a subprocess, capturing stdout/stderr; supports `-start`, `-kill`, `-wait`
- **`client -connect <path>`** — opens an HTTP/1.1 connection to a Unix socket
- **`shell`** — runs a POSIX sh snippet: the vigil CLI (`-expect`, `-match`), symlinks, TLS files and what else VTest2 cannot do

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
- `helpers/`: small standalone scripts for what VTest2 cannot do:
  `helpers/tcp_capture.py` (a TCP sink that appends what it receives to a
  file and reports its port in a file; `--port` binds a given port again,
  e.g. to test a reconnect; VTest2 cannot check raw TCP content) and
  `helpers/tls_identity.py` (request body for a TLS identity whose CA the
  test creates at runtime) and `helpers/relay_until.sh` (runs
  vigil-log-relay with `--stdout-sink` until its output has a matching
  line or it exits, for several relay runs in one test).

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

### What stays in shell

VTest2 has no TLS, cannot check the content of a raw TCP stream and cannot
connect to a port known only at run time (`--tls-addr 127.0.0.1:0`, a
healthcheck on `:0`). So TLS calls use `vigil --url https://… --insecure`
(with `--cert`/`--key` for mTLS), certificates come from `openssl`, a TCP
sink is `helpers/tcp_capture.py`, and the TLS port is read from `vigil
system-info` into a file. VTest2 cannot make symlinks either: `ln -s` and
`ln -sfn` switch a layers directory between fixtures.

A helper started in the background redirects all three file descriptors
(`> /dev/null 2>&1 < /dev/null &`), or it holds VTest2's output pipe open.
VTest2 shells run under `dash`: no `disown`, no bash syntax.

### Signals and exit codes

`process p1 -kill HUP` sends a signal; VTest2 knows TERM, INT, KILL and HUP
by name, others as a negative number (`-kill -10` for SIGUSR1 on Linux).
For a process that ends by itself, set `-expect-exit N` on the line with
`-start`: it may exit before `-wait`.

### Alert config: `retry-attempts: 1`

Always set `retry-attempts: 1` on alerts in tests. Without it, the overlord
blocks for ~3 seconds per failed HTTP delivery (3 retries with 1s + 2s
backoff). This delay makes `vigil alerts list` return stale data.

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

With `delay: 0s` the first run starts at once, so a `delay 0.5` after vigild
is up is enough for all three timed checks to have run once (passing exec
checks log nothing to wait for).

### VTest2 server macro

When using a VTest2 `server` directive, use `${s1_sock}` (full `ip:port`) as
the URL base, not `${s1_addr}` (IP only, no port).
