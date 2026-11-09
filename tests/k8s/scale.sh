#!/usr/bin/env bash
# End-to-end test of the release image in a Deployment with several
# replicas: scaling up and down under load.
#
# Usage (from the repository root):
#   KUBECONFIG=~/.kube/test-cluster.yml E2E_IMAGE=… tests/k8s/scale.sh
#
# The Deployment, its Service and a client pod come from
# tests/k8s/manifests/scale.yaml (namespace vigil-scale-test, removed again
# at the end). E2E_IMAGE is built by tests/k8s/build-image.sh.
#
# Environment variables:
#   KUBECONFIG      Cluster to test against (required).
#   E2E_IMAGE       Image built by tests/k8s/build-image.sh (required).
#   KUBECTL         kubectl binary (default: kubectl from PATH).
#   KEEP_NAMESPACE  Set to 1 to keep the namespace for debugging.
#
# Checked:
#   - both replicas become ready; a scheduled oneshot runs in each of them
#   - scaling up to 3: the new pod gets requests only once it is ready,
#     not a single request fails
#   - scaling down to 1: with VIGIL_SHUTDOWN_DELAY the pods leave the
#     Service before their services stop, not a single request fails

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
MANIFEST="${REPO_ROOT}/tests/k8s/manifests/scale.yaml"
NS=vigil-scale-test
DEPLOY=vigil-scale

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
    echo "--- pods" >&2
    k get pods -o wide >&2 || true
    echo "--- failed requests of the client" >&2
    k logs client 2>/dev/null | awk '$2 != "200"' | tail -20 >&2 || true
    exit 1
}

# wait_for <seconds> <description> <command...>: poll every second; the
# command is run anew on every try.
wait_for() {
    local secs=$1 what=$2
    shift 2
    for _ in $(seq 1 "${secs}"); do
        "$@" > /dev/null 2>&1 && return 0
        sleep 1
    done
    fail "timed out after ${secs}s: ${what}"
}

# The client's clock: its log lines carry the node's time.
now() { k exec client -- date +%s; }
pods() { k get pods -l app=vigil-scale -o jsonpath='{.items[*].metadata.name}'; }
pod_count() { test "$(pods | wc -w)" -eq "$1"; }
ticked() { k logs "$1" | grep -q "^\[tick\] tick from $1$"; }
all_ticked() {
    local p
    for p in $(pods); do ticked "${p}" || return 1; done
}
# Requests of the client since unix time $1 that did not get 200.
failures_since() { k logs client | awk -v t="$1" '$1 >= t && $2 != "200"' | wc -l; }
requests_since() { k logs client | awk -v t="$1" '$1 >= t' | wc -l; }

echo "==> Creating the Deployment (2 replicas) in namespace ${NS} (image ${E2E_IMAGE})"
sed "s|VIGIL_E2E_IMAGE|${E2E_IMAGE}|" "${MANIFEST}" | "${KUBECTL}" apply -f - > /dev/null
k rollout status "deployment/${DEPLOY}" --timeout=180s > /dev/null \
    || fail "the Deployment did not become available"
k wait --for=condition=Ready pod/client --timeout=120s > /dev/null || fail "the client is not running"

echo "==> A scheduled oneshot runs in every replica"
wait_for 30 "[tick] in the log of every replica" all_ticked

echo "==> Scaling up to 3 under load: no request fails"
START=$(now)
k scale "deployment/${DEPLOY}" --replicas=3 > /dev/null
k rollout status "deployment/${DEPLOY}" --timeout=180s > /dev/null || fail "the third replica did not become ready"
sleep 5
test "$(requests_since "${START}")" -gt 20 || fail "the client sent too few requests"
FAILED=$(failures_since "${START}")
test "${FAILED}" -eq 0 || fail "${FAILED} requests failed while scaling up"

echo "==> Scaling down to 1 under load: no request fails (VIGIL_SHUTDOWN_DELAY)"
START=$(now)
k scale "deployment/${DEPLOY}" --replicas=1 > /dev/null
wait_for 60 "only one replica left" pod_count 1
sleep 3
test "$(requests_since "${START}")" -gt 20 || fail "the client sent too few requests"
FAILED=$(failures_since "${START}")
test "${FAILED}" -eq 0 || fail "${FAILED} requests failed while scaling down"

echo "PASS: scaling the release image (${E2E_IMAGE})"
