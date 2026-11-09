#!/usr/bin/env bash
# HTTPS through a real forward proxy, which VTest2 cannot be:
# vigil-log-relay and vigil reach vigild's TLS API through squid, once
# through a plain tunnel (CONNECT) and once through a man in the middle that
# re-signs the TLS session with its own CA, like a corporate proxy.
#
# Usage (from the repository root, on a host with podman and openssl):
#   tests/proxy/squid.sh
#
# squid runs in a container on the host network (image from
# tests/proxy/Containerfile, built locally; config tests/proxy/squid.conf).
# vigild (tests/proxy/layers) listens on 127.0.0.1 with a certificate of a
# CA the script makes (ca.crt); squid's man in the middle signs with a
# second CA (mitm-ca.crt).
#
# Environment variables:
#   VIGILD_BIN   vigild binary   (default: target/debug/vigild)
#   VIGIL_BIN    vigil binary    (default: target/debug/vigil)
#   RELAY_BIN    vigil-log-relay (default: target/debug/vigil-log-relay)
#   SQUID_IMAGE  squid image built from tests/proxy/Containerfile
#                (default: localhost/vigil-proxy-test-squid)
#   PODMAN       podman binary   (default: podman)
#
# Checked, through the tunnel (client certificate, identity relay):
#   - vigil-log-relay --source-proxy forwards vigild's log lines
#   - HTTPS_PROXY instead of --source-proxy: the same
#   - vigil --proxy: services list
#   - --source-no-proxy 127.0.0.1: the relay connects directly
# and through the man in the middle (user and password, identity
# relay-basic; a client certificate cannot pass it):
#   - vigil-log-relay --source-proxy-cacert with squid's CA forwards lines
#   - without squid's CA the relay does not trust the connection
#   - vigil --proxy-cacert: services list

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VIGILD_BIN="${VIGILD_BIN:-${REPO_ROOT}/target/debug/vigild}"
VIGIL_BIN="${VIGIL_BIN:-${REPO_ROOT}/target/debug/vigil}"
RELAY_BIN="${RELAY_BIN:-${REPO_ROOT}/target/debug/vigil-log-relay}"
SQUID_IMAGE="${SQUID_IMAGE:-localhost/vigil-proxy-test-squid}"
PODMAN="${PODMAN:-podman}"
RELAY_UNTIL="${REPO_ROOT}/tests/vtest/helpers/relay_until.sh"

for tool in "${PODMAN}" openssl; do
    command -v "${tool}" > /dev/null || { echo "ERROR: ${tool} not found" >&2; exit 1; }
done

# Short paths: the socket path must stay under 108 bytes.
WORK="$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/vigil-proxy.XXXXXX")"
SQUID="vigil-proxy-test-$$"
VIGILD_PID=""
cleanup() {
    if [[ -n "${VIGILD_PID}" ]]; then
        kill "${VIGILD_PID}" 2> /dev/null || true
        wait "${VIGILD_PID}" 2> /dev/null || true
    fi
    "${PODMAN}" rm -f "${SQUID}" > /dev/null 2>&1 || true
    rm -rf "${WORK}"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    echo "--- squid" >&2
    "${PODMAN}" logs "${SQUID}" 2>&1 | tail -20 >&2 || true
    for log in "${WORK}"/*.log; do
        [[ -e "${log}" ]] || continue
        echo "--- ${log##*/}" >&2
        tail -5 "${log}" >&2
    done
    exit 1
}

# A port nobody listens on.
free_port() {
    local p
    for _ in $(seq 1 50); do
        p=$((20000 + RANDOM % 40000))
        if ! (exec 3<> "/dev/tcp/127.0.0.1/${p}") 2> /dev/null; then
            echo "${p}"
            return
        fi
    done
    return 1
}

# Lines in squid's access log that match a regex.
squid_log() { "${PODMAN}" logs "${SQUID}" 2> /dev/null | grep -c -E "$1" || true; }
tunnels() { squid_log "TCP_TUNNEL/200 .* CONNECT 127.0.0.1:${TLS_PORT} "; }
# squid logs a bumped request with its URL (the query cut off).
bumped() { squid_log "/200 .* GET https://127.0.0.1:${TLS_PORT}$1"; }

# make_ca NAME CN: a CA; make_cert NAME EXTENSIONS: a certificate of ca.
make_ca() {
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
        -keyout "$1.key" -out "$1.crt" -days 1 -nodes -subj "/CN=$2" \
        -addext "basicConstraints=critical,CA:true" \
        -addext "keyUsage=critical,keyCertSign,cRLSign" 2> /dev/null
}
make_cert() {
    printf "%b" "$2" > "$1.cnf"
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
        -keyout "$1.key" -out "$1.csr" -nodes -subj "/CN=$1" 2> /dev/null
    openssl x509 -req -in "$1.csr" -CA ca.crt -CAkey ca.key \
        -CAcreateserial -out "$1.crt" -days 1 -extfile "$1.cnf" 2> /dev/null
}

echo "==> Certificates: vigild's CA (server for 127.0.0.1, client), squid's CA"
(
    cd "${WORK}"
    make_ca ca test-ca
    make_cert server "subjectAltName=IP:127.0.0.1\nextendedKeyUsage=serverAuth"
    make_cert client "subjectAltName=DNS:relay\nextendedKeyUsage=clientAuth"
    make_ca mitm-ca test-mitm-ca
)

echo "==> vigild with TLS on 127.0.0.1"
mkdir "${WORK}/layers"
sed "s|@CERT_DIR@|${WORK}|" "${REPO_ROOT}/tests/proxy/layers/001-relay.yaml" \
    > "${WORK}/layers/001-relay.yaml"
"${VIGILD_BIN}" --layers-dir "${WORK}/layers" --socket "${WORK}/vigild.sock" \
    --path-prefix "${WORK}/run" --tls-addr 127.0.0.1:0 \
    --cert "${WORK}/server.crt" --key "${WORK}/server.key" --tls-client-ca "${WORK}/ca.crt" \
    > "${WORK}/vigild.log" 2>&1 < /dev/null &
VIGILD_PID=$!
TLS_PORT=""
for _ in $(seq 1 50); do
    TLS_PORT="$("${VIGIL_BIN}" --socket "${WORK}/vigild.sock" system-info 2> /dev/null \
        | sed -n 's/^https-address: 127.0.0.1:\([0-9]*\)$/\1/p')" || true
    [[ -n "${TLS_PORT}" ]] && break
    sleep 0.1
done
[[ -n "${TLS_PORT}" ]] || fail "vigild did not start"
URL="https://127.0.0.1:${TLS_PORT}/v1/logs/follow?format=ndjson"

echo "==> squid (${SQUID_IMAGE} from tests/proxy/Containerfile)"
"${PODMAN}" build -q -t "${SQUID_IMAGE}" -f "${REPO_ROOT}/tests/proxy/Containerfile" \
    "${REPO_ROOT}/tests/proxy" > /dev/null
TUNNEL_PORT="$(free_port)" || fail "no free port"
BUMP_PORT="$(free_port)" || fail "no free port"
# squid runs as its own user: the files it reads are world-readable.
mkdir -m 755 "${WORK}/squid"
cp "${WORK}/ca.crt" "${WORK}/mitm-ca.crt" "${WORK}/mitm-ca.key" "${WORK}/squid/"
sed -e "s|@TUNNEL_PORT@|${TUNNEL_PORT}|" -e "s|@BUMP_PORT@|${BUMP_PORT}|" \
    "${REPO_ROOT}/tests/proxy/squid.conf" > "${WORK}/squid/squid.conf"
chmod 644 "${WORK}/squid/"*
"${PODMAN}" run -d --name "${SQUID}" --network host \
    -v "${WORK}/squid:/etc/squid/test:ro,Z" "${SQUID_IMAGE}" > /dev/null
for port in "${TUNNEL_PORT}" "${BUMP_PORT}"; do
    for _ in $(seq 1 100); do
        (exec 3<> "/dev/tcp/127.0.0.1/${port}") 2> /dev/null && break
        sleep 0.2
    done
    (exec 3<> "/dev/tcp/127.0.0.1/${port}") 2> /dev/null || fail "squid does not listen on ${port}"
done
TUNNEL="http://127.0.0.1:${TUNNEL_PORT}"
BUMP="http://127.0.0.1:${BUMP_PORT}"

# relay NAME ARGS…: vigil-log-relay on vigild's stream until its first log
# line; stdout in NAME.out, its log in NAME.out.log. One failure ends it.
relay() {
    local name=$1
    shift
    "${RELAY_UNTIL}" "${RELAY_BIN}" "${WORK}/${name}.out" proxy-test-line \
        --source-url "${URL}" --source-reconnect-retries 1 "$@"
}
MTLS=(--source-cacert "${WORK}/ca.crt" --source-cert "${WORK}/client.crt" --source-key "${WORK}/client.key")

echo "==> Tunnel: vigil-log-relay --source-proxy (client certificate)"
relay flag --source-proxy "${TUNNEL}" "${MTLS[@]}" || fail "the relay exited"
grep -q '"service":"log-svc".*proxy-test-line' "${WORK}/flag.out" || fail "no line through --source-proxy"
test "$(tunnels)" -eq 1 || fail "squid logged no tunnel for --source-proxy"

echo "==> Tunnel: vigil-log-relay with HTTPS_PROXY"
HTTPS_PROXY="${TUNNEL}" relay env "${MTLS[@]}" || fail "the relay exited"
grep -q proxy-test-line "${WORK}/env.out" || fail "no line through HTTPS_PROXY"
test "$(tunnels)" -eq 2 || fail "squid logged no tunnel for HTTPS_PROXY"

echo "==> Tunnel: vigil --proxy services list (client certificate)"
"${VIGIL_BIN}" --url "https://127.0.0.1:${TLS_PORT}" --proxy "${TUNNEL}" \
    --cacert "${WORK}/ca.crt" --cert "${WORK}/client.crt" --key "${WORK}/client.key" \
    services list > "${WORK}/vigil-tunnel.log" 2>&1 || fail "vigil services list through the tunnel"
grep -q "^log-svc " "${WORK}/vigil-tunnel.log" || fail "vigil did not list log-svc"
test "$(tunnels)" -eq 3 || fail "squid logged no tunnel for vigil --proxy"

echo "==> vigil-log-relay --source-no-proxy 127.0.0.1: direct"
relay direct --source-proxy "${TUNNEL}" --source-no-proxy 127.0.0.1 "${MTLS[@]}" \
    || fail "the relay exited"
grep -q proxy-test-line "${WORK}/direct.out" || fail "no line with --source-no-proxy"
test "$(tunnels)" -eq 3 || fail "--source-no-proxy still went through squid"

USER_FILE="${REPO_ROOT}/tests/proxy/relay-user"

echo "==> Man in the middle: vigil-log-relay --source-proxy-cacert (user and password)"
relay bump --source-proxy "${BUMP}" --source-proxy-cacert "${WORK}/mitm-ca.crt" \
    --source-user-file "${USER_FILE}" || fail "the relay exited"
grep -q '"service":"log-svc".*proxy-test-line' "${WORK}/bump.out" || fail "no line through the man in the middle"
test "$(bumped "/v1/logs/follow")" -eq 1 || fail "squid did not see the request in clear"

echo "==> Man in the middle: without squid's CA the relay does not connect"
rc=0
relay untrusted --source-proxy "${BUMP}" --source-cacert "${WORK}/ca.crt" \
    --source-user-file "${USER_FILE}" || rc=$?
test "${rc}" -eq 1 || fail "the relay trusted squid's certificate without its CA (exit ${rc})"
test ! -s "${WORK}/untrusted.out" || fail "lines without squid's CA"
grep -q "connection failed" "${WORK}/untrusted.out.log" || fail "no TLS error without squid's CA"

echo "==> Man in the middle: vigil --proxy-cacert services list"
"${VIGIL_BIN}" --url "https://127.0.0.1:${TLS_PORT}" --proxy "${BUMP}" \
    --proxy-cacert "${WORK}/mitm-ca.crt" --user relay-basic:extrapass \
    services list > "${WORK}/vigil-bump.log" 2>&1 || fail "vigil services list through the man in the middle"
grep -q "^log-svc " "${WORK}/vigil-bump.log" || fail "vigil did not list log-svc"
test "$(bumped "/v1/services")" -ge 1 || fail "squid did not see vigil's request in clear"

echo "PASS: HTTPS through squid, tunnel and man in the middle"
