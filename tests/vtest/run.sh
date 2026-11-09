#!/usr/bin/env bash
# Run vigild VTest2 integration tests, optionally collecting code coverage.
#
# Usage (from the repository root):
#   tests/vtest/run.sh                     # run VTest2 tests only
#   tests/vtest/run.sh --coverage          # build instrumented binary, run all
#                                          # tests (unit + Rust integration +
#                                          # VTest2), print combined coverage
#   tests/vtest/run.sh --coverage --html   # same but open an HTML report
#   tests/vtest/run.sh -v                  # verbose vtest output
#   tests/vtest/run.sh tests/vtest/v0001_system_info.vtc   # single test file
#
# Environment variables:
#   VTEST       Path to the vtest binary   (default: /datadisk/git-repos/VTest2/vtest)
#   VIGILD_BIN  Path to a pre-built vigild (default: target/debug/vigild)
#               Ignored in --coverage mode; the instrumented binary is used,
#               built in target/llvm-cov-target/ (target/debug/ is left alone).
#   CARGO_TEST_ARGS  Extra arguments for `cargo test` in --coverage mode,
#               split on whitespace. Example for an unprivileged sandbox
#               whose /etc/passwd lacks root:
#               CARGO_TEST_ARGS="-- --skip resolve_uid_name_and --skip resolve_gid_name_and"
#   K8S_TEST    Set to 1 in --coverage mode to also run tests/k8s/relay.sh
#               (vigil-log-relay --kubernetes) against the cluster in KUBECONFIG,
#               and tests/k8s/e2e.sh (the release image in a pod) when
#               E2E_IMAGE is set (see tests/k8s/build-image.sh).

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VTEST="${VTEST:-/datadisk/git-repos/VTest2/vtest}"
VIGIL_BIN="${VIGIL_BIN:-${REPO_ROOT}/target/debug/vigil}"
VIGIL_LOG_RELAY_BIN="${VIGIL_LOG_RELAY_BIN:-${REPO_ROOT}/target/debug/vigil-log-relay}"

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
COVERAGE=false
HTML=false
VTC_FILES=()
VTEST_OPTS=()

for arg in "$@"; do
    case "${arg}" in
        --coverage) COVERAGE=true ;;
        --html)     HTML=true ;;
        *.vtc)      VTC_FILES+=("${arg}") ;;
        *)          VTEST_OPTS+=("${arg}") ;;
    esac
done

if [[ ${#VTC_FILES[@]} -eq 0 ]]; then
    VTC_FILES=("${REPO_ROOT}/tests/vtest/"v*.vtc)
fi

# ---------------------------------------------------------------------------
# Sanity checks
# ---------------------------------------------------------------------------
if [[ ! -x "${VTEST}" ]]; then
    echo "ERROR: vtest not found at ${VTEST}" >&2
    echo "       Set VTEST=/path/to/vtest or install VTest2." >&2
    exit 1
fi

cd "${REPO_ROOT}"

# vigild's runtime directory (--path-prefix). Its default /run/vigil can only
# be created as root or where the user owns /run (e.g. a Flatpak sandbox), so
# the tests get their own, removed at the end. vigild and vigil inherit it.
if [[ -z "${VIGIL_PATH_PREFIX:-}" ]]; then
    VTEST_RUNTIME="$(mktemp -d)"
    trap 'rm -rf "${VTEST_RUNTIME}"' EXIT
    export VIGIL_PATH_PREFIX="${VTEST_RUNTIME}/vigil"
fi

# VTest2 keeps each test's log in a buffer of 1 MiB by default. A failing
# test dumps the 200x400 screen of its processes into it, and the load test
# (v0034) logs over a thousand requests; when the buffer is full VTest2
# aborts instead of reporting. 64 MiB, mapped per test, are enough.
VTEST_BUFFER=(-b 64M)

# ---------------------------------------------------------------------------
# Normal mode — just run VTest2
# ---------------------------------------------------------------------------
if [[ "${COVERAGE}" == false ]]; then
    VIGILD_BIN="${VIGILD_BIN:-${REPO_ROOT}/target/debug/vigild}"
    if [[ ! -x "${VIGILD_BIN}" ]]; then
        echo "ERROR: vigild binary not found at ${VIGILD_BIN}" >&2
        echo "       Run: cargo build -p vigild" >&2
        exit 1
    fi
    if [[ ! -x "${VIGIL_BIN}" ]]; then
        echo "ERROR: vigil binary not found at ${VIGIL_BIN}" >&2
        echo "       Run: cargo build -p vigil" >&2
        exit 1
    fi
    if [[ ! -x "${VIGIL_LOG_RELAY_BIN}" ]]; then
        echo "ERROR: vigil-log-relay binary not found at ${VIGIL_LOG_RELAY_BIN}" >&2
        echo "       Run: cargo build -p vigil-log-relay" >&2
        exit 1
    fi

    "${VTEST}" "${VTEST_BUFFER[@]}" \
        -Dvigild="${VIGILD_BIN}" \
        -Dvigil="${VIGIL_BIN}" \
        -Dvigil_log_relay="${VIGIL_LOG_RELAY_BIN}" \
        -Dvtestdir="${REPO_ROOT}/tests/vtest" \
        "${VTEST_OPTS[@]}" \
        "${VTC_FILES[@]}"
    exit
fi

# ---------------------------------------------------------------------------
# Coverage mode
# ---------------------------------------------------------------------------
if ! command -v cargo-llvm-cov &>/dev/null && ! cargo llvm-cov --version &>/dev/null 2>&1; then
    echo "ERROR: cargo-llvm-cov not installed." >&2
    echo "       Run: cargo install cargo-llvm-cov --locked" >&2
    exit 1
fi

echo "==> Loading coverage environment…"
# Instrumented builds get their own target directory: built in target/,
# they would replace the normal binaries in target/debug/ (and `cargo
# llvm-cov clean` remove them), and every switch between the two would
# rebuild all dependencies because the compiler flags differ. show-env
# derives CARGO_LLVM_COV_TARGET_DIR and LLVM_PROFILE_FILE from this.
export CARGO_TARGET_DIR="${REPO_ROOT}/target/llvm-cov-target"
# Export the llvm-cov env vars into this shell so that all subsequent
# cargo invocations produce instrumented binaries and write profraw files.
eval "$(cargo llvm-cov show-env --sh 2>/dev/null)"
# CARGO_LLVM_COV_SHOW_ENV=1 tells the rustc wrapper we're in "show-env
# mode" and suppresses actual instrumentation — unset it so that cargo
# build / cargo test produce real profraw files.
unset CARGO_LLVM_COV_SHOW_ENV

echo "==> Cleaning previous coverage data…"
# Clean must run *after* loading the env vars so that cargo-llvm-cov
# knows which target directory and which profraw files to remove.
cargo llvm-cov clean --workspace 2>/dev/null || true

echo "==> Building instrumented vigild, vigil, and vigil-log-relay binaries…"
cargo build -p vigild -p vigil -p vigil-log-relay

# The instrumented binaries, apart from the normal build in target/debug/.
VIGILD_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigild"
VIGIL_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigil"
VIGIL_LOG_RELAY_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigil-log-relay"

echo "==> Running unit + Rust integration tests…"
# With the show-env vars active, cargo test writes profraw files into
# CARGO_LLVM_COV_TARGET_DIR. On success only the summary is shown; on a
# failure the whole output, so that the failing tests' output is visible.
read -ra CARGO_TEST_EXTRA <<< "${CARGO_TEST_ARGS:-}"
CARGO_TEST_LOG="${CARGO_LLVM_COV_TARGET_DIR}/cargo-test.log"
if ! cargo test --workspace "${CARGO_TEST_EXTRA[@]}" > "${CARGO_TEST_LOG}" 2>&1; then
    cat "${CARGO_TEST_LOG}"
    echo "ERROR: cargo test failed (output above, also in ${CARGO_TEST_LOG})" >&2
    exit 1
fi
awk '/^test result:/ { p += $4; i += $8 } END { print "    " p " passed, " i " ignored" }' "${CARGO_TEST_LOG}"

echo "==> Running VTest2 tests…"
# LLVM_PROFILE_FILE is set by show-env.  Explicitly re-export it so that
# every subprocess spawned by VTest2 (vigild, vigil, vigil-log-relay) writes
# its profraw into the same target directory and is picked up by the report.
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE}"
"${VTEST}" "${VTEST_BUFFER[@]}" \
    -Dvigild="${VIGILD_BIN}" \
    -Dvigil="${VIGIL_BIN}" \
    -Dvigil_log_relay="${VIGIL_LOG_RELAY_BIN}" \
    -Dvtestdir="${REPO_ROOT}/tests/vtest" \
    "${VTEST_OPTS[@]}" \
    "${VTC_FILES[@]}"

if [[ "${K8S_TEST:-0}" == 1 ]]; then
    echo "==> Running the Kubernetes relay test…"
    RELAY_BIN="${VIGIL_LOG_RELAY_BIN}" "${REPO_ROOT}/tests/k8s/relay.sh"
    # The image runs its own (release) binaries: no coverage from this test.
    if [[ -n "${E2E_IMAGE:-}" ]]; then
        echo "==> Running the Kubernetes end-to-end test…"
        "${REPO_ROOT}/tests/k8s/e2e.sh"
        echo "==> Running the Kubernetes scaling test…"
        "${REPO_ROOT}/tests/k8s/scale.sh"
    else
        echo "==> Skipping the Kubernetes end-to-end test: E2E_IMAGE is not set (tests/k8s/build-image.sh)"
    fi
fi

echo ""
echo "==> Coverage report (unit + integration + VTest2)…"
# cargo llvm-cov report only knows about test binaries built by cargo test.
# To map profraw data from vigil/vigil-log-relay VTest2 subprocess runs back
# to their source lines we must call the underlying llvm tools directly:
#   1. llvm-profdata merge — collects all *.profraw files in the target dir
#   2. llvm-cov report/show — accepts explicit --object flags per binary
LLVM_BIN="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin"
LLVM_PROFDATA="${LLVM_BIN}/llvm-profdata"
LLVM_COV="${LLVM_BIN}/llvm-cov"

PROFDATA="${CARGO_LLVM_COV_TARGET_DIR}/merged.profdata"
echo "    Merging profraw files…"
"${LLVM_PROFDATA}" merge \
    "${CARGO_LLVM_COV_TARGET_DIR}"/vigil-rs-*.profraw \
    -o "${PROFDATA}"

# All instrumented objects: test binaries from cargo test + standalone CLIs
OBJECTS=()
while IFS= read -r -d '' obj; do
    OBJECTS+=("--object" "${obj}")
done < <(find "${CARGO_LLVM_COV_TARGET_DIR}/debug/deps" -maxdepth 1 \
    -type f -executable -not -name "*.d" -print0 2>/dev/null)
OBJECTS+=("--object" "${VIGILD_BIN}")
OBJECTS+=("--object" "${VIGIL_BIN}")
OBJECTS+=("--object" "${VIGIL_LOG_RELAY_BIN}")

SOURCE_FLAGS=(
    "--sources" "${REPO_ROOT}/crates"
)

if [[ "${HTML}" == true ]]; then
    HTML_DIR="${REPO_ROOT}/target/llvm-cov/html"
    mkdir -p "${HTML_DIR}"
    "${LLVM_COV}" show \
        --format=html \
        --instr-profile="${PROFDATA}" \
        --output-dir="${HTML_DIR}" \
        "${OBJECTS[@]}" \
        "${SOURCE_FLAGS[@]}"
    HTML_INDEX="${HTML_DIR}/index.html"
    echo "    HTML report: ${HTML_INDEX}"
    if command -v xdg-open &>/dev/null; then
        xdg-open "${HTML_INDEX}" &>/dev/null &
    fi
else
    "${LLVM_COV}" report \
        --instr-profile="${PROFDATA}" \
        "${OBJECTS[@]}" \
        "${SOURCE_FLAGS[@]}"
fi
