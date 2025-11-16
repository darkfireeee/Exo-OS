#!/bin/bash
# run-qemu.sh
# Script pour lancer QEMU avec l'image ISO Exo-OS

set -e

# Couleurs
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Déterminer le répertoire racine du projet
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$PROJECT_ROOT/build"

# Sélection de l'ISO (ordre de priorité):
# 1) Variable d'environnement EXO_ISO
# 2) Argument --iso <path>
# 3) build/exo-os-v2.iso
# 4) build/exo-os.iso
# 5) racine/exo-os.iso

# 1) Variable d'environnement
ISO_FILE="${EXO_ISO:-}"

# 2) Argument --iso (consommé si présent)
if [ -z "$ISO_FILE" ]; then
    while [ $# -gt 0 ]; do
        case "$1" in
            --iso)
                if [ -n "$2" ]; then
                    ISO_FILE="$2"
                    shift 2
                    break
                else
                    echo -e "${RED}[ERROR]${NC} L'option --iso requiert un chemin"
                    exit 1
                fi
                ;;
            *)
                break
                ;;
        esac
    done
fi

# 3..5) Fallbacks connus si rien de spécifié
if [ -z "$ISO_FILE" ]; then
    # Priorité 1: build/exo-os.iso (ISO officielle générée par scripts)
    if [ -f "$BUILD_DIR/exo-os.iso" ]; then
        ISO_FILE="$BUILD_DIR/exo-os.iso"
    # Priorité 2: build/exo-os-v2.iso (si variante présente)
    elif [ -f "$BUILD_DIR/exo-os-v2.iso" ]; then
        ISO_FILE="$BUILD_DIR/exo-os-v2.iso"
    # Dernier recours: ISO à la racine (déconseillé)
    elif [ -f "$PROJECT_ROOT/exo-os.iso" ]; then
        echo -e "${YELLOW}[WARN]${NC} ISO racine détectée (déconseillée): $PROJECT_ROOT/exo-os.iso"
        echo -e "${YELLOW}[WARN]${NC} Utilisez EXO_ISO=build/exo-os.iso ou supprimez l'ISO racine pour éviter les confusions"
        ISO_FILE="$PROJECT_ROOT/exo-os.iso"
    else
        # Aucun ISO trouvé → message d'erreur sera affiché plus bas
        ISO_FILE="$BUILD_DIR/exo-os.iso"
    fi
fi

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Exo-OS QEMU Runner${NC}"
echo -e "${BLUE}=========================================${NC}"

# Vérifier que l'ISO existe
if [ ! -f "$ISO_FILE" ]; then
    echo -e "${RED}[ERROR]${NC} ISO non trouvée: $ISO_FILE"
    echo -e "${YELLOW}[INFO]${NC} Exécutez d'abord: ./scripts/build-iso.sh"
    exit 1
fi

# Vérifier que QEMU est installé
if ! command -v qemu-system-x86_64 &> /dev/null; then
    echo -e "${RED}[ERROR]${NC} QEMU n'est pas installé. Installation..."
    sudo apt-get update
    sudo apt-get install -y qemu-system-x86
fi

echo -e "${GREEN}[QEMU]${NC} Lancement de Exo-OS..."
echo -e "${YELLOW}[INFO]${NC} ISO: $ISO_FILE"
# Afficher taille et SHA-256 si possible
if command -v sha256sum >/dev/null 2>&1; then
    ISO_SHA=$(sha256sum "$ISO_FILE" | awk '{print $1}')
    echo -e "${YELLOW}[INFO]${NC} SHA-256: $ISO_SHA"
fi
if command -v stat >/dev/null 2>&1; then
    ISO_SIZE=$(stat -c '%s' "$ISO_FILE" 2>/dev/null || echo "")
    if [ -n "$ISO_SIZE" ]; then
        echo -e "${YELLOW}[INFO]${NC} Taille: ${ISO_SIZE} octets"
    fi
fi
echo -e "${YELLOW}[INFO]${NC} Pour quitter: Ctrl+A puis X"
echo -e "${BLUE}=========================================${NC}"

# Lancer QEMU avec l'ISO
# Options QEMU de base
QEMU_OPTS=(
    -cdrom "$ISO_FILE"
    -boot d
    -m 512M
    -no-reboot
    -no-shutdown
)

# Mode GUI par défaut (fenêtre graphique). Pour mode headless: export EXO_QEMU_HEADLESS=1
HEADLESS="${EXO_QEMU_HEADLESS:-0}"
if [ "$HEADLESS" != "0" ]; then
    echo -e "${YELLOW}[INFO]${NC} Mode headless (-nographic)"
    QEMU_OPTS+=( -nographic -monitor none )
else
    echo -e "${YELLOW}[INFO]${NC} Mode GUI (fenêtre QEMU)"
fi

# Choix de la sortie série: fichier si EXO_SERIAL_LOG défini, sinon stdio
if [ -n "${EXO_SERIAL_LOG:-}" ]; then
    echo -e "${YELLOW}[INFO]${NC} Serial → file: ${EXO_SERIAL_LOG}"
    QEMU_OPTS+=( -serial "file:${EXO_SERIAL_LOG}" )
else
    echo -e "${YELLOW}[INFO]${NC} Serial → stdio (Ctrl+A puis X pour quitter)"
    QEMU_OPTS+=( -serial stdio )
fi

# Traces internes QEMU si demandé
if [ -n "${EXO_QEMU_TRACE:-}" ]; then
    echo -e "${YELLOW}[INFO]${NC} Tracing QEMU: ${EXO_QEMU_TRACE} → qemu-debug.txt"
    QEMU_OPTS+=( -d "${EXO_QEMU_TRACE}" -D qemu-debug.txt )
fi

# Ajout de l'appareil isa-debug-exit pour sortie automatique si activé
if [ "${EXO_QEMU_EXIT:-1}" != "0" ]; then
    echo -e "${YELLOW}[INFO]${NC} isa-debug-exit activé (port 0xF4)"
    QEMU_OPTS+=( -device isa-debug-exit,iobase=0xf4,iosize=0x04 )
fi

# Support optionnel d'un timeout via EXO_QEMU_TIMEOUT (en secondes)
if [ -n "${EXO_QEMU_TIMEOUT:-}" ]; then
    echo -e "${YELLOW}[INFO]${NC} Timeout activé: ${EXO_QEMU_TIMEOUT}s"
    timeout "${EXO_QEMU_TIMEOUT}s" qemu-system-x86_64 "${QEMU_OPTS[@]}" "$@" || true
else
    qemu-system-x86_64 "${QEMU_OPTS[@]}" "$@"
fi

echo -e "${BLUE}=========================================${NC}"
echo -e "${GREEN}[QEMU]${NC} Session terminée"
echo -e "${BLUE}=========================================${NC}"
