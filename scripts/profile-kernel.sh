#!/bin/bash
# Script de profilage automatique pour Exo-OS
# Utilise QEMU pour mesurer les performances réelles du noyau

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
ISO_PATH="$PROJECT_ROOT/build/exo-os.iso"
PROFILE_DIR="$PROJECT_ROOT/profile_data"

# Fonction d'aide
show_help() {
    cat << EOF
Script de profilage Exo-OS

Usage: $0 [OPTIONS]

Options:
    -h, --help          Affiche cette aide
    -t, --time SEC      Durée du profilage en secondes (défaut: 30)
    -o, --output DIR    Répertoire de sortie (défaut: ./profile_data)
    -v, --verbose       Mode verbeux
    --trace             Active les traces d'exécution QEMU
    --qemu-opts OPTS    Options supplémentaires pour QEMU
    --no-build          Ne pas recompiler avant le profilage

Exemples:
    $0                              # Profilage standard (30s)
    $0 -t 60 --trace                # Profilage avec traces (60s)
    $0 -o /tmp/profile --verbose    # Sortie personnalisée

EOF
}

# Parser les arguments
TIME_SECONDS=30
VERBOSE=0
ENABLE_TRACE=0
EXTRA_QEMU_OPTS=""
DO_BUILD=1

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        -t|--time)
            TIME_SECONDS="$2"
            shift 2
            ;;
        -o|--output)
            PROFILE_DIR="$2"
            shift 2
            ;;
        -v|--verbose)
            VERBOSE=1
            shift
            ;;
        --trace)
            ENABLE_TRACE=1
            shift
            ;;
        --qemu-opts)
            EXTRA_QEMU_OPTS="$2"
            shift 2
            ;;
        --no-build)
            DO_BUILD=0
            shift
            ;;
        *)
            echo "Option inconnue: $1"
            show_help
            exit 1
            ;;
    esac
done

# Vérifications préliminaires
if [ ! -f "$ISO_PATH" ]; then
    echo "❌ ISO non trouvée: $ISO_PATH"
    echo "💡 Utilisez --no-build ou编译ez d'abord avec ./scripts/build-iso.sh"
    exit 1
fi

# Créer le répertoire de sortie
mkdir -p "$PROFILE_DIR"

log() {
    if [ $VERBOSE -eq 1 ]; then
        echo "[$(date '+%H:%M:%S')] $*"
    fi
}

# Compilation si demandée
if [ $DO_BUILD -eq 1 ]; then
    log "🔨 Compilation du projet..."
    if ! ./scripts/build-iso.sh > "$PROFILE_DIR/build.log" 2>&1; then
        echo "❌ Échec de la compilation"
        echo "📄 Consultez: $PROFILE_DIR/build.log"
        exit 1
    fi
    log "✅ Compilation réussie"
else
    log "⏭️  Compilation ignorée"
fi

# Construire la commande QEMU
QEMU_CMD="qemu-system-x86_64 \
    -cdrom $ISO_PATH \
    -m 1G \
    -smp 4 \
    -serial stdio \
    -monitor none \
    -no-reboot"

# Ajouter les options de profilage
if [ $ENABLE_TRACE -eq 1 ]; then
    QEMU_CMD="$QEMU_CMD \
        -d trace:kernel_* \
        -D $PROFILE_DIR/qemu_traces.log"
fi

# Options d'optimisation pour le profilage (sans KVM pour la compatibilité)
QEMU_CMD="$QEMU_CMD \
    -no-hpet"

# Désactiver KVM par défaut pour éviter les problèmes de permissions
# QEMU_CMD="$QEMU_CMD \
#     -enable-kvm \
#     -cpu host"

# Ajouter les options utilisateur
if [ -n "$EXTRA_QEMU_OPTS" ]; then
    QEMU_CMD="$QEMU_CMD $EXTRA_QEMU_OPTS"
fi

# Rediriger la sortie série vers un fichier
SERIAL_LOG="$PROFILE_DIR/serial_output.log"
QEMU_CMD="$QEMU_CMD > $SERIAL_LOG 2>&1"

log "🚀 Lancement du profilage ($TIME_SECONDS secondes)"
log "📁 Sortie: $PROFILE_DIR"
log "🔍 Traces QEMU: $([ $ENABLE_TRACE -eq 1 ] && echo "Activées" || echo "Désactivées")"

# Démarrer QEMU en arrière-plan
eval "$QEMU_CMD" &
QEMU_PID=$!

log "📊 Processus QEMU PID: $QEMU_PID"

# Fonction de nettoyage
cleanup() {
    log "🧹 Nettoyage..."
    if kill -0 $QEMU_PID 2>/dev/null; then
        log "⏹️  Arrêt de QEMU (PID: $QEMU_PID)"
        kill -TERM $QEMU_PID 2>/dev/null || true
        sleep 2
        kill -KILL $QEMU_PID 2>/dev/null || true
    fi
}

trap cleanup EXIT

# Attendre la durée spécifiée
log "⏱️  Profilage en cours..."
sleep $TIME_SECONDS

# Arrêter proprement QEMU
cleanup

# Traiter les résultats
log "📈 Analyse des résultats..."

# Parser les données de performance depuis les logs série
PERF_LOG="$PROFILE_DIR/performance_analysis.log"
{
    echo "========================================"
    echo "RAPPORT DE PROFILAGE EXO-OS"
    echo "Date: $(date)"
    echo "Durée: ${TIME_SECONDS}s"
    echo "========================================"
    echo ""
    
    if [ -f "$SERIAL_LOG" ]; then
        echo "=== SYNTHÈSE DE PERFORMANCE ==="
        grep -A 20 "SYNTHESE DE PERFORMANCE" "$SERIAL_LOG" || echo "❌ Données de performance non trouvées"
        echo ""
        
        echo "=== MÉTRIQUES DÉTAILLÉES ==="
        grep -E "(VGA|Scheduler|Syscall|Memory).*cycles.*moyen" "$SERIAL_LOG" || echo "❌ Métriques détaillées non trouvées"
        echo ""
        
        echo "=== LOGS DE DÉMARRAGE ==="
        grep -E "(INIT|SUCCESS|KERNEL)" "$SERIAL_LOG" | head -20 || echo "❌ Logs de démarrage non trouvés"
        echo ""
    else
        echo "❌ Fichier de sortie série non trouvé: $SERIAL_LOG"
    fi
    
    # Analyse des traces QEMU si disponibles
    if [ $ENABLE_TRACE -eq 1 ] && [ -f "$PROFILE_DIR/qemu_traces.log" ]; then
        echo "=== ANALYSE DES TRACES QEMU ==="
        echo "Nombre d'événements trace: $(wc -l < "$PROFILE_DIR/qemu_traces.log")"
        echo ""
        echo "Types d'événements les plus fréquents:"
        cut -d: -f1 "$PROFILE_DIR/qemu_traces.log" | sort | uniq -c | sort -nr | head -10
        echo ""
    fi
    
    echo "========================================"
    echo "Rapport généré le: $(date)"
    echo "========================================"

} > "$PERF_LOG"

# Générer un rapport de performance
SUMMARY_LOG="$PROFILE_DIR/summary.txt"
{
    echo "Exo-OS Performance Profile Summary"
    echo "=================================="
    echo ""
    echo "Configuration:"
    echo "  - Durée profilage: ${TIME_SECONDS}s"
    echo "  - Mémoire QEMU: 1GB"
    echo "  - CPUs: 4"
    echo "  - Traces: $([ $ENABLE_TRACE -eq 1 ] && echo "Activées" || echo "Désactivées")"
    echo ""
    echo "Fichiers générés:"
    echo "  - $SERIAL_LOG"
    echo "  - $PERF_LOG"
    if [ $ENABLE_TRACE -eq 1 ]; then
        echo "  - $PROFILE_DIR/qemu_traces.log"
    fi
    echo ""
    echo "Pour analyser en détail:"
    echo "  less $PERF_LOG"
    echo "  cat $SERIAL_LOG"
    
} > "$SUMMARY_LOG"

# Créer un graphique simple (si gnuplot est disponible)
if command -v gnuplot >/dev/null 2>&1; then
    log "📊 Génération des graphiques..."
    
    # Extraire les données VGA pour un graphique simple
    if [ -f "$SERIAL_LOG" ]; then
        grep "VGA:" "$SERIAL_LOG" | awk '{print $4}' | grep -E '^[0-9]+$' > "$PROFILE_DIR/vga_cycles.dat" 2>/dev/null || true
        
        if [ -f "$PROFILE_DIR/vga_cycles.dat" ]; then
            cat > "$PROFILE_DIR/plot.gp" << 'EOF'
set terminal png size 800,600
set output "vga_performance.png"
set title "Performance VGA - Cycles CPU"
set xlabel "Mesure"
set ylabel "Cycles CPU"
plot "vga_cycles.dat" with linespoints title "Cycles VGA"
EOF
            cd "$PROFILE_DIR"
            gnuplot plot.gp 2>/dev/null || true
            cd - >/dev/null
        fi
    fi
fi

echo ""
echo "🎉 Profilage terminé!"
echo ""
echo "📊 Résultats dans: $PROFILE_DIR"
echo "📄 Rapport principal: $PERF_LOG"
echo "📋 Résumé: $SUMMARY_LOG"
echo ""

# Afficher un aperçu des résultats
if [ -f "$PERF_LOG" ]; then
    echo "🔍 Aperçu des résultats:"
    head -30 "$PERF_LOG"
    echo ""
    echo "💡 Rapport complet: less $PERF_LOG"
fi

log "✅ Profilage terminé avec succès"