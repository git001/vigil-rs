# vigil-rs examples

Each directory demonstrates a different aspect of vigil-rs. The examples are
ordered from simplest to most involved.

All container examples are built from the **repository root** so the
Rust sources are available to the builder stage:

```
podman build -f examples/<name>/Containerfile -t vigil-<name> .
```

---

## layers-minimal — quickstart without a container

**Features:** single layer · exec check · TCP check · `after:` ordering · restart/backoff

The fastest way to see vigil-rs in action. No container, no extra packages —
only a POSIX shell is required. Three shell-based services show the core
lifecycle:

| Service   | Behaviour                                               |
|-----------|---------------------------------------------------------|
| `ticker`  | Prints a timestamp every second (runs forever)          |
| `counter` | Counts 1–30 then exits cleanly; auto-restarts           |
| `canary`  | Crashes after 10 s to demonstrate `on-failure: restart` |

```
vigild --layers-dir examples/layers-minimal --socket /tmp/vigild.sock &
vigil --socket /tmp/vigild.sock services list
vigil --socket /tmp/vigild.sock logs -f
```

---

## layers — multi-layer configuration reference

**Features:** layer merge · `override: replace` · `after:` · environment · disabled-by-default services · dev/prod split

Four YAML files show how layers compose in numeric order:

| File                      | Purpose                                              |
|---------------------------|------------------------------------------------------|
| `000-base.yaml`           | PostgreSQL + Redis (infrastructure layer)            |
| `100-app.yaml`            | Web API + background worker (app layer)              |
| `200-worker-enabled.yaml` | Opt-in layer that flips the worker to `startup: enabled` |
| `900-dev-overrides.yaml`  | Dev overrides: looser settings, Redis stub, debug log |

Key patterns illustrated:

- `override: merge` — patch individual fields without replacing the full definition
- `override: replace` — swap a service wholesale in a higher layer (used for dev)
- Worker service is `startup: disabled` in `100-app.yaml`; `200-worker-enabled.yaml`
  enables it with a single line — no other fields duplicated
- `900-dev-overrides.yaml` substitutes a `sleep infinity` stub for Redis so the
  dependency graph resolves without a real Redis in development

---

## full-container — h2o web server

**Features:** HTTP health check · `on-check-failure: restart` · exponential backoff

A minimal complete container: the [h2o](https://h2o.examp1e.net/) HTTP/1.1 +
HTTP/2 web server supervised by vigil-rs with an HTTP liveness probe.

```
podman build -f examples/full-container/Containerfile -t vigil-h2o .
podman run --rm -p 8080:8080 vigil-h2o
curl http://localhost:8080/healthz
```

---

## hug — HAProxy + controller

**Features:** two services · `after:` ordering · exec check over Unix socket · `SIGUSR1` graceful drain

Two cooperating services: HAProxy runs in master-worker mode, a controller
configures it via the HAProxy runtime API once the socket is ready.

- `after: [haproxy]` defers the controller start until HAProxy is `active`
- The health check uses `curl --unix-socket` to query HAProxy's stats socket —
  demonstrates an exec check that does not require an open TCP port
- `stop-signal: SIGUSR1` triggers HAProxy's graceful drain

```
podman build -f examples/hug/Containerfile -t vigil-hug .
podman run --rm -p 8080:8080 vigil-hug
```

---

## identities — service identity management

**Features:** exec checks with `service-context` · `kill -0` liveness pattern · flaky service / restart demo

Three lightweight shell services (`counter`, `ticker`, `flaky`) provide
something observable while demonstrating the identity and user-context
features of vigil-rs.

- `service-context: <name>` on an exec check inherits the service's
  `user`, `group`, `working-dir`, and `environment`
- `flaky` exits randomly with a non-zero code to exercise `on-failure: restart`
  and the backoff logic

```
podman build -f examples/identities/Containerfile -t vigil-identities .
podman run --rm vigil-identities
```

---

## migrate-from-s6 — side-by-side s6-overlay → vigil-rs migration

**Features:** `logs-forward: disabled` · `after:` · HTTP + TCP checks · migration guide

Shows the same two-service stack (nginx + API) built twice:

| File                | Description                                    |
|---------------------|------------------------------------------------|
| `Containerfile.s6`  | s6-overlay "before" version (Ubuntu)           |
| `Containerfile`     | vigil-rs "after" version (Alpine)              |
| `s6/s6-rc.d/`       | Original s6 service definitions for comparison |
| `layers/`           | Equivalent vigil-rs layer                      |

The s6 version requires a separate `nginx-healthcheck` longrun service (a
polling loop) just to run a health check. The vigil-rs version expresses the
same check as two lines of YAML.

Notable vigil-rs layer settings:
- `logs-forward: disabled` on nginx — nginx access logs go to the internal
  log buffer only (`vigil logs -f`) and are not printed to `podman logs`
- `after: [nginx]` on the API service — replaces `dependencies.d/nginx`

```
# Build the s6 version
podman build -f examples/migrate-from-s6/Containerfile.s6 -t vigil-s6-before .

# Build the vigil-rs version
podman build -f examples/migrate-from-s6/Containerfile -t vigil-s6-after .
```

---

## tomcat — Apache Tomcat 10.1

**Features:** JVM startup delay · HTTP alive + ready checks · `on-check-failure: restart` · stdout-only Tomcat logging

Apache Tomcat 10.1 supervised by vigil-rs. Demonstrates JVM-specific
considerations:

- Tomcat 10.1's official image ships an **empty** `webapps/` directory; the
  Containerfile copies the `ROOT` webapp from `webapps.dist/` so the health
  check has a target
- `delay: 15s` on the alive check accounts for JVM + webapp startup time
- A separate `tomcat-ready` check at a longer period and lower frequency models
  a Kubernetes-style readiness probe
- `conf/logging.properties` redirects all Tomcat logs to stdout so
  `logs-forward: enabled` captures them via `podman logs`

```
podman build -f examples/tomcat/Containerfile -t vigil-tomcat .
podman run --rm -p 8080:8080 --name vigil-tomcat vigil-tomcat

podman exec vigil-tomcat vigil services list
podman exec vigil-tomcat vigil checks list
```

---

## php-caddy — Caddy + PHP-FPM

**Features:** two services · `after:` · HTTP check · FastCGI ping check · aggregated health endpoint

Caddy acts as the HTTP front-end; PHP-FPM processes requests via FastCGI.
Three health endpoints are exposed:

| Endpoint       | What it checks                                              |
|----------------|-------------------------------------------------------------|
| `/caddy-health` | Caddy itself — static `200 OK`, no back-end involved        |
| `/php-health`  | PHP-FPM's built-in ping endpoint via FastCGI transport      |
| `/healthz`     | PHP script that checks the FPM Unix socket and returns 200/503 |

Key points:
- `after: [php-fpm]` ensures Caddy starts only once the FPM socket exists
- The `/php-health` check uses Caddy's `transport fastcgi { env SCRIPT_FILENAME /ping }`
  to reach FPM's internal ping endpoint — no PHP script needed
- The default Alpine `www.conf` is removed to avoid a conflicting `[www]` pool

```
podman build -f examples/php-caddy/Containerfile -t vigil-php-caddy .
podman run --rm -p 8080:8080 --name vigil-php-caddy vigil-php-caddy

curl http://localhost:8080/caddy-health
curl http://localhost:8080/php-health
curl http://localhost:8080/healthz
curl http://localhost:8080/
```

---

## php-fpm — standalone PHP-FPM (no web server)

**Features:** exec check · `pgrep` liveness pattern · `SIGQUIT` graceful drain · Unix socket

PHP-FPM running without any web server in front of it. Typical use case:
PHP queue workers or background job processors that consume tasks from a
queue (Redis, database, filesystem) without needing HTTP.

The health check uses `pgrep` to verify the FPM master process is present
in the process table — no port or socket connection required:

```yaml
exec:
  command: pgrep -x php-fpm83
```

`-x` matches the exact binary name so the check cannot accidentally match
an unrelated process. Exit code 0 = at least one match found = check passes.

A Unix socket (`/run/php-fpm/php-fpm.sock`) is still configured so a web
server can be wired in via an additional layer later without rebuilding the
image.

```
podman build -f examples/php-fpm/Containerfile -t vigil-php-fpm .
podman run --rm --name vigil-php-fpm vigil-php-fpm

podman exec vigil-php-fpm vigil checks list
podman exec vigil-php-fpm pgrep -a php-fpm83
```

---

## quarkus — Quarkus (Vert.x / Netty)

**Features:** three-stage build · SmallRye Health liveness + readiness probes · `logs-forward: enabled` · fast-jar layout

A Quarkus 3.x application built as a fast-jar and supervised by vigil-rs.
The three-stage Containerfile keeps the final image small:

| Stage             | Base image                          | Purpose                         |
|-------------------|-------------------------------------|---------------------------------|
| `quarkus-builder` | `maven:3.9-eclipse-temurin-21-alpine` | Compile + package Quarkus app   |
| `vigil-builder`   | `rust:alpine`                       | Build static `vigild` + `vigil` |
| final             | `eclipse-temurin:21-alpine`         | JRE + app + vigil binaries      |

Two checks map directly to Quarkus's SmallRye Health endpoints:

| Check           | Endpoint            | Vigil level | Meaning                            |
|-----------------|---------------------|-------------|------------------------------------|
| `quarkus-alive` | `/q/health/live`    | `alive`     | JVM is responsive                  |
| `quarkus-ready` | `/q/health/ready`   | `ready`     | All `@Readiness` checks pass (DB, …) |

The `DatabaseReadinessCheck` (`@Readiness`) models an external dependency
check that would gate load-balancer traffic in production.

```
podman build -f examples/quarkus/Containerfile -t vigil-quarkus .
podman run --rm -p 8080:8080 --name vigil-quarkus vigil-quarkus

curl http://localhost:8080/hello
curl http://localhost:8080/q/health/live
curl http://localhost:8080/q/health/ready
podman exec vigil-quarkus vigil checks list
```

---

## filebeat — Filebeat log collector via ndjson stream

**Features:** `logs-forward: disabled` · `logs-forward: passthrough` · `after:` · vigild ndjson API · Filebeat stdin input

Demonstrates log collection without a sidecar container. vigild's ndjson
log stream (`/v1/logs/follow?format=ndjson`) is consumed by a Filebeat process
that runs as a regular supervised service alongside the application.

Data flow:

```
dummy-logger  →  vigild log buffer  →  /v1/logs/follow?format=ndjson
                                    →  curl (one JSON object per line)
                                    →  Filebeat stdin input (json.keys_under_root)
                                    →  add_fields (collector=filebeat)
                                    →  console output (JSON lines → podman logs)
```

Key settings:

| Service        | `logs-forward`  | Effect                                                    |
|----------------|-----------------|-----------------------------------------------------------|
| `dummy-logger` | `disabled`      | Raw lines go to log buffer only — not to `podman logs`    |
| `filebeat`     | `passthrough`   | Filebeat owns its own stdout; vigild does not intercept   |

```
podman build -f examples/filebeat/Containerfile -t vigil-filebeat .
podman run --rm --name vigil-filebeat vigil-filebeat

# Expected output:
# {"@timestamp":"...","@metadata":{...},"collector":"filebeat","service":"dummy-logger","stream":"stdout","message":"INFO ...",...}

podman exec vigil-filebeat vigil services list
podman exec vigil-filebeat vigil logs -f
```

---

## filebeat-push — Filebeat via vigild push (Unix socket)

**Features:** `logs-push-socket` · `logs-forward: disabled` · `logs-forward: passthrough` · Filebeat unix input

Demonstrates vigild's **push mode**: vigild connects to Filebeat's Unix socket and
streams ndjson directly — no curl, no SSE framing, no polling.

Data flow:

```
dummy-logger  →  vigild push task  →  /run/collector/input.sock  (Unix socket)
                                    →  Filebeat unix input (decode_json_fields)
                                    →  add_fields (collector=filebeat)
                                    →  console output (JSON lines → podman logs)
```

Key difference from `filebeat`: vigild is the **client** (connects and pushes),
Filebeat is the **server** (listens on the socket). No curl process needed.

```
podman build -f examples/filebeat-push/Containerfile -t vigil-filebeat-push .
podman run --rm --name vigil-filebeat-push vigil-filebeat-push

# Expected output:
# {"@timestamp":"...","collector":"filebeat","message":"INFO ...","service":"dummy-logger","stream":"stdout",...}

podman exec vigil-filebeat-push vigil services list
podman exec vigil-filebeat-push vigil logs -f
```

---

## fluentbit — Fluent Bit via vigild push (TCP)

**Features:** `logs-push-addr` · `logs-forward: disabled` · `logs-forward: passthrough` · Fluent Bit tcp input

Demonstrates vigild's **push mode** with TCP: vigild connects to Fluent Bit's TCP
listener on `127.0.0.1:5170` and streams ndjson directly.

Data flow:

```
dummy-logger  →  vigild push task  →  127.0.0.1:5170  (TCP)
                                    →  Fluent Bit tcp input (Format json)
                                    →  record_modifier (add collector=fluent-bit)
                                    →  stdout sink (JSON lines → podman logs)
```

Uses `fluent/fluent-bit:4.2-debug` — the debug variant includes a shell, which
is required to run vigild as PID 1 alongside Fluent Bit.

```
podman build -f examples/fluentbit/Containerfile -t vigil-fluentbit .
podman run --rm --name vigil-fluentbit vigil-fluentbit

# Expected output:
# {"date":...,"timestamp":"...","service":"dummy-logger","stream":"stdout","message":"INFO ...","collector":"fluent-bit"}

podman exec vigil-fluentbit vigil services list
podman exec vigil-fluentbit vigil logs -f
```

---

## vector — Vector log collector via ndjson stream

**Features:** `logs-forward: disabled` · `logs-forward: passthrough` · `after:` · vigild ndjson API · Vector exec source

Demonstrates the same topology as the Filebeat example using
[Vector](https://vector.dev/) as the collector. Vector runs on Alpine (musl)
which matches the vigild builder stage — no extra base-image gymnastics needed.

Data flow:

```
dummy-logger  →  vigild log buffer  →  /v1/logs/follow?format=ndjson
                                    →  Vector exec source (curl, one JSON object per line)
                                    →  remap transform (add collector=vector, derive level)
                                    →  console sink (JSON → podman logs)
```

The `remap` transform also drops exec-source metadata fields (`command`,
`source_type`, `pid`, `host`) that Vector adds automatically but are not useful
downstream.

```
podman build -f examples/vector/Containerfile -t vigil-vector .
podman run --rm --name vigil-vector vigil-vector

# Expected output:
# {"collector":"vector","level":"info","message":"INFO ...","service":"dummy-logger",...}

podman exec vigil-vector vigil services list
podman exec vigil-vector vigil logs -f
```

---

## kubernetes-pod-logs — Kubernetes pod log collector

**Features:** `after:` · TCP push to Filebeat · dynamic pod watch · in-cluster Kubernetes API · RBAC

Collects logs from pods running in a Kubernetes namespace. Designed to run
**inside** the cluster as a single-container Deployment — no DaemonSet, no
sidecar, no external log shipper infrastructure required.

vigil-rs supervises two services in one container:

| Service | Role |
|---|---|
| `pod-log-collector` | `vigil-http-streamer --kubernetes`: watches pods, streams logs via Kubernetes API, pushes ndjson to Filebeat via TCP |
| `filebeat` | Listens on TCP 5170, decodes ndjson, emits enriched JSON to stdout |

The pod list is refreshed every `WATCH_INTERVAL` seconds (default: 30 s):
- New pods get a log stream started automatically.
- Deleted pods have their stream stopped and cleaned up.
- Crashed streams (pod evicted mid-stream) are restarted on the next cycle.

Data flow:

```
Kubernetes API  →  vigil-http-streamer --kubernetes
                →  ndjson: {timestamp, namespace, pod, stream, message}
                →  TCP → Filebeat 127.0.0.1:5170
                →  decode_json_fields (pod/namespace/message to root)
                →  console sink (JSON lines → kubectl logs)
```

If Filebeat or the collector crashes, vigil-rs restarts with exponential
backoff. `after: [filebeat]` ensures the collector does not push before
Filebeat's TCP listener is ready.

**Log latency:** Filebeat's internal event queue flushes every 10 seconds by
default (`queue.mem.flush.timeout`). Expect up to ~10 s delay between a log
line being written in the source pod and it appearing in `kubectl logs`. To
reduce latency, add `queue.mem: flush.timeout: 1s` to `filebeat.yml`.

```
# Build
podman build -f examples/kubernetes-pod-logs/Containerfile \
             -t vigil-k8s-pod-logs .

# Deploy (requires cluster access)
kubectl apply -f examples/kubernetes-pod-logs/k8s/rbac.yaml
kubectl apply -f examples/kubernetes-pod-logs/k8s/deployment.yaml

# Watch logs
kubectl logs -f -n monitoring deploy/vigil-pod-log-collector

# Inspect from inside
kubectl exec -n monitoring deploy/vigil-pod-log-collector -- vigil services list
kubectl exec -n monitoring deploy/vigil-pod-log-collector -- vigil checks list
```

Required RBAC (`k8s/rbac.yaml`):

```yaml
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list"]
  - apiGroups: [""]
    resources: ["pods/log"]
    verbs: ["get"]
```
