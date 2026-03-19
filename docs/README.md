# vigil-rs Documentation

vigil-rs is a service supervisor and container init daemon written in Rust.

## Contents

### User Guide

| Document | Description |
|---|---|
| [Installation](user-guide/installation.md) | Build from source, binary install |
| [Configuration](user-guide/configuration.md) | Full YAML layer reference |
| [CLI Reference](user-guide/cli-reference.md) | `vigil` command-line client |

### Operator Guide

| Document | Description |
|---|---|
| [Container Setup](operator-guide/container-setup.md) | Dockerfile, entrypoint, directory layout |
| [TLS](operator-guide/tls.md) | HTTPS API listener, certificates |
| [Orchestrators](operator-guide/orchestrators.md) | Kubernetes, Nomad integration |

### API Reference

| Document | Description |
|---|---|
| [REST API](api-reference/rest-api.md) | All HTTP endpoints with examples |

### Specs

- [openapi.yaml](specs/openapi.yaml) — machine-readable OpenAPI 3.0 spec
- Swagger UI available at `/docs` on a running daemon

---

## Quick orientation

```
vigild   Daemon binary. Runs as PID 1, reads YAML layers, supervises services.
vigil    CLI client. Connects to vigild via Unix socket or HTTP/HTTPS URL.
```

```bash
# Start the daemon
vigild --layers-dir /etc/vigil/layers --socket /run/vigil/vigild.sock

# Use the CLI (inside the container)
vigil services
vigil logs -f

# Use the CLI (remote via HTTPS)
vigil --url https://host:8443 --insecure services
```
