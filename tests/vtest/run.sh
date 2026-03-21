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
#               Ignored in --coverage mode; the instrumented binary is used.

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

    exec "${VTEST}" \
        -Dvigild="${VIGILD_BIN}" \
        -Dvigil="${VIGIL_BIN}" \
        -Dvigil_log_relay="${VIGIL_LOG_RELAY_BIN}" \
        "${VTEST_OPTS[@]}" \
        "${VTC_FILES[@]}"
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

# The instrumented binaries land in the same target/debug/ as a normal build.
VIGILD_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigild"
VIGIL_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigil"
VIGIL_LOG_RELAY_BIN="${CARGO_LLVM_COV_TARGET_DIR}/debug/vigil-log-relay"

echo "==> Running unit + Rust integration tests…"
# With the show-env vars active, cargo test writes profraw files into target/.
cargo test --workspace 2>&1 | tail -5

echo "==> Running VTest2 tests…"
# LLVM_PROFILE_FILE is set by show-env.  Explicitly re-export it so that
# every subprocess spawned by VTest2 (vigild, vigil, vigil-log-relay) writes
# its profraw into the same target directory and is picked up by the report.
export LLVM_PROFILE_FILE="${LLVM_PROFILE_FILE}"
"${VTEST}" \
    -Dvigild="${VIGILD_BIN}" \
    -Dvigil="${VIGIL_BIN}" \
    -Dvigil_log_relay="${VIGIL_LOG_RELAY_BIN}" \
    "${VTEST_OPTS[@]}" \
    "${VTC_FILES[@]}"

echo ""
echo "==> Coverage report (unit + integration + VTest2)…"
# cargo llvm-cov report only knows about test binaries built by cargo test.
# To map profraw data from vigil/vigil-log-relay VTest2 subprocess runs back
# to their source lines we must call the underlying llvm tools directly:
#   1. llvm-profdata merge — collects all *.profraw files in target/
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
