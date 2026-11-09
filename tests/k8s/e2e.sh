#!/usr/bin/env bash
# End-to-end test of the release image in a real cluster: vigild as PID 1
# of a pod that runs as on OpenShift.
#
# Usage (from the repository root):
#   eval "$(tests/k8s/build-image.sh | grep '^E2E_IMAGE=')"   # or set it
#   KUBECONFIG=~/.kube/test-cluster.yml E2E_IMAGE=… tests/k8s/e2e.sh
#
# The pod and its layers come from tests/k8s/manifests/e2e.yaml (namespace
# vigil-e2e-test, removed again at the end).
#
# Environment variables:
#   KUBECONFIG      Cluster to test against (required).
#   E2E_IMAGE       Image built by tests/k8s/build-image.sh (required).
#   KUBECTL         kubectl binary (default: kubectl from PATH).
#   KEEP_NAMESPACE  Set to 1 to keep the namespace for debugging.
#
# Checked:
#   - the pod becomes ready (readiness probe on /v1/health?level=ready)
#   - vigild is PID 1 and reaps an orphaned process
#   - vigil works as the pod's random UID (socket under /tmp/vigil)
#   - kubectl logs shows the services' output; a stack trace with
#     logs-multiline has the [service] prefix on its first line only
#   - a failing ready check makes the pod not ready, and ready again
#   - a changed ConfigMap is applied without kubectl exec (--watch-layers)
#   - a failing alive check makes the kubelet restart the container, and
#     vigild starts again on the same /tmp (stale socket)
#   - deleting the pod stops vigild well within the grace period

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
MANIFEST="${REPO_ROOT}/tests/k8s/manifests/e2e.yaml"
LAYERS_V2="${REPO_ROOT}/tests/k8s/manifests/e2e-layers-v2.yaml"
NS=vigil-e2e-test
POD=vigil-e2e

: "${KUBECONFIG:?set KUBECONFIG to the test cluster}"
: "${E2E_IMAGE:?set E2E_IMAGE, see tests/k8s/build-image.sh}"
command -v "${KUBECTL}" > /dev/null || { echo "ERROR: ${KUBECTL} not found" >&2; exit 1; }

k() { "${KUBECTL}" -n "${NS}" "$@"; }

cleanup() {
    if [[ "${KEEP_NAMESPACE:-0}" != 1 ]]; then
        "${KUBECTL}" delete namespace "${NS}" --wait=false > /dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    echo "--- pod status" >&2
    k get pod "${POD}" -o wide >&2 || true
    echo "--- kubectl logs (last 30 lines)" >&2
    k logs "${POD}" --tail=30 >&2 || true
    exit 1
}

# wait_for <seconds> <description> <command...>: poll every second.
wait_for() {
    local secs=$1 what=$2
    shift 2
    for _ in $(seq 1 "${secs}"); do
        "$@" > /dev/null 2>&1 && return 0
        sleep 1
    done
    fail "timed out after ${secs}s: ${what}"
}

in_pod() { k exec "${POD}" -- "$@"; }
ready() { test "$(k get pod "${POD}" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}')" = "$1"; }
# Functions, so that wait_for asks again on every try.
restarted() { test "$(k get pod "${POD}" -o jsonpath='{.status.containerStatuses[0].restartCount}')" -ge 1; }
service_active() { in_pod vigil services list | grep -Eq "^$1 +enabled +active"; }
zombies() { in_pod sh -c 'for f in /proc/[0-9]*/stat; do cut -d" " -f3 "$f"; done' | grep -c '^Z$' || true; }

echo "==> Creating the pod in namespace ${NS} (image ${E2E_IMAGE})"
sed "s|VIGIL_E2E_IMAGE|${E2E_IMAGE}|" "${MANIFEST}" | "${KUBECTL}" apply -f - > /dev/null
k wait --for=condition=Ready "pod/${POD}" --timeout=180s > /dev/null \
    || fail "the pod did not become ready"

echo "==> vigild is PID 1 and reaps orphans"
test "$(in_pod cat /proc/1/comm)" = vigild || fail "PID 1 is not vigild"
# orphan-maker left "sleep 2" behind, which vigild inherits as PID 1.
sleep 3
test "$(zombies)" -eq 0 || fail "zombie processes: vigild did not reap them"

echo "==> vigil works as the random UID"
test "$(in_pod id -u)" = 1000710000 || fail "the pod does not run as UID 1000710000"
service_active app || fail "vigil services list does not show app active"
in_pod vigil checks list | grep -Eq '^ready-marker +ready +up' \
    || fail "vigil checks list does not show ready-marker up"

echo "==> kubectl logs: service output, a stack trace as one entry"
k logs "${POD}" | grep -q '^\[app\] app line ' || fail "no [app] lines in kubectl logs"
TRACE="$(k logs "${POD}" | grep -A2 '^\[trace\] ERROR e2e request failed$' || true)"
test "$(printf '%s\n' "${TRACE}" | sed -n 2p)" = "java.lang.IllegalStateException: boom" \
    || fail "the second line of the trace is not unprefixed: ${TRACE}"

echo "==> A failing ready check makes the pod not ready"
in_pod rm /tmp/ready
wait_for 30 "pod not ready after /tmp/ready was removed" ready False
in_pod touch /tmp/ready
wait_for 30 "pod ready again" ready True

echo "==> A changed ConfigMap is applied (VIGIL_WATCH_LAYERS)"
"${KUBECTL}" apply -f "${LAYERS_V2}" > /dev/null
# The kubelet updates a mounted ConfigMap after its sync period (about a
# minute or two); vigild applies it within two intervals after that.
wait_for 180 "service extra from the changed ConfigMap" service_active extra
k logs "${POD}" | grep -q '^\[extra\] extra started$' || fail "no [extra] line in kubectl logs"

echo "==> A failing alive check restarts the container"
in_pod rm /tmp/alive
wait_for 60 "a container restart after /tmp/alive was removed" restarted
# The new vigild starts on the same /tmp: its old socket is still there.
k wait --for=condition=Ready "pod/${POD}" --timeout=120s > /dev/null \
    || fail "the pod did not become ready after the restart"
service_active extra || fail "vigil does not work after the restart"

echo "==> Deleting the pod stops vigild within the grace period"
START=$(date +%s)
k delete pod "${POD}" --wait=true > /dev/null
ELAPSED=$(( $(date +%s) - START ))
test "${ELAPSED}" -lt 20 || fail "deleting the pod took ${ELAPSED}s (grace period 30s)"

echo "PASS: vigild in the release image (${E2E_IMAGE})"
