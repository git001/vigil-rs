#!/bin/bash
# Kubernetes pod log collector for vigil-rs
#
# Watches Running pods in NAMESPACE, streams their logs from the Kubernetes API,
# and pushes each line as ndjson to Filebeat over TCP.
#
# Pod list is refreshed every WATCH_INTERVAL seconds:
#   - New pods get a log stream started automatically.
#   - Deleted/restarted pods have their stream stopped and cleaned up.
#   - Crashed streams (e.g. pod evicted mid-stream) are restarted on next cycle.
#
# Environment variables:
#   NAMESPACE        Kubernetes namespace to watch (default: default)
#   POD_SELECTOR     Label selector to filter pods (default: all pods)
#                    Examples: "app=myapp"  "app=api,env=prod"  "tier notin (cache)"
#   WATCH_INTERVAL   Seconds between pod-list refreshes (default: 30)
#   FILEBEAT_HOST    Filebeat TCP host (default: 127.0.0.1)
#   FILEBEAT_PORT    Filebeat TCP port (default: 5170)
#
# Requires: bash, curl, jq, socat
# Auth: in-cluster service account at /var/run/secrets/kubernetes.io/serviceaccount/

set -uo pipefail

NAMESPACE="${NAMESPACE:-default}"
POD_SELECTOR="${POD_SELECTOR:-}"
FILEBEAT_HOST="${FILEBEAT_HOST:-127.0.0.1}"
FILEBEAT_PORT="${FILEBEAT_PORT:-5170}"
WATCH_INTERVAL="${WATCH_INTERVAL:-30}"

export CA=/var/run/secrets/kubernetes.io/serviceaccount/ca.crt
export TOKEN_FILE=/var/run/secrets/kubernetes.io/serviceaccount/token

# vigild prefixes output with [pod-log-collector] automatically.
log() { printf '%s\n' "$*"; }

# Preflight: verify in-cluster environment before referencing API host vars
if [[ ! -f "$TOKEN_FILE" ]] || [[ -z "${KUBERNETES_SERVICE_HOST:-}" ]]; then
    log "ERROR: not running inside a Kubernetes cluster"
    log "  expected: service account token at $TOKEN_FILE"
    log "  expected: KUBERNETES_SERVICE_HOST env var (injected by Kubernetes)"
    log "Deploy with k8s/deployment.yaml and k8s/rbac.yaml."
    exit 1
fi

export API="https://${KUBERNETES_SERVICE_HOST}:${KUBERNETES_SERVICE_PORT}"

PID_DIR=$(mktemp -d /tmp/k8s-log-pids.XXXXXX)
trap 'kill $(cat "$PID_DIR"/*.pid 2>/dev/null) 2>/dev/null; rm -rf "$PID_DIR"' EXIT

# List names of Running pods in the namespace, optionally filtered by POD_SELECTOR.
# The labelSelector query parameter is URL-encoded via jq's @uri filter.
list_pods() {
    local url="$API/api/v1/namespaces/$NAMESPACE/pods"
    if [[ -n "$POD_SELECTOR" ]]; then
        local encoded
        encoded=$(printf '%s' "$POD_SELECTOR" | jq -sRr @uri)
        url="${url}?labelSelector=${encoded}"
    fi
    curl -sS --cacert "$CA" \
        -H "Authorization: Bearer $(cat "$TOKEN_FILE")" \
        "$url" \
    | jq -r '.items[] | select(.status.phase == "Running") | .metadata.name'
}

# Stream one pod's logs and push as ndjson to Filebeat.
# Runs as a background job; exits when the pod is deleted or the connection drops.
stream_pod() {
    local pod="$1"
    log "start  pod=$pod ns=$NAMESPACE"

    curl -sS --cacert "$CA" \
        -H "Authorization: Bearer $(cat "$TOKEN_FILE")" \
        "$API/api/v1/namespaces/$NAMESPACE/pods/$pod/log?follow=true&timestamps=true" \
    | while IFS= read -r line; do
        # Kubernetes timestamped lines: "<RFC3339Nano> <message>"
        local ts="${line%% *}"
        local msg="${line#* }"
        jq -cn \
            --arg ts  "$ts" \
            --arg ns  "$NAMESPACE" \
            --arg pod "$pod" \
            --arg msg "$msg" \
            '{timestamp:$ts, namespace:$ns, pod:$pod, stream:"stdout", message:$msg}' \
        || break  # socat gone (SIGPIPE) — exit loop so stream_pod can be restarted
    done | socat - TCP:"$FILEBEAT_HOST":"$FILEBEAT_PORT"

    log "end    pod=$pod"
}

is_alive() {
    local pid_file="$PID_DIR/$1.pid"
    [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null
}

start_stream() {
    local pod="$1"
    is_alive "$pod" && return
    stream_pod "$pod" &
    echo $! > "$PID_DIR/$pod.pid"
}

stop_stream() {
    local pod="$1"
    local pid_file="$PID_DIR/$pod.pid"
    [[ -f "$pid_file" ]] || return
    kill "$(cat "$pid_file")" 2>/dev/null || true
    rm -f "$pid_file"
    log "stop   pod=$pod"
}

log "starting namespace=$NAMESPACE selector=${POD_SELECTOR:-(all pods)} interval=${WATCH_INTERVAL}s filebeat=$FILEBEAT_HOST:$FILEBEAT_PORT"

while true; do
    mapfile -t current < <(list_pods 2>/dev/null || true)
    mapfile -t running < <(ls "$PID_DIR" 2>/dev/null | sed 's/\.pid$//' || true)

    # Start streams for new pods (or streams that died and need restarting)
    for pod in "${current[@]:-}"; do
        [[ -n "$pod" ]] && start_stream "$pod"
    done

    # Stop streams for pods that are gone
    for pod in "${running[@]:-}"; do
        if [[ -n "$pod" ]] && ! printf '%s\n' "${current[@]:-}" | grep -qx "$pod"; then
            stop_stream "$pod"
        fi
    done

    # Heartbeat: updated every cycle so the alive check can detect a stuck loop.
    touch /tmp/pod-log-collector.heartbeat

    sleep "$WATCH_INTERVAL"
done
