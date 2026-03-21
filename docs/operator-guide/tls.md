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

# With a client certificate (mTLS — see section below)
curl --insecure --cert client.crt --key client.key \
     https://myhost:8443/v1/services
```

## Mutual TLS (mTLS)

mTLS lets vigild authenticate callers by verifying the TLS client certificate
they present during the handshake.  It is **optional** at the transport level —
connections without a client certificate are accepted but treated as
unauthenticated (Open access) unless another auth method matches.

### How it works

1. **vigild is started with `--tls-client-ca`** — this loads a CA certificate
   and configures the TLS listener to request (but not require) a client cert.
2. **A TLS identity is registered** via `POST /v1/identities` — it stores the
   CA's PEM and the access level to grant when a cert signed by that CA is
   presented.
3. **At request time:** if a client cert is present, vigild verifies it
   cryptographically against every registered TLS identity.  The most-permissive
   matching level is used.  No match → Open (fallback).

### Step 1 — Start vigild with `--tls-client-ca`

```bash
vigild \
  --layers-dir /etc/vigil/layers \
  --socket     /run/vigil/vigild.sock \
  --tls-addr   0.0.0.0:8443 \
  --tls-client-ca /etc/vigil/certs/client-ca.pem
```

Or via environment variable:

```bash
VIGIL_TLS_ADDR=0.0.0.0:8443 \
VIGIL_TLS_CLIENT_CA=/etc/vigil/certs/client-ca.pem \
vigild --layers-dir /etc/vigil/layers
```

`--tls-client-ca` accepts a PEM file with one or more CA certificates
(concatenated chain).

### Step 2 — Register a TLS identity

Add an identity whose `tls.ca-cert` matches the CA that signed the client
certificate.  This can be done via the Unix socket (no auth required during
bootstrap, or with an existing admin identity):

```bash
curl --unix-socket /run/vigil/vigild.sock \
  -X POST http://localhost/v1/identities \
  -H "Content-Type: application/json" \
  -d '{
    "identities": {
      "ci-pipeline": {
        "access": "write",
        "tls": {
          "ca-cert": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----\n"
        }
      }
    }
  }'
```

Or embed the CA cert inline in the JSON by reading the file:

```bash
CA=$(python3 -c "import json,sys; print(json.dumps(open('/etc/vigil/certs/client-ca.pem').read()))")
curl --unix-socket /run/vigil/vigild.sock \
  -X POST http://localhost/v1/identities \
  -H "Content-Type: application/json" \
  -d "{\"identities\":{\"ci-pipeline\":{\"access\":\"write\",\"tls\":{\"ca-cert\":$CA}}}}"
```

### Step 3 — Connect with a client certificate

```bash
# vigil CLI — present client cert
vigil --url https://myhost:8443 --insecure \
      --cert /path/to/client.crt \
      --key  /path/to/client.key \
      services list

# curl — present client cert signed by the registered CA
curl --insecure \
     --cert /path/to/client.crt \
     --key  /path/to/client.key \
     https://myhost:8443/v1/services

# Without client cert → Open access → 403 on protected endpoints
vigil --url https://myhost:8443 --insecure services list
curl --insecure https://myhost:8443/v1/services
```

### Certificate requirements

Client certificates must satisfy the requirements of `rustls`/`webpki`:

- The CA cert must have `basicConstraints=CA:true` and
  `keyUsage=keyCertSign,cRLSign`.
- The client cert must have `extendedKeyUsage=clientAuth` and a
  `subjectAltName` extension.

Example with `openssl`:

```bash
# CA key + self-signed cert
openssl req -x509 \
  -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
  -keyout ca.key -out ca.crt -days 365 -nodes \
  -subj "/CN=my-ca" \
  -addext "basicConstraints=critical,CA:true" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

# Client key + CSR
openssl req \
  -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
  -keyout client.key -out client.csr -nodes \
  -subj "/CN=my-client"

# Sign client cert with required extensions
printf "subjectAltName=DNS:my-client\nextendedKeyUsage=clientAuth" \
  > client_ext.cnf
openssl x509 -req \
  -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial \
  -out client.crt -days 365 -extfile client_ext.cnf
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
