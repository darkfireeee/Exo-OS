#!/usr/bin/env bash
# run_tests.sh — Exo-OS test runner (P2-8)
#
# Couverture : kernel bare-metal check + tests host ciblés
# Comportement 0 tests : warning + exit 0
# Sortie : résumé court pass/fail/skip
#
# Usage : ./run_tests.sh [--verbose]

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$REPO_ROOT/kernel"
LIBS_DIR="$REPO_ROOT/libs"
CARGO_BARE_CMD=(cargo +nightly check -Z build-std=core,alloc --target x86_64-unknown-none)
CARGO_HOST_CMD=(cargo test --target x86_64-unknown-linux-gnu)
VERBOSE="${1:-}"

# ── Couleurs ──────────────────────────────────────────────────────────────────

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BLUE='\033[0;34m'
RESET='\033[0m'

# ── Compteurs globaux ─────────────────────────────────────────────────────────

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0
TOTAL_WARN=0
FAILURES=""

# ── Helpers ───────────────────────────────────────────────────────────────────

log_step() { echo -e "\n${BLUE}▶ $1${RESET}"; }
log_ok()   { echo -e "  ${GREEN}✓ $1${RESET}"; }
log_warn() { echo -e "  ${YELLOW}⚠ $1${RESET}"; TOTAL_WARN=$((TOTAL_WARN + 1)); }
log_fail() { echo -e "  ${RED}✗ $1${RESET}"; FAILURES="$FAILURES\n  - $1"; }

# Parse la sortie cargo test et met à jour les compteurs
# Attend en entrée le log brut cargo test
parse_test_output() {
    local log="$1"
    local label="$2"

    if ! grep -q -E '^test result' "$log"; then
        log_warn "$label : aucune sortie 'test result' détectée (0 tests ?)"
        return 0
    fi

    # Agrège toutes les lignes "test result" (unit tests + doc-tests éventuels).
    local passed failed ignored
    passed=$(awk '/^test result:/{for(i=1;i<=NF;i++){if($i=="passed;") p+=$(i-1)}} END{print p+0}' "$log")
    failed=$(awk '/^test result:/{for(i=1;i<=NF;i++){if($i=="failed;") f+=$(i-1)}} END{print f+0}' "$log")
    ignored=$(awk '/^test result:/{for(i=1;i<=NF;i++){if($i=="ignored;") ig+=$(i-1)}} END{print ig+0}' "$log")

    TOTAL_PASS=$((TOTAL_PASS + passed))
    TOTAL_FAIL=$((TOTAL_FAIL + failed))
    TOTAL_SKIP=$((TOTAL_SKIP + ignored))

    if [[ "$passed" -eq 0 && "$failed" -eq 0 ]]; then
        log_warn "$label : 0 tests exécutés (ignored=$ignored) — vérifier les filtres"
    elif [[ "$failed" -gt 0 ]]; then
        log_fail "$label : $failed test(s) échoué(s) / $passed passé(s) / $ignored ignoré(s)"
        if [[ "$VERBOSE" == "--verbose" ]]; then
            grep -E '^test .* FAILED' "$log" | sed 's/^/    /' || true
        fi
    else
        log_ok "$label : $passed passé(s) / $ignored ignoré(s)"
    fi
}

# ── Étape 1 : kernel bare-metal check ────────────────────────────────────────

log_step "Kernel bare-metal check (x86_64-unknown-none)"

BARE_LOG=$(mktemp /tmp/exoos_bare_XXXXXX.log)
if "${CARGO_BARE_CMD[@]}" -p exo-os-kernel \
        --manifest-path "$KERNEL_DIR/Cargo.toml" \
        > "$BARE_LOG" 2>&1; then
    log_ok "kernel bare-metal check : OK"
    TOTAL_PASS=$((TOTAL_PASS + 1))
else
    log_fail "kernel bare-metal check : ÉCHEC"
    TOTAL_FAIL=$((TOTAL_FAIL + 1))
    if [[ "$VERBOSE" == "--verbose" ]]; then
        cat "$BARE_LOG"
    else
        tail -20 "$BARE_LOG"
    fi
fi
rm -f "$BARE_LOG"

# ── Étape 2 : tests GI-02 host (percpu / security / fpu) ─────────────────────

log_step "Tests GI-02 host (percpu / security / fpu)"

GI02_LOG=$(mktemp /tmp/exoos_gi02_XXXXXX.log)
if "${CARGO_HOST_CMD[@]}" -p exo-os-kernel \
        --manifest-path "$KERNEL_DIR/Cargo.toml" \
        --lib \
        p2_7_ \
        > "$GI02_LOG" 2>&1; then
    parse_test_output "$GI02_LOG" "GI-02 host (security + fpu)"
else
    # cargo test retourne non-zero si des tests échouent OU si la compile échoue
    if grep -q 'error\[' "$GI02_LOG"; then
        log_fail "GI-02 host : erreur de compilation"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        tail -30 "$GI02_LOG"
    else
        parse_test_output "$GI02_LOG" "GI-02 host (security + fpu)"
    fi
fi
rm -f "$GI02_LOG"

# ── Étape 3 : tests exo-types host ───────────────────────────────────────────

log_step "Tests exo-types host"

TYPES_LOG=$(mktemp /tmp/exoos_types_XXXXXX.log)
if "${CARGO_HOST_CMD[@]}" -p exo-types \
        --manifest-path "$LIBS_DIR/Cargo.toml" \
        > "$TYPES_LOG" 2>&1; then
    parse_test_output "$TYPES_LOG" "exo-types"
else
    if grep -q 'error\[' "$TYPES_LOG"; then
        log_fail "exo-types : erreur de compilation"
        TOTAL_FAIL=$((TOTAL_FAIL + 1))
        tail -30 "$TYPES_LOG"
    else
        parse_test_output "$TYPES_LOG" "exo-types"
    fi
fi
rm -f "$TYPES_LOG"

# ── Résumé final ──────────────────────────────────────────────────────────────

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo -e "  Résultat final Exo-OS"
echo -e "  ${GREEN}PASS  : $TOTAL_PASS${RESET}"
echo -e "  ${RED}FAIL  : $TOTAL_FAIL${RESET}"
echo -e "  ${YELLOW}SKIP  : $TOTAL_SKIP${RESET}"
echo -e "  ${YELLOW}WARN  : $TOTAL_WARN${RESET}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [[ $TOTAL_FAIL -gt 0 ]]; then
    echo -e "\n${RED}Échecs :${RESET}$FAILURES"
    echo ""
    exit 1
fi

if [[ $TOTAL_WARN -gt 0 ]]; then
    echo -e "${YELLOW}Warnings présents — vérifier les filtres de test.${RESET}"
fi

echo ""
exit 0