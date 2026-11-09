#!/usr/bin/env bash
# Build the release image for tests/k8s/e2e.sh and push it to a registry
# the cluster can pull from.
#
# Usage (from the repository root, on a host with podman):
#   tests/k8s/build-image.sh        # prints E2E_IMAGE=… at the end
#
# Built as by the release workflow: static musl binaries (compiled in a
# rust:alpine container, with the host's cargo registry as cache), then the
# repository's Dockerfile. By default the image goes to ttl.sh, an anonymous
# public registry whose images expire after the time in the tag (2h).
#
# Environment variables:
#   E2E_IMAGE   Image to build and push
#               (default: ttl.sh/vigil-rs-e2e-<commit>-<random>:2h).
#   RUST_IMAGE  Build image (default: docker.io/library/rust:1.99-alpine).
#   PODMAN      podman binary (default: podman).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PODMAN="${PODMAN:-podman}"
RUST_IMAGE="${RUST_IMAGE:-docker.io/library/rust:1.99-alpine}"
COMMIT="$(git -C "${REPO_ROOT}" rev-parse --short HEAD)"
RANDOM_PART="$(od -An -N4 -tx4 /dev/urandom | tr -d ' ')"
E2E_IMAGE="${E2E_IMAGE:-ttl.sh/vigil-rs-e2e-${COMMIT}-${RANDOM_PART}:2h}"
# Inside the repository, so the build container can write it; ignored by git.
TARGET_DIR=target/e2e-musl

mkdir -p "${HOME}/.cargo/registry"

echo "==> Building static binaries in ${RUST_IMAGE}"
"${PODMAN}" run --rm --security-opt label=disable \
    -v "${REPO_ROOT}:/src" \
    -v "${HOME}/.cargo/registry:/usr/local/cargo/registry" \
    -w /src \
    -e CARGO_TARGET_DIR="/src/${TARGET_DIR}" \
    "${RUST_IMAGE}" \
    sh -c 'apk add --no-cache musl-dev > /dev/null && cargo build --release --locked --bin vigild --bin vigil --bin vigil-log-relay'

CONTEXT="$(mktemp -d)"
trap 'rm -rf "${CONTEXT}"' EXIT
mkdir -p "${CONTEXT}/dist/amd64"
for bin in vigild vigil vigil-log-relay; do
    cp "${REPO_ROOT}/${TARGET_DIR}/release/${bin}" "${CONTEXT}/dist/amd64/"
done
cp "${REPO_ROOT}/Dockerfile" "${CONTEXT}/"

echo "==> Building ${E2E_IMAGE} from the Dockerfile"
"${PODMAN}" build --build-arg TARGETARCH=amd64 -t "${E2E_IMAGE}" "${CONTEXT}"

echo "==> Pushing ${E2E_IMAGE}"
"${PODMAN}" push "${E2E_IMAGE}"

echo "E2E_IMAGE=${E2E_IMAGE}"
