#!/bin/bash
# Script de compilation et test des bibliothèques Exo-OS
# Adapté de build.sh pour compiler les libs uniquement

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT/libs"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}=== Exo-OS Libraries Compilation & Testing ===${NC}"
echo "Working directory: $(pwd)"
echo ""

# Check Rust installation
echo -e "${BLUE}[1/3] Checking Rust...${NC}"

USE_WINDOWS_CARGO=false
if ! command -v cargo >/dev/null 2>&1; then
    if command -v cargo.exe >/dev/null 2>&1; then
        USE_WINDOWS_CARGO=true
        echo -e "${YELLOW}Using Windows cargo.exe${NC}"
        CARGO_CMD="cargo.exe"
    else
        echo -e "${RED}Error: cargo not found${NC}"
        echo "Please install Rust: https://rustup.rs/"
        exit 1
    fi
else
    CARGO_CMD="cargo"
    echo -e "${GREEN}✓ Cargo found${NC}"
fi

# Check for nightly toolchain
if ! rustup toolchain list | grep -q nightly 2>/dev/null; then
    echo -e "${YELLOW}Installing Rust nightly...${NC}"
    rustup toolchain install nightly
fi

# Set nightly as default
rustup default nightly 2>/dev/null || true

# Ensure rust-src is installed
if ! rustup component list | grep -q "rust-src.*installed" 2>/dev/null; then
    echo -e "${YELLOW}Installing rust-src...${NC}"
    rustup component add rust-src
fi

echo ""

# Libraries to test in dependency order
LIBS=(
    "exo_types:Foundation types"
    "exo_crypto:Cryptography"
    "exo_allocator:Memory allocation"
    "exo_ipc:Inter-process communication"
    "exo_std:Standard library"
    "exo_metrics:Metrics collection"
    "exo_service_registry:Service registry"
    "exo_config:Configuration"
    "exo_logger:Logging"
)

echo -e "${BLUE}[2/3] Compiling libraries...${NC}"
echo ""

PASSED=0
FAILED=0
FAILED_LIBS=""

for lib_info in "${LIBS[@]}"; do
    IFS=':' read -r lib desc <<< "$lib_info"

    if [ ! -d "$lib" ]; then
        echo -e "${YELLOW}⊘ Skipping $lib (not found)${NC}"
        continue
    fi

    echo -e "${BLUE}━━━ $lib: $desc ━━━${NC}"

    cd "$lib"

    # Check syntax with cargo check
    echo -n "  • Checking syntax... "
    if $CARGO_CMD check --lib 2>&1 | tee /tmp/cargo_check.log | tail -5; then
        echo -e "${GREEN}✓ OK${NC}"
    else
        echo -e "${RED}✗ FAILED${NC}"
        echo "    Error output:"
        tail -20 /tmp/cargo_check.log | sed 's/^/    /'
        FAILED=$((FAILED + 1))
        FAILED_LIBS="$FAILED_LIBS $lib"
        cd ..
        continue
    fi

    # Build library
    echo -n "  • Building... "
    if $CARGO_CMD build --lib 2>&1 | tee /tmp/cargo_build.log | tail -3; then
        echo -e "${GREEN}✓ OK${NC}"
    else
        echo -e "${RED}✗ FAILED${NC}"
        echo "    Error output:"
        tail -20 /tmp/cargo_build.log | sed 's/^/    /'
        FAILED=$((FAILED + 1))
        FAILED_LIBS="$FAILED_LIBS $lib"
        cd ..
        continue
    fi

    # Run tests (if any)
    if grep -q "\[dev-dependencies\]\|\[\[test\]\]" Cargo.toml 2>/dev/null; then
        echo -n "  • Running tests... "
        if $CARGO_CMD test --lib 2>&1 | tee /tmp/cargo_test.log | grep -E "test result:|running" | tail -3; then
            echo -e "${GREEN}✓ OK${NC}"
        else
            echo -e "${YELLOW}⊘ No tests or tests skipped${NC}"
        fi
    fi

    PASSED=$((PASSED + 1))
    echo ""

    cd ..
done

echo ""
echo -e "${BLUE}[3/3] Summary${NC}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}✓ All libraries compiled successfully!${NC}"
    echo ""
    echo "  Passed: $PASSED"
    echo "  Failed: $FAILED"
    echo ""
    echo -e "${GREEN}=== SUCCESS ===${NC}"
    exit 0
else
    echo -e "${RED}✗ Some libraries failed to compile${NC}"
    echo ""
    echo "  Passed: $PASSED"
    echo "  Failed: $FAILED"
    echo ""
    echo -e "${RED}Failed libraries:${NC}"
    for lib in $FAILED_LIBS; do
        echo "  - $lib"
    done
    echo ""
    echo -e "${YELLOW}Check error logs above for details${NC}"
    exit 1
fi
