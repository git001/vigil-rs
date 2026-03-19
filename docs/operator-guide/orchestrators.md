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

      # Liveness probe via vigild health check API
      livenessProbe:
        httpGet:
          path: /v1/checks
          port: 8443
          scheme: HTTPS
        initialDelaySeconds: 10
        periodSeconds: 15

      volumeMounts:
        - name: vigil-layers
          mountPath: /etc/vigil/layers

  volumes:
    - name: vigil-layers
      configMap:
        name: myapp-vigil-layers
```

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
# Liveness: vigild is responsive
livenessProbe:
  httpGet:
    path: /v1/checks
    port: 8443
    scheme: HTTPS
    insecureSkipTLSVerify: true   # vigild uses a self-signed cert by default
  initialDelaySeconds: 10
  periodSeconds: 30

# Readiness: vigild is responsive and serving requests
readinessProbe:
  httpGet:
    path: /v1/checks
    port: 8443
    scheme: HTTPS
    insecureSkipTLSVerify: true   # vigild uses a self-signed cert by default
  initialDelaySeconds: 5
  periodSeconds: 10
```

Requires `VIGIL_TLS_ADDR: "0.0.0.0:8443"` (or `--tls-addr`) to enable the
HTTPS listener. The `/v1/checks` endpoint returns `200` when vigild is
healthy and reachable.

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
| `WATCH_INTERVAL` | `30` | Seconds between pod-list refreshes |

**OpenShift:** the image uses `WORKDIR /tmp` and `--socket /tmp/vigild.sock`
because OpenShift mounts `/run` as a root-owned tmpfs at runtime. An
`image.openshift.io/triggers` annotation in `k8s/deployment.yaml` rolls the
Deployment automatically on `oc start-build`.

### Replan on ConfigMap change

Trigger a `replan` when the layer ConfigMap is updated:

```bash
# After updating the ConfigMap
kubectl exec <pod> -- vigil replan
```

Or automate with a Kubernetes operator that watches ConfigMaps and calls the
vigild REST API:

```bash
curl -k -X POST https://<pod-ip>:8443/v1/replan
```

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
          path     = "/v1/checks"
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

Nomad re-renders templates on change. You can trigger a replan via a
`sidecar_task` or a lifecycle hook:

```hcl
task "replan-on-change" {
  driver = "exec"
  lifecycle {
    hook    = "poststart"
    sidecar = true
  }
  config {
    command = "/bin/sh"
    args    = ["-c", "inotifywait -m -e close_write /local/vigil/layers/ | while read; do vigil --url https://localhost:8443 --insecure replan; done"]
  }
}
```

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
      test: ["CMD", "vigil", "--url", "https://localhost:8443", "--insecure", "checks"]
      interval: 15s
      timeout: 5s
      retries: 3
      start_period: 10s
```

---

## CI / CD integration

With the TLS listener enabled, you can control vigild from a CI pipeline
without `exec`-ing into the container:

```bash
# Restart a service after a deployment
vigil --url https://myhost:8443 --insecure restart myapp

# Wait until all checks are up
until vigil --url https://myhost:8443 --insecure checks | grep -v down; do
  sleep 2
done

# Trigger a hot config reload
vigil --url https://myhost:8443 --insecure replan

# Stream logs during a deployment
vigil --url https://myhost:8443 --insecure logs -f myapp &
LOG_PID=$!
# ... run deployment ...
kill $LOG_PID
```
