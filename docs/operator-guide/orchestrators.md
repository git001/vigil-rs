# Orchestrator Integration

## Kubernetes

### Basic Pod with vigild as PID 1

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: myapp
spec:
  containers:
    - name: app
      image: ghcr.io/your-org/myapp:latest
      # vigild is the ENTRYPOINT in the image — no command override needed

      ports:
        - containerPort: 8080   # application port
        - containerPort: 8443   # vigild TLS API (optional)

      env:
        - name: VIGIL_TLS_ADDR
          value: "0.0.0.0:8443"

      # Liveness probe: 502 while an alive check is down
      livenessProbe:
        httpGet:
          path: /v1/health?level=alive
          port: 8443
          scheme: HTTPS
        initialDelaySeconds: 10
        periodSeconds: 15

      # Size these for the services vigild runs, including the processes of
      # exec checks. vigild itself needs about 25 MiB plus 150 KiB per
      # service, more with many services or checks; see "Sizing memory and
      # CPU" in container-setup.md. VIGIL_WORKER_THREADS is not a CPU
      # request.
      resources:
        requests:
          cpu: 250m
          memory: 256Mi
        limits:
          memory: 512Mi

      volumeMounts:
        - name: vigil-layers
          mountPath: /etc/vigil/layers

  volumes:
    - name: vigil-layers
      configMap:
        name: myapp-vigil-layers
```

The example sets no CPU limit, a common choice to avoid throttling. vigild
then sees every CPU of the node but still uses only 2 worker threads (see
[`VIGIL_WORKER_THREADS`](container-setup.md#vigild-cli-flags)). How much
memory and CPU vigild needs per service and per check is listed in
[Sizing memory and CPU](container-setup.md#sizing-memory-and-cpu).

### Environment variables and Secrets

Variables from `env:` / `envFrom:` (including Secrets, e.g. synced by the
External Secrets Operator) are set on the container, so they reach **vigild**.
Since v4, services do not inherit them automatically: list what each service
needs with `pass-env-glob` / `pass-env-regex` (see the
[configuration reference](../user-guide/configuration.md#environment--pass-env--pass-env-glob--pass-env-regex)).

```yaml
services:
  api:
    command: /usr/local/bin/api
    pass-env-glob: [DATABASE_URL, DB_PASSWORD, APP_*]
```

Use `pass-env: true` to restore the pre-v4 behaviour for a service.

#### Secret files

A Secret mounted as a volume (without `subPath`) can be read with a
[`file` template](../user-guide/configuration.md#templates-in-command-and-environment).
Unlike `env:`, the kubelet updates the file when the Secret changes (after
about a minute). With [`watch-files`](../user-guide/configuration.md#watch-files)
vigild notices the change and restarts the service with the new value, so
stakater Reloader is not needed and only the affected service restarts, not
the pod:

```yaml
services:
  api:
    command: /usr/local/bin/api
    environment:
      DB_PASSWORD: "{{ file('/etc/secrets/db-password') }}"
    watch-files: {}     # restart api when the Secret changes
```

#### Vault Agent and other env files

The Vault Agent Injector writes rendered secrets to `/vault/secrets/<name>`,
usually as `KEY=value` lines. Read such a file with
[`env-file`](../user-guide/configuration.md#env-file):

```yaml
# Pod annotations
vault.hashicorp.com/agent-inject: "true"
vault.hashicorp.com/agent-inject-secret-db: database/creds/app
vault.hashicorp.com/agent-inject-template-db: |
  {{- with secret "database/creds/app" -}}
  export DB_USER="{{ .Data.username }}"
  export DB_PASSWORD="{{ .Data.password }}"
  {{- end }}
```

```yaml
# vigil layer
services:
  api:
    command: /usr/local/bin/api
    env-file: [/vault/secrets/db]
```

The file is read on every start of the service. Add `watch-files: {}` to
restart the service when the agent renews the credentials and rewrites the
file ([`watch-files`](../user-guide/configuration.md#watch-files)).

#### Memory and CPU templates

`{{ mem(…) }}` and `{{ cpu(…) }}` follow the container's `resources.limits`:

| Kubernetes | Seen by vigild | Template |
|---|---|---|
| `limits.memory: 2Gi` | cgroup `memory.max` = 2147483648 | `mem("75%")` = `1536` |
| `limits.cpu: 1500m` | cgroup `cpu.max` = `150000 100000` | `cpu()` = `2` (rounded up) |
| Guaranteed pod, static CPU manager | exclusive cpuset | `cpu()` = number of pinned CPUs |
| `requests.*` only | nothing | — |

Requests are not visible inside the container, only limits are. **Without
limits** the templates use the whole node (all its RAM and CPUs) and vigild
logs a warning. To size by requests, expose them with the Downward API and
read the variable:

```yaml
# Pod spec
env:
  - name: CPU_REQUEST
    valueFrom:
      resourceFieldRef: {containerName: app, resource: requests.cpu, divisor: "1"}
  - name: MEM_REQUEST_MIB
    valueFrom:
      resourceFieldRef: {containerName: app, resource: requests.memory, divisor: 1Mi}
```

```yaml
# vigil layer
services:
  app:
    command: /usr/local/bin/app --workers {{ env.CPU_REQUEST }}
    pass-env-glob: [CPU_REQUEST, MEM_REQUEST_MIB]
```

A `divisor: "1"` rounds a CPU request up to whole CPUs.

### ConfigMap for layers

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: myapp-vigil-layers
data:
  001-app.yaml: |
    summary: myapp layer
    services:
      myapp:
        command: /usr/local/bin/myapp
        startup: enabled
        stop-signal: SIGTERM
        kill-delay: 30s
        on-success: restart
        on-failure: restart
        on-check-failure:
          myapp-alive: restart
    checks:
      myapp-alive:
        level: alive
        startup: enabled
        delay: 5s
        period: 10s
        timeout: 3s
        threshold: 3
        http:
          url: http://localhost:8080/healthz
```

### Kubernetes readiness/liveness via vigild API

vigild exposes its REST API directly over HTTPS — use `httpGet` probes
without requiring the `vigil` CLI to be present in the image:

```yaml
# Liveness: no check with level: alive is down
livenessProbe:
  httpGet:
    path: /v1/health?level=alive
    port: 8443
    scheme: HTTPS   # the kubelet does not verify the certificate
  initialDelaySeconds: 10
  periodSeconds: 30

# Readiness: no check is down (level=ready counts ready and alive checks)
readinessProbe:
  httpGet:
    path: /v1/health?level=ready
    port: 8443
    scheme: HTTPS
  initialDelaySeconds: 5
  periodSeconds: 10
```

Requires `VIGIL_TLS_ADDR: "0.0.0.0:8443"` (or `--tls-addr`) to enable the
HTTPS listener. `GET /v1/health` needs no credentials and answers `200`
while none of the selected checks is down, `502` otherwise; checks with
`startup: disabled` do not count. `?names=a,b` selects checks by name; a
name that is not a check is `404`, so a typo cannot pass unnoticed. If a
service restarts itself on a failed check (`on-check-failure: restart`),
give vigild time for that before the kubelet restarts the whole container:
`periodSeconds` x `failureThreshold` longer than the check's `period` x
`threshold` plus the service's start.

**Which checks belong to which probe.** A failed liveness probe restarts
the container, so `level: alive` is only for checks of the app in this
container (Tomcat, Keycloak, Spring Boot on `localhost`). Checks of
dependencies (database, message broker, other services, external APIs) get
`level: ready`: while one is down the pod gets no traffic but keeps
running, and becomes ready again by itself. A dependency checked as `alive`
(also by default, when `level` is not set) restarts every replica while the
database is away and fixes nothing; vigild warns about such checks at load
time. See [`level`](../user-guide/configuration.md#level) and the flow chart
[check-levels.svg](../../images/check-levels.svg).

### Scaling: several replicas

A Deployment with `replicas: N` (or a HorizontalPodAutoscaler) runs one
vigild per pod, each as PID 1 with its own plan; the replicas know nothing
of each other. The pod is the unit that scales: all services vigild runs
in it scale together. That suits services that belong together (php-fpm
and Caddy, Tomcat and its log shipper). One service inside the pod cannot
be scaled on its own; give it more workers or threads, or its own
Deployment.

**Scaling up.** A new pod gets requests only once its readiness probe
(`/v1/health?level=ready`) passes, that is once vigild's `ready` checks of
the app pass. The HorizontalPodAutoscaler sees the CPU and memory of the
whole container, all its processes together; vigild's own share is small
(see [Sizing](container-setup.md#sizing-memory-and-cpu)).

**Scaling down and rollouts.** Kubernetes sends SIGTERM and removes the
pod from the Service's endpoints at the same time; kube-proxy and ingress
controllers notice that a few seconds later. Without a delay vigild stops
the services at once, and the requests of those seconds fail. With
`--shutdown-delay` vigild answers `503` on `/v1/health` at once, so the
readiness probe takes the pod out, and stops the services only after the
delay:

```yaml
spec:
  terminationGracePeriodSeconds: 30   # delay + time the services need to stop (below)
  containers:
    - name: app
      env:
        - name: VIGIL_SHUTDOWN_DELAY
          value: 5s
      readinessProbe:
        httpGet:
          path: /v1/health?level=ready
          port: 8443
          scheme: HTTPS
        periodSeconds: 2
        failureThreshold: 1
```

The liveness probe (`level=alive`) is not affected, so the kubelet does
not restart a container that is stopping. A second SIGTERM stops at once.

The time the services need to stop: vigild stops them in the start order
reversed, each after the ones that depend on it have exited, and gives each
at most its `kill-delay` (default `5s`). The worst case is the sum of the
`kill-delay`s along the longest `after`/`requires` chain, e.g. `proxy` →
`app` → `db` with the defaults: 15s. Keep `terminationGracePeriodSeconds`
above `--shutdown-delay` plus that sum, or the kubelet's SIGKILL ends the
services first.

**Per pod, not per Deployment:**

| Topic | With N replicas | What to do |
|---|---|---|
| Oneshots with `schedule` | run in every replica, N times | a separate Deployment with `replicas: 1`, or a Kubernetes CronJob |
| Alerts | every replica sends its own; a shared dependency that fails alerts N times | group them in Alertmanager by labels |
| ConfigMap with `--watch-layers` | every pod applies the change when its kubelet syncs it, about at the same time, without `maxUnavailable` | for risky changes put a hash of the ConfigMap into a pod annotation, so Kubernetes rolls the pods one by one |
| Identities added over the API | exist only in the pod where they were added, and are gone after a restart | define identities in the layers (ConfigMap, Secret) |
| `vigil-log-relay --kubernetes` | every replica streams the same pod logs | `replicas: 1` (see below) |
| Generated TLS certificate | every replica has its own | `--cert` and `--key` from a Secret when clients verify it |

### Kubernetes pod log collector

[`examples/kubernetes-pod-logs/`](../../examples/kubernetes-pod-logs/) is a
self-contained Deployment that collects logs from other pods in a namespace
via the Kubernetes API and forwards them to Filebeat — no DaemonSet, no
cluster-level log agent required.

vigild supervises both Filebeat and `vigil-log-relay`, restarting either
on failure. An HTTP `alive` check on `GET /healthz` detects a stalled watch loop.

```bash
# Build (from repo root)
podman build -f examples/kubernetes-pod-logs/Containerfile -t vigil-k8s-pod-logs .

# Deploy
kubectl apply -f examples/kubernetes-pod-logs/k8s/rbac.yaml
kubectl apply -f examples/kubernetes-pod-logs/k8s/deployment.yaml

# Inspect collector status
kubectl exec <pod> -- vigil services list
kubectl exec <pod> -- vigil logs -f pod-log-collector
```

Configure which pods to collect via env vars in the Deployment manifest:

| Variable | Default | Description |
|---|---|---|
| `NAMESPACE` | `default` | Namespace to watch |
| `POD_SELECTOR` | *(all pods)* | Label selector, e.g. `app=myapp` |
| `CONTAINER_SELECTOR` | *(default container)* | Container(s) to stream: a name, a comma-separated list, or `all` |
| `WATCH_INTERVAL` | `10` | How quickly a stream is restarted after the API server closed it; pods are discovered by events |
| `HEALTHCHECK_MAX_AGE` | `30` | Seconds without a watch tick before `/healthz` returns 503 (keep ≥ 3 × `WATCH_INTERVAL`) |

Every other option has a variable too; see the
[vigil-log-relay reference](../user-guide/cli-reference.md#vigil-log-relay).

**OpenShift:** the image uses `WORKDIR /tmp` and `VIGIL_PATH_PREFIX=/tmp/vigil`
because OpenShift mounts `/run` as a root-owned tmpfs at runtime. vigild
creates `/tmp/vigil` (mode 0711) at startup; the `vigil` client reads the same
variable and finds the socket. With `readOnlyRootFilesystem: true`, mount an
`emptyDir` at `/tmp` (or point the prefix at a subdirectory of another
`emptyDir`). An
`image.openshift.io/triggers` annotation in `k8s/deployment.yaml` rolls the
Deployment automatically on `oc start-build`.

### Replan on ConfigMap change

With `--watch-layers` (or `VIGIL_WATCH_LAYERS`) vigild notices changed layer
files itself and replans, so a ConfigMap update needs no `kubectl exec`.
A change while vigild starts (a ConfigMap updated during a rollout) is not
missed: vigild compares against the files as it loaded them.

```yaml
spec:
  containers:
    - name: app
      env:
        - name: VIGIL_LAYERS
          value: /etc/vigil/layers
        - name: VIGIL_WATCH_LAYERS
          value: 30s                 # check every 30s (1s to 24h)
      volumeMounts:
        - name: layers
          mountPath: /etc/vigil/layers   # the whole directory, no subPath
  volumes:
    - name: layers
      configMap:
        name: myapp-vigil-layers
```

- **No `subPath`:** the kubelet never updates files mounted with `subPath`,
  so vigild would not see a change. Mount the ConfigMap as a directory.
- **Delay:** the kubelet updates a mounted ConfigMap after about 1-2 minutes
  (its sync period plus cache); vigild applies it within two intervals
  after that (one to see the change, one to make sure it is complete).
- **Same as `vigil replan`:** only changed services and checks are
  restarted, plus the services that need a restarted one via `requires`;
  finished oneshot jobs do not run again. See
  [`vigil replan`](../user-guide/cli-reference.md#replan).
- **Invalid layers** (a YAML error, a wrong value, a failed validation)
  are not applied: the running plan stays, vigild logs the error and sends
  the alert `vigild`/`layers`/`invalid` (subscribe with
  `on-vigild: [layers]`). The next change of the files is tried again; a
  successful reload resolves the alert. A manual `vigil replan` reports the
  same way.
- vigild reads the layer files off its main loop with a 10s timeout, so a
  hanging volume (e.g. a dead NFS mount) does not block it.
- Metrics: `vigil_layers_reloads_total{result="ok|invalid"}` and
  `vigil_layers_last_reload_timestamp_seconds`.

Without `--watch-layers`, trigger the replan yourself after updating the
ConfigMap:

```bash
kubectl exec <pod> -- vigil replan
```

or over the REST API, with an identity of access `write`:

```bash
curl -k -u deploy:secret -X POST https://<pod-ip>:8443/v1/replan
```

### Identities from a ConfigMap and a Secret

Identities in a layer file survive a restart, so the read identity of a log
shipper can ship with the Pod. The password hash stays in a Secret; the layer
names the file:

```bash
# The hash, not the password, goes into the Secret.
kubectl create secret generic myapp-vigil-identities \
  --from-literal=log-relay.hash="$(openssl passwd -6 'MY-pw1!')"
```

```yaml
# ConfigMap myapp-vigil-layers, file 010-identities.yaml
identities:
  log-relay:
    access: read
    basic:
      password-hash-file: /run/secrets/vigil/log-relay.hash
```

```yaml
spec:
  containers:
    - name: app
      volumeMounts:
        - name: layers
          mountPath: /etc/vigil/layers
        - name: identities
          mountPath: /run/secrets/vigil
          readOnly: true
  volumes:
    - name: layers
      configMap:
        name: myapp-vigil-layers
    - name: identities
      secret:
        secretName: myapp-vigil-identities
```

With `--watch-layers`, vigild also watches the content of
`password-hash-file` and `tls.ca`: after `kubectl create secret …
--dry-run=client -o yaml | kubectl apply -f -` with a new hash, the new
password applies once the kubelet has updated the volume. Removing the
identity from the ConfigMap removes its access with the next replan.

Identities from layer files are read-only over the API and do not end the
Unix-socket bootstrap: root and vigild's own UID stay admin on the socket
(e.g. for `kubectl exec <pod> -- vigil …`). See
[Identities](../user-guide/configuration.md#identities).

---

## Nomad

### Job spec with vigild

```hcl
job "myapp" {
  datacenters = ["dc1"]
  type        = "service"

  group "app" {
    count = 1

    network {
      port "app"    { static = 8080 }
      port "vigild" { static = 8443 }
    }

    task "vigild" {
      driver = "docker"

      config {
        image   = "ghcr.io/your-org/myapp:latest"
        ports   = ["app", "vigild"]
      }

      env {
        VIGIL_TLS_ADDR = "0.0.0.0:8443"
        VIGIL_LAYERS   = "/local/vigil/layers"
      }

      template {
        data        = <<EOF
summary: myapp
services:
  myapp:
    command: /usr/local/bin/myapp
    startup: enabled
    on-success: restart
    on-failure: restart
    on-check-failure:
      myapp-alive: restart
checks:
  myapp-alive:
    level: alive
    startup: enabled
    delay: 5s
    period: 10s
    timeout: 3s
    threshold: 3
    http:
      url: http://localhost:8080/healthz
EOF
        destination = "local/vigil/layers/001-app.yaml"
      }

      service {
        name = "myapp"
        port = "app"

        check {
          type     = "http"
          path     = "/v1/health"
          port     = "vigild"
          protocol = "https"
          tls_skip_verify = true
          interval = "15s"
          timeout  = "3s"
        }
      }

      resources {
        cpu    = 256
        memory = 128
      }
    }
  }
}
```

### Nomad replan on template change

Nomad re-renders templates on change. Let vigild pick the change up itself
with [`--watch-layers`](#replan-on-configmap-change):

```hcl
      env {
        VIGIL_TLS_ADDR     = "0.0.0.0:8443"
        VIGIL_LAYERS       = "/local/vigil/layers"
        VIGIL_WATCH_LAYERS = "10s"
      }
```

Set `change_mode = "noop"` on the `template` block so Nomad does not restart
the task as well.

---

## Docker Compose

```yaml
services:
  app:
    image: ghcr.io/your-org/myapp:latest
    pid: "host"          # optional: share PID namespace for debugging
    ports:
      - "8080:8080"
      - "8443:8443"
    environment:
      VIGIL_TLS_ADDR: "0.0.0.0:8443"
    volumes:
      - ./layers:/etc/vigil/layers:ro
    restart: unless-stopped
    healthcheck:
      # /v1/health is open: 200 while no check is down, 502 otherwise.
      test: ["CMD", "curl", "-fsk", "https://localhost:8443/v1/health"]
      interval: 15s
      timeout: 5s
      retries: 3
      start_period: 10s
```

---

## CI / CD integration

With the TLS listener enabled, you can control vigild from a CI pipeline
without `exec`-ing into the container. The pipeline needs an identity of
access `write` (see [TLS](tls.md#who-may-do-what-over-tls)):

```bash
export VIGIL_URL=https://myhost:8443
export VIGIL_CACERT=/path/to/ca.pem      # or VIGIL_INSECURE=true
export VIGIL_USER=deploy:secret

# Restart a service after a deployment
vigil services restart myapp

# Wait until no check is down (/v1/health is 502 while one is)
until curl -fs --cacert "$VIGIL_CACERT" "$VIGIL_URL/v1/health"; do
  sleep 2
done

# Trigger a hot config reload
vigil replan

# Stream logs during a deployment
vigil logs -f myapp &
LOG_PID=$!
# ... run deployment ...
kill $LOG_PID
```
