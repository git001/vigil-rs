#!/usr/bin/env bash
# End-to-end test of vigil-log-relay --kubernetes against a real cluster.
#
# Usage (from the repository root):
#   KUBECONFIG=~/.kube/test-cluster.yml tests/k8s/relay.sh
#
# The relay runs locally and reaches the API server through KUBECONFIG; the
# test pods come from tests/k8s/manifests/relay-test.yaml (namespace
# vigil-relay-test, removed again at the end).
#
# Environment variables:
#   KUBECONFIG      Cluster to test against (required).
#   KUBECTL         kubectl binary (default: kubectl from PATH).
#   RELAY_BIN       vigil-log-relay binary (default: target/debug/vigil-log-relay).
#   KEEP_NAMESPACE  Set to 1 to keep the namespace for debugging.
#
# Checked:
#   - --pod-selector and --exclude-pod pick the right pods
#   - --container all streams both containers of a pod
#   - stream=Stdout/Stderr, or the fallback to one combined stream when the
#     API server rejects the parameter (feature gate off)
#   - --multiline java forwards a stack trace as one record
#   - a deleted and recreated pod is streamed again
#   - the healthcheck endpoint answers

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
RELAY_BIN="${RELAY_BIN:-${REPO_ROOT}/target/debug/vigil-log-relay}"
MANIFEST="${REPO_ROOT}/tests/k8s/manifests/relay-test.yaml"
NS=vigil-relay-test

: "${KUBECONFIG:?set KUBECONFIG to the test cluster}"
command -v "${KUBECTL}" > /dev/null || { echo "ERROR: ${KUBECTL} not found" >&2; exit 1; }
test -x "${RELAY_BIN}" || { echo "ERROR: ${RELAY_BIN} not found; run: cargo build -p vigil-log-relay" >&2; exit 1; }

WORK="$(mktemp -d)"
RELAY_PID=""
SINK_PID=""

cleanup() {
    [[ -n "${RELAY_PID}" ]] && kill "${RELAY_PID}" 2>/dev/null && wait "${RELAY_PID}" 2>/dev/null || true
    [[ -n "${SINK_PID}" ]] && kill "${SINK_PID}" 2>/dev/null || true
    if [[ "${KEEP_NAMESPACE:-0}" != 1 ]]; then
        "${KUBECTL}" delete namespace "${NS}" --wait=false > /dev/null 2>&1 || true
    fi
    rm -rf "${WORK}"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    echo "--- relay log (last 20 lines)" >&2
    tail -20 "${WORK}/relay.log" >&2 || true
    echo "--- sink output of logger-trace (last 8 records)" >&2
    grep '"pod":"logger-trace"' "${WORK}/out.ndjson" 2>/dev/null | tail -8 >&2 || true
    exit 1
}

# wait_for <seconds> <description> <command...>: poll every 0.5s.
wait_for() {
    local secs=$1 what=$2
    shift 2
    for _ in $(seq 1 $((secs * 2))); do
        "$@" && return 0
        sleep 0.5
    done
    fail "timed out after ${secs}s: ${what}"
}

# Lines from <pod>/<container> in the sink output, from byte offset <from>.
lines_of() {
    tail -c +"$(($3 + 1))" "${WORK}/out.ndjson" 2>/dev/null \
        | grep -c "\"pod\":\"$1\",\"container\":\"$2\"" || true
}
has_lines() { test "$(lines_of "$1" "$2" "${3:-0}")" -ge 2; }

echo "==> Creating test pods in namespace ${NS}"
"${KUBECTL}" apply -f "${MANIFEST}" > /dev/null
"${KUBECTL}" -n "${NS}" wait --for=condition=Ready pod --all --timeout=180s > /dev/null

echo "==> Starting TCP sink and relay"
"${REPO_ROOT}/tests/vtest/helpers/tcp_capture.py" "${WORK}/port" "${WORK}/out.ndjson" \
    > /dev/null 2>&1 < /dev/null &
SINK_PID=$!
wait_for 10 "TCP sink listening" test -s "${WORK}/port"

"${RELAY_BIN}" \
    --kubernetes \
    --namespace "${NS}" \
    --pod-selector app=vigil-relay-test \
    --container all \
    --exclude-pod '^excluded-' \
    --multiline java \
    --watch-interval 1 \
    --since-seconds 2 \
    --tcp-sink-host 127.0.0.1 \
    --tcp-sink-port "$(cat "${WORK}/port")" \
    --healthcheck 127.0.0.1:0 \
    --log-format json \
    > "${WORK}/relay.log" 2>&1 < /dev/null &
RELAY_PID=$!

echo "==> Selector, --exclude-pod and --container all"
wait_for 30 "lines from logger-a/app" has_lines logger-a app
wait_for 30 "lines from logger-multi/app" has_lines logger-multi app
wait_for 30 "lines from logger-multi/sidecar" has_lines logger-multi sidecar
! grep -q '"pod":"excluded-pod"' "${WORK}/out.ndjson" || fail "excluded-pod was streamed"
! grep -q '"pod":"other-app"' "${WORK}/out.ndjson" || fail "other-app does not match the selector"

echo "==> stdout/stderr separation"
if grep -q 'rejects the `stream` parameter' "${WORK}/relay.log"; then
    echo "    API server rejects stream=; relay fell back to the combined stream"
    grep -q '"container":"sidecar","stream":"output"' "${WORK}/out.ndjson" \
        || fail "fallback stream is not labelled output"
else
    echo "    API server accepts stream="
    grep -q '"container":"sidecar","stream":"stderr"' "${WORK}/out.ndjson" \
        || fail "sidecar lines are not labelled stderr"
fi

echo "==> --multiline java: a stack trace is one record"
TRACE='"pod":"logger-trace".*"message":"ERROR request [0-9]+ failed\\njava\.lang\.IllegalStateException: boom\\n\\tat com\.example\.A\.b\(A\.java:1\)\\n\\tat com\.example\.C\.d\(C\.java:2\)"'
# A function, so that wait_for counts again on every try.
has_traces() { test "$(grep -cE "${TRACE}" "${WORK}/out.ndjson" || true)" -ge 2; }
wait_for 30 "two joined stack traces from logger-trace" has_traces
# The stream may start in the middle of a trace; from the first joined one
# on, no line of a trace may come as a record of its own.
FIRST=$(grep -bE "${TRACE}" "${WORK}/out.ndjson" | head -1 | cut -d: -f1)
! tail -c +"$((FIRST + 1))" "${WORK}/out.ndjson" \
    | grep -qE '"pod":"logger-trace".*"message":"(\\tat|java\.lang)' \
    || fail "a line of a stack trace was forwarded as a record of its own"

echo "==> Healthcheck"
HPORT=$(sed -n 's/.*healthcheck listening.*"addr":"[^"]*:\([0-9]*\)".*/\1/p' "${WORK}/relay.log" | head -1)
curl -sf "http://127.0.0.1:${HPORT}/healthz" > /dev/null || fail "healthcheck on port ${HPORT}"

echo "==> Deleted and recreated pod is streamed again"
"${KUBECTL}" -n "${NS}" delete pod logger-a --wait=true > /dev/null
OFFSET=$(stat -c %s "${WORK}/out.ndjson")
"${KUBECTL}" apply -f "${MANIFEST}" > /dev/null
"${KUBECTL}" -n "${NS}" wait --for=condition=Ready pod/logger-a --timeout=180s > /dev/null
wait_for 30 "lines from the new logger-a" has_lines logger-a app "${OFFSET}"

echo "==> Stopping the relay"
kill "${RELAY_PID}"
wait "${RELAY_PID}" || true
RELAY_PID=""
grep -q 'shutdown complete' "${WORK}/relay.log" || fail "relay did not shut down cleanly"

echo "PASS: vigil-log-relay --kubernetes ($(grep -c . "${WORK}/out.ndjson") lines forwarded)"
