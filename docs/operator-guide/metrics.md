# Metrics

vigild exposes Prometheus/OpenMetrics metrics at `GET /v1/metrics`
(access level `metrics`). This page lists every metric and shows PromQL
queries for the usual questions: is a dependency down, is the own app
down, how available was a check over the last 30 days.

## Scraping

`GET /v1/metrics` answers on the Unix socket and on the TLS listener
(`--tls-addr`). Over TLS a caller without an identity gets the access level
`--tls-anonymous-access`, which is `metrics` by default, so Prometheus needs
no credentials; see [TLS](tls.md).

```yaml
# prometheus.yml: scrape vigild's TLS listener on port 8443.
scrape_configs:
  - job_name: vigild
    scheme: https
    metrics_path: /v1/metrics
    tls_config:
      ca_file: /etc/prometheus/vigild-ca.pem
    static_configs:
      - targets: ["app-1.example.com:8443"]
```

Counters carry the suffix `_total` in the output
(`vigil_check_down_count_total`); the tables below use the names as they
appear there.

## Checks

Every check metric has the labels `check` (the check's name) and `level`
(`alive` or `ready`, see [`level`](../user-guide/configuration.md#level)).
Series exist for running checks only: a check that a replan removes, sets
to `startup: disabled` or gives another `level` loses its series.

| Metric | Type | Description |
|---|---|---|
| `vigil_health{level}` | gauge | `1` if `GET /v1/health?level=<level>` answers healthy (`200`), `0` if not (`502`). `level="alive"` counts alive checks, `level="ready"` all running checks. |
| `vigil_check_up{check,level}` | gauge | `1` while the check is up, `0` while it is down (after `threshold` failures in a row). |
| `vigil_check_success_count_total{check,level}` | counter | Runs that passed. |
| `vigil_check_failure_count_total{check,level}` | counter | Runs that failed, also those below `threshold`. |
| `vigil_check_down_count_total{check,level}` | counter | Outages: how often the check went from up to down. One outage counts once, however many runs fail during it. |
| `vigil_check_down_seconds_total{check,level}` | counter | Time the check spent down. It grows at each run while the check is down and when it recovers, so it lags behind by up to one `period`. |
| `vigil_check_duration_seconds{check,level,result}` | histogram | How long a run took, from its start to its result. `result` is `success` or `error`. A run that times out counts with the check's `timeout`. |
| `vigil_check_connect_duration_seconds{check,level,result}` | histogram | How long the connection took to come up. http: DNS, TCP connect, TLS handshake and a proxy tunnel; tcp: the connect. Exec checks and runs whose connection never came up have no value. |

The histograms have the buckets 1ms, 5ms, 10ms, 25ms, 50ms, 100ms, 250ms,
500ms, 1s, 2.5s, 5s, 10s, 30s and 60s, and the series `_bucket`, `_sum` and
`_count`. An HTTP check opens a new connection on every run, so each run
has a connect time.

## Services

| Metric | Type | Description |
|---|---|---|
| `vigil_services_count` | gauge | Number of configured services. |
| `vigil_service_info{service}` | gauge (always 1) | One series per configured service. |
| `vigil_service_active{service}` | gauge | `1` if the service is `active`, `0` otherwise. |
| `vigil_service_running{service}` | gauge | `1` while a process of the service is alive (`starting`, `active` or `stopping`), `0` otherwise; unlike `vigil_service_active` also `1` for a running `oneshot` job or a service still waiting for readiness. |
| `vigil_service_start_count_total{service}` | counter | Starts of the service, restarts included. |
| `vigil_service_last_exit_code{service}` | gauge | Exit code of the last process (`128 + signal` if killed by a signal, `-1` if unknown); absent until the service has exited once. |
| `vigil_service_last_run_timestamp_seconds{service}` | gauge | Unix time when the last run of a `type: oneshot` service ended. |
| `vigil_service_last_run_success{service}` | gauge | `1` if the last run of a `type: oneshot` service succeeded, else `0`. |
| `vigil_service_runs_total{service,result}` | counter | Finished runs of a `type: oneshot` service; `result` is `success` or `failure`. |
| `vigil_service_file_reloads_total{service}` | counter | Reactions (restart or signal) to a change of a watched file (`watch-files`). |

## vigild

| Metric | Type | Description |
|---|---|---|
| `vigil_alert_fire_count_total{alert}` | counter | Alerts sent as firing, per alert. |
| `vigil_layers_reloads_total{result}` | counter | Layer reloads (`vigil replan` or `--watch-layers`); `result` is `ok` or `invalid`. |
| `vigil_layers_last_reload_timestamp_seconds` | gauge | Unix time of the last successful layer reload (`0` = none since start). |

## PromQL examples

A dependency is down (the pod is not ready, see
[Kubernetes readiness/liveness via vigild API](orchestrators.md#kubernetes-readinessliveness-via-vigild-api)):

```promql
vigil_health{level="ready"} == 0 and on (instance) vigil_health{level="alive"} == 1
```

The own app is down (a liveness probe on `/v1/health?level=alive` restarts
the container):

```promql
vigil_health{level="alive"} == 0
```

Which dependency is down:

```promql
vigil_check_up{level="ready"} == 0
```

Outages of each check in the last 24 hours:

```promql
increase(vigil_check_down_count_total[24h])
```

Availability of each check over the last 30 days (`1` = never down,
`0.999` = down 43 minutes):

```promql
1 - rate(vigil_check_down_seconds_total[30d])
```

Failed runs that have not (yet) made a check down, an early warning:

```promql
increase(vigil_check_failure_count_total[15m]) > 0 and on (check, instance) vigil_check_up == 1
```

How long the runs of each check take, the 95th percentile over 5 minutes:

```promql
histogram_quantile(0.95, sum by (check, le) (rate(vigil_check_duration_seconds_bucket[5m])))
```

The connect alone (a slow DNS or TLS handshake):

```promql
histogram_quantile(0.95, sum by (check, le) (rate(vigil_check_connect_duration_seconds_bucket[5m])))
```

Runs that time out are counted with the check's `timeout`, so a 95th
percentile at the timeout means at least 5 % of the runs ran out of time.

A service restarts often:

```promql
increase(vigil_service_start_count_total[1h]) > 5
```

An alerting rule for a dependency that stays down:

```yaml
groups:
  - name: vigild
    rules:
      - alert: DependencyDown
        expr: vigil_check_up{level="ready"} == 0
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "{{ $labels.check }} on {{ $labels.instance }} is down"
```
