# TLS Configuration

vigild can optionally expose its HTTP API over TLS in addition to the Unix
socket. This is useful for accessing the API from outside the container, from
CI pipelines, or from container orchestrators.

## Enabling the TLS listener

```bash
vigild \
  --layers-dir /etc/vigil/layers \
  --socket     /run/vigil/vigild.sock \
  --tls-addr   0.0.0.0:8443
```

Or via environment variable:

```bash
VIGIL_TLS_ADDR=0.0.0.0:8443 vigild ...
```

In a Dockerfile:

```dockerfile
EXPOSE 8443
ENTRYPOINT ["/usr/local/bin/vigild", \
    "--layers-dir", "/etc/vigil/layers", \
    "--socket",     "/run/vigil/vigild.sock", \
    "--tls-addr",   "0.0.0.0:8443"]
```

## Auto-generated self-signed certificate

When `--tls-addr` is set without `--cert` / `--key`, vigild generates a
self-signed certificate automatically at startup:

- RSA 2048-bit key (generated fresh each run)
- Subject: `CN=localhost` (or the hostname part of `--tls-addr`)
- Valid for 365 days from startup

The certificate is held in memory only — it is not written to disk.

Connect with certificate verification disabled:

```bash
# vigil CLI
vigil --url https://myhost:8443 --insecure services

# curl
curl -k https://myhost:8443/v1/services
```

## Using a custom certificate

Provide your own certificate (e.g. from Let's Encrypt, an internal CA, or
a self-signed cert with a known fingerprint):

```bash
vigild \
  --tls-addr 0.0.0.0:8443 \
  --cert /etc/vigil/tls/cert.pem \
  --key  /etc/vigil/tls/key.pem
```

### Certificate chains

`--cert` accepts a PEM file containing the full certificate chain (leaf +
intermediate CAs). All PEM blocks are loaded in order:

```pem
-----BEGIN CERTIFICATE-----
(leaf certificate)
-----END CERTIFICATE-----
-----BEGIN CERTIFICATE-----
(intermediate CA)
-----END CERTIFICATE-----
```

### Mounting certificates in containers

```dockerfile
# In your Dockerfile or compose file
COPY tls/cert.pem /etc/vigil/tls/cert.pem
COPY tls/key.pem  /etc/vigil/tls/key.pem
```

Or mount at runtime:

```bash
podman run \
  -v /path/to/cert.pem:/etc/vigil/tls/cert.pem:ro \
  -v /path/to/key.pem:/etc/vigil/tls/key.pem:ro \
  -e VIGIL_TLS_ADDR=0.0.0.0:8443 \
  -e VIGIL_CERT=/etc/vigil/tls/cert.pem \
  -e VIGIL_KEY=/etc/vigil/tls/key.pem \
  myimage
```

## Connecting with the vigil CLI

```bash
# With a valid/trusted certificate
vigil --url https://myhost:8443 services

# With a self-signed or untrusted certificate
vigil --url https://myhost:8443 --insecure services

# Via environment
export VIGIL_URL=https://myhost:8443
vigil --insecure logs -f
```

## Connecting with curl

```bash
# Self-signed (skip verification)
curl -k https://myhost:8443/v1/services

# With a custom CA
curl --cacert /path/to/ca.pem https://myhost:8443/v1/services

# With a client certificate (mTLS — if you implement auth middleware)
curl --cacert ca.pem --cert client.pem --key client.key \
     https://myhost:8443/v1/services
```

## Both transports active simultaneously

Unix socket and TLS listener run in parallel. You can connect via either:

```bash
# From inside the container (Unix socket)
vigil services

# From outside (HTTPS)
vigil --url https://myhost:8443 --insecure services

# Via curl (Unix socket)
curl --unix-socket /run/vigil/vigild.sock http://localhost/v1/services

# Via curl (HTTPS)
curl -k https://myhost:8443/v1/services
```

## Swagger UI over TLS

The Swagger UI at `/docs` and OpenAPI spec at `/openapi.json` are available
on both the Unix socket and the TLS listener:

```
https://myhost:8443/docs
https://myhost:8443/openapi.json
```
