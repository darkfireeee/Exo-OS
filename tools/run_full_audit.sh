#!/usr/bin/env bash
# System audit runner for the Exo-OS pre-v0.2 verification pass.
# Run from WSL so bare-metal targets, Rust audit tools, and QEMU-adjacent
# validation use the same environment as the build pipeline.

set -u

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STRICT=0
RUN_TESTS=1
RUN_KANI=1
RUN_DEPS=1
RUN_TLA=1
TLA_REQUIRED=0
PASS=0
FAIL=0
WARN=0

usage() {
    cat <<'EOF'
Usage: bash tools/run_full_audit.sh [options]

Prerequisite for the exo-boot scope:
  rustup target add x86_64-unknown-uefi

Options:
  --strict       Treat required audit failures as a non-zero result.
  --skip-tests   Skip unit-test stages after compile and static checks.
  --skip-kani    Skip Kani even when it is installed.
  --skip-deps    Skip cargo-deny and cargo-audit dependency stages.
  --skip-tla     Skip TLA+ semantic validation.
  --with-tla     Require TLA+ semantic validation instead of warning if absent.
  -h, --help     Show this help.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --strict)
            STRICT=1
            ;;
        --skip-tests)
            RUN_TESTS=0
            ;;
        --skip-kani)
            RUN_KANI=0
            ;;
        --skip-deps)
            RUN_DEPS=0
            ;;
        --skip-tla)
            RUN_TLA=0
            ;;
        --with-tla)
            RUN_TLA=1
            TLA_REQUIRED=1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

cd "$ROOT_DIR"

step() {
    printf '\n== %s ==\n' "$1"
}

ok() {
    printf '  PASS %s\n' "$1"
    PASS=$((PASS + 1))
}

fail() {
    printf '  FAIL %s\n' "$1"
    FAIL=$((FAIL + 1))
}

warn() {
    printf '  WARN %s\n' "$1"
    WARN=$((WARN + 1))
}

run_required() {
    local label="$1"
    shift
    printf '  RUN  %s\n' "$label"
    if "$@"; then
        ok "$label"
    else
        fail "$label"
    fi
}

run_required_in_dir() {
    local label="$1"
    local dir="$2"
    shift 2
    printf '  RUN  %s\n' "$label"
    if (cd "$dir" && "$@"); then
        ok "$label"
    else
        fail "$label"
    fi
}

run_skipped() {
    warn "$1 skipped by option"
}

server_manifests() {
    find servers -mindepth 2 -maxdepth 2 -type f -name Cargo.toml | sort
}

local_lib_src_dirs() {
    find libs -mindepth 2 -maxdepth 2 -type d -name src \
        ! -path 'libs/vendors/*' | sort
}

has_first_party_kani_proofs() {
    grep -R --include='*.rs' -q '#\[kani::proof\]' \
        exo-boot/src loader/src kernel/src servers userspace libs/*/src \
        2>/dev/null
}

semgrep_scope() {
    local label="$1"
    shift
    run_required "Semgrep ${label}" \
        semgrep --config tools/semgrep-rules/exoos.yaml --error --timeout 30 "$@"
}

find_tla_jar() {
    if [ -n "${TLA2TOOLS_JAR:-}" ] && [ -f "$TLA2TOOLS_JAR" ]; then
        printf '%s\n' "$TLA2TOOLS_JAR"
        return 0
    fi
    if [ -f /opt/tlaplus/tla2tools.jar ]; then
        printf '%s\n' /opt/tlaplus/tla2tools.jar
        return 0
    fi
    if [ -f "$ROOT_DIR/tla2tools.jar" ]; then
        printf '%s\n' "$ROOT_DIR/tla2tools.jar"
        return 0
    fi
    return 1
}

require_or_warn_tla() {
    if [ "$TLA_REQUIRED" -eq 1 ]; then
        fail "$1"
    else
        warn "$1"
    fi
}

step "Scope compile checks"
run_required "exo-boot UEFI check" \
    cargo check --manifest-path exo-boot/Cargo.toml \
        --target x86_64-unknown-uefi \
        --features uefi-boot,dev-skip-sig
run_required "loader check" \
    cargo check --manifest-path loader/Cargo.toml --all-targets
run_required "kernel bare-metal check" \
    cargo check -p exo-os-kernel --lib \
        -Z build-std=core,alloc,compiler_builtins \
        -Z build-std-features=compiler-builtins-mem \
        --target x86_64-unknown-none
while IFS= read -r manifest; do
    [ -n "$manifest" ] || continue
    server_name="$(basename "$(dirname "$manifest")")"
    run_required "server ${server_name} check" \
        cargo check --manifest-path "$manifest"
done < <(server_manifests)
run_required "userspace workspace check" \
    cargo check --manifest-path userspace/Cargo.toml --workspace --all-targets

step "Constant and static audits"
run_required "critical constant audit" \
    python3 tools/audit_constants.py --fail-on-warn
if command -v semgrep >/dev/null 2>&1; then
    semgrep_scope "exo-boot" exo-boot/src
    semgrep_scope "loader" loader/src
    semgrep_scope "kernel" kernel/src
    while IFS= read -r manifest; do
        [ -n "$manifest" ] || continue
        server_dir="$(dirname "$manifest")"
        server_name="$(basename "$server_dir")"
        if [ -d "$server_dir/src" ]; then
            semgrep_scope "server ${server_name}" "$server_dir/src"
        fi
    done < <(server_manifests)
    semgrep_scope "userspace" userspace

    scan_paths=(exo-boot/src loader/src kernel/src servers userspace)
    while IFS= read -r lib_src; do
        [ -n "$lib_src" ] || continue
        scan_paths+=("$lib_src")
    done < <(local_lib_src_dirs)
    semgrep_scope "global first-party tree" "${scan_paths[@]}"
else
    warn "Semgrep unavailable; install it before relying on static scan coverage"
fi

step "Dependency audits"
if [ "$RUN_DEPS" -eq 0 ]; then
    run_skipped "dependency audits"
else
    if cargo deny --version >/dev/null 2>&1; then
        run_required "cargo-deny" cargo deny check
    else
        warn "cargo-deny unavailable"
    fi
    if cargo audit --version >/dev/null 2>&1; then
        printf '  RUN  cargo-audit\n'
        if cargo audit; then
            ok "cargo-audit"
        else
            warn "cargo-audit reported advisories or could not complete"
        fi
    else
        warn "cargo-audit unavailable"
    fi
fi

step "Kani"
if [ "$RUN_KANI" -eq 0 ]; then
    run_skipped "Kani"
elif ! has_first_party_kani_proofs; then
    warn "No first-party #[kani::proof] harnesses found; docs still describe planned Kani coverage"
elif cargo kani --version >/dev/null 2>&1; then
    run_required "Kani proofs" \
        timeout 900s cargo kani --tests -Z unstable-options --ignore-global-asm
else
    warn "Kani unavailable"
fi

step "TLA semantic checks"
if [ "$RUN_TLA" -eq 0 ]; then
    run_skipped "TLA semantic checks"
elif ! command -v java >/dev/null 2>&1; then
    require_or_warn_tla "Java unavailable for docs/Exo-OS-TLA+ specs"
elif ! TLA_JAR="$(find_tla_jar)"; then
    require_or_warn_tla "tla2tools.jar unavailable; set TLA2TOOLS_JAR or install /opt/tlaplus/tla2tools.jar"
else
    while IFS= read -r spec; do
        [ -n "$spec" ] || continue
        spec_name="$(basename "$spec")"
        run_required_in_dir "TLA SANY ${spec_name}" docs/Exo-OS-TLA+ \
            java -cp "$TLA_JAR" tla2sany.SANY "$spec_name"
    done < <(find docs/Exo-OS-TLA+ -maxdepth 1 -type f -name '*.tla' | sort)
fi

step "Focused unit tests"
if [ "$RUN_TESTS" -eq 0 ]; then
    run_skipped "unit tests"
else
    run_required "kernel unit tests" make test
    run_required "loader tests" make test-loader
    run_required "userspace tests" make test-userspace
fi

printf '\n== Summary ==\n'
printf '  PASS %s\n' "$PASS"
printf '  FAIL %s\n' "$FAIL"
printf '  WARN %s\n' "$WARN"

if [ "$FAIL" -gt 0 ]; then
    printf '  RESULT FAIL\n'
    exit 1
fi
if [ "$STRICT" -eq 1 ] && [ "$WARN" -gt 0 ]; then
    printf '  RESULT WARNINGS-IN-STRICT-MODE\n'
    exit 1
fi

printf '  RESULT PASS\n'
