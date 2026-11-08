# vigil-rs Documentation

vigil-rs is a service supervisor and container init daemon written in Rust.

## Contents

### User Guide

| Document | Description |
|---|---|
| [Installation](user-guide/installation.md) | Build from source, binary install |
| [Configuration](user-guide/configuration.md) | Full YAML layer reference |
| [CLI Reference](user-guide/cli-reference.md) | `vigil` command-line client and `vigil-log-relay` |
| [Logging](user-guide/logging.md) | Service output, log API, push to collectors, `vigil-log-relay` |

### Operator Guide

| Document | Description |
|---|---|
| [Container Setup](operator-guide/container-setup.md) | Dockerfile, entrypoint, directory layout |
| [TLS](operator-guide/tls.md) | HTTPS API listener, certificates |
| [Metrics](operator-guide/metrics.md) | Prometheus metrics, PromQL examples |
| [Orchestrators](operator-guide/orchestrators.md) | Kubernetes, Nomad integration |

### API Reference

| Document | Description |
|---|---|
| [REST API](api-reference/rest-api.md) | All HTTP endpoints with examples |

### Specs

- [openapi.yaml](specs/openapi.yaml) — machine-readable OpenAPI 3.1 spec,
  generated from vigild (the same as `/openapi.json` on a running daemon)
- Swagger UI available at `/docs` on a running daemon
- [plan-security-and-features.md](specs/plan-security-and-features.md) — the
  v4 plan: security fixes and new features, with their design decisions

### Diagrams

Graphviz sources, rendered to `images/*.svg` by CI:

- [service-lifecycle.dot](user-guide/service-lifecycle.dot) — service states and transitions
- [service-dependencies.dot](user-guide/service-dependencies.dot) — `after`, `before`, `requires`
- [check-levels.dot](user-guide/check-levels.dot) — which checks `/v1/health` counts for which Kubernetes probe

---

## Quick orientation

```
vigild           Daemon binary. Runs as PID 1, reads YAML layers, supervises services.
vigil            CLI client. Connects to vigild via the Unix socket or HTTPS.
vigil-log-relay  Forwards log streams (vigild, Kubernetes pods) to a TCP sink.
```

```bash
# Start the daemon (runtime files under /tmp/vigil, e.g. on OpenShift)
vigild --layers-dir /etc/vigil/layers --path-prefix /tmp/vigil

# Use the CLI (inside the container; same prefix)
export VIGIL_PATH_PREFIX=/tmp/vigil
vigil services list
vigil logs -f

# Use the CLI (remote via HTTPS, with an identity)
vigil --url https://host:8443 --insecure --user deploy:secret services list
```
