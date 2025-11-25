#!/bin/bash
# diagnose_cache.sh - Diagnostic complet du problème de cache

set -e

echo "========================================"
echo "  EXO-OS CACHE DIAGNOSTIC"
echo "========================================"
echo ""

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check() {
    echo -e "${BLUE}[CHECK]${NC} $1"
}

echo ""
info "Step 1: Checking file timestamps..."
echo ""

# Variables
KERNEL_SOURCE="kernel/src/main.rs"
KERNEL_BIN="target/x86_64-unknown-none/release/kernel"
ISO_FILE="build/os.iso"
ISO_MOUNT="/tmp/exo-os-mount"

# Fonction pour afficher timestamp lisible
show_timestamp() {
    local file=$1
    if [ -f "$file" ]; then
        local timestamp=$(stat -c %y "$file" 2>/dev/null || stat -f "%Sm" "$file" 2>/dev/null)
        echo "  $file"
        echo "    └─ $timestamp"
    else
        echo "  $file"
        echo "    └─ ${RED}NOT FOUND${NC}"
    fi
}

# Vérifier les timestamps
check "Source file:"
show_timestamp "$KERNEL_SOURCE"

echo ""
check "Compiled kernel:"
show_timestamp "$KERNEL_BIN"

echo ""
check "ISO file:"
show_timestamp "$ISO_FILE"

# Comparer les timestamps
echo ""
info "Step 2: Comparing modification times..."
echo ""

if [ -f "$KERNEL_SOURCE" ] && [ -f "$KERNEL_BIN" ]; then
    SOURCE_TIME=$(stat -c %Y "$KERNEL_SOURCE" 2>/dev/null || stat -f "%m" "$KERNEL_SOURCE" 2>/dev/null)
    BIN_TIME=$(stat -c %Y "$KERNEL_BIN" 2>/dev/null || stat -f "%m" "$KERNEL_BIN" 2>/dev/null)
    
    if [ "$BIN_TIME" -lt "$SOURCE_TIME" ]; then
        error "kernel binary is OLDER than source!"
        warn "This means the kernel was NOT recompiled"
    else
        info "kernel binary is newer than source ✓"
    fi
fi

if [ -f "$KERNEL_BIN" ] && [ -f "$ISO_FILE" ]; then
    BIN_TIME=$(stat -c %Y "$KERNEL_BIN" 2>/dev/null || stat -f "%m" "$KERNEL_BIN" 2>/dev/null)
    ISO_TIME=$(stat -c %Y "$ISO_FILE" 2>/dev/null || stat -f "%m" "$ISO_FILE" 2>/dev/null)
    
    if [ "$ISO_TIME" -lt "$BIN_TIME" ]; then
        error "ISO is OLDER than kernel binary!"
        warn "This means the ISO was NOT regenerated"
    else
        info "ISO is newer than kernel binary ✓"
    fi
fi

# Vérifier le contenu de l'ISO
echo ""
info "Step 3: Checking ISO contents..."
echo ""

if [ -f "$ISO_FILE" ]; then
    # Créer point de montage
    mkdir -p "$ISO_MOUNT"
    
    # Monter l'ISO
    if sudo mount -o loop "$ISO_FILE" "$ISO_MOUNT" 2>/dev/null; then
        check "ISO mounted at $ISO_MOUNT"
        
        # Lister le contenu
        echo ""
        check "Files in ISO:"
        ls -lh "$ISO_MOUNT/"
        
        # Vérifier si kernel.bin existe
        if [ -f "$ISO_MOUNT/boot/kernel.bin" ]; then
            KERNEL_IN_ISO="$ISO_MOUNT/boot/kernel.bin"
            show_timestamp "$KERNEL_IN_ISO"
            
            # Comparer taille
            if [ -f "$KERNEL_BIN" ]; then
                SIZE_SOURCE=$(stat -c %s "$KERNEL_BIN" 2>/dev/null || stat -f "%z" "$KERNEL_BIN")
                SIZE_ISO=$(stat -c %s "$KERNEL_IN_ISO" 2>/dev/null || stat -f "%z" "$KERNEL_IN_ISO")
                
                echo ""
                check "Kernel sizes:"
                echo "  Source: $SIZE_SOURCE bytes"
                echo "  In ISO: $SIZE_ISO bytes"
                
                if [ "$SIZE_SOURCE" != "$SIZE_ISO" ]; then
                    error "Sizes DIFFER! ISO contains OLD kernel!"
                else
                    info "Sizes match ✓"
                fi
            fi
        else
            error "kernel.bin NOT FOUND in ISO!"
        fi
        
        # Démonter
        sudo umount "$ISO_MOUNT"
    else
        warn "Could not mount ISO (need sudo)"
    fi
    
    rmdir "$ISO_MOUNT" 2>/dev/null || true
else
    error "ISO file not found: $ISO_FILE"
fi

# Vérifier le contenu du kernel
echo ""
info "Step 4: Searching for strings in kernel..."
echo ""

if [ -f "$KERNEL_BIN" ]; then
    check "Searching for 'Magic: 0x' in kernel binary..."
    
    if strings "$KERNEL_BIN" | grep -q "Magic: 0x"; then
        info "FOUND 'Magic: 0x' in kernel! ✓"
        echo "  Matching lines:"
        strings "$KERNEL_BIN" | grep "Magic"
    else
        error "NOT FOUND 'Magic: 0x' in kernel!"
        warn "This confirms the kernel was NOT recompiled with new code"
        
        echo ""
        check "Searching for old string 'Initializing logger'..."
        if strings "$KERNEL_BIN" | grep -q "Initializing logger"; then
            error "Found OLD string! The kernel IS outdated"
        fi
    fi
else
    error "Kernel binary not found: $KERNEL_BIN"
fi

# Résumé
echo ""
echo "========================================"
echo "  DIAGNOSTIC SUMMARY"
echo "========================================"
echo ""

ISSUE_FOUND=0

# Check 1: Binary newer than source?
if [ -f "$KERNEL_SOURCE" ] && [ -f "$KERNEL_BIN" ]; then
    SOURCE_TIME=$(stat -c %Y "$KERNEL_SOURCE" 2>/dev/null || stat -f "%m" "$KERNEL_SOURCE" 2>/dev/null)
    BIN_TIME=$(stat -c %Y "$KERNEL_BIN" 2>/dev/null || stat -f "%m" "$KERNEL_BIN" 2>/dev/null)
    
    if [ "$BIN_TIME" -lt "$SOURCE_TIME" ]; then
        error "Issue 1: Kernel not recompiled"
        ISSUE_FOUND=1
    else
        info "✓ Kernel is up to date"
    fi
fi

# Check 2: New string present?
if [ -f "$KERNEL_BIN" ]; then
    if ! strings "$KERNEL_BIN" | grep -q "Magic: 0x"; then
        error "Issue 2: New code not present in binary"
        ISSUE_FOUND=1
    else
        info "✓ New code is present"
    fi
fi

echo ""
if [ $ISSUE_FOUND -eq 1 ]; then
    error "CACHE PROBLEM CONFIRMED!"
    echo ""
    echo "Recommended actions:"
    echo "  1. Run: cargo clean"
    echo "  2. Run: rm -rf target/"
    echo "  3. Run: rm build/os.iso"
    echo "  4. Run: cargo build --release"
    echo "  5. Run: ./build/image.sh"
else
    info "No cache issues detected"
    warn "The problem might be in the runtime code (stack, memory, etc.)"
fi

echo ""
