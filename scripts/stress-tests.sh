#!/bin/bash
# Script de tests de charge automatisés pour Exo-OS
# Génère des scénarios de stress pour chaque composant du noyau

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TEST_DIR="$PROJECT_ROOT/tests/performance"

# Couleurs pour l'affichage
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Scénarios de test définis
declare -A TEST_SCENARIOS=(
    ["memory_stress"]="Allocation mémoire continue - 10000 opérations"
    ["vga_stress"]="Écritures VGA intensives - 5000 clear_screen"
    ["interrupt_stress"]="Simulation d'interruptions - 10000 interruptions"
    ["scheduler_stress"]="Changements de contexte - 5000 context_switch"
    ["syscall_stress"]="Appels système intensifs - 10000 syscalls"
    ["multitask_stress"]="Multitâche lourd - 10 threads simultanés"
    ["mixed_stress"]="Test mixte - combinaison de tous les composants"
)

# Seuils de performance (en cycles CPU)
PERF_THRESHOLDS=(
    "vga_clear:10000"
    "context_switch:200000"
    "syscall_dispatch:5000"
    "memory_alloc:10000"
    "interrupt_handle:50000"
)

show_help() {
    cat << EOF
Script de tests de charge Exo-OS

Usage: $0 [OPTIONS] [TESTS...]

Options:
    -h, --help          Affiche cette aide
    -v, --verbose       Mode verbeux
    -o, --output DIR    Répertoire de sortie (défaut: ./tests/performance)
    -t, --threshold     Activer la vérification des seuils de performance
    -r, --report FORMAT Générer un rapport (html|json|text)
    --no-qemu           Ne pas lancer QEMU, seulement préparer les tests
    --duration SEC      Durée de chaque test en secondes (défaut: 10)

TESTS DISPONIBLES:
$(for test in "${!TEST_SCENARIOS[@]}"; do
    echo "  $test: ${TEST_SCENARIOS[$test]}"
done)

Exemples:
    $0                          # Tous les tests
    $0 memory_stress vga_stress # Tests spécifiques
    $0 -t -r html              # Tests avec seuils et rapport HTML
    $0 --duration 30           # Tests plus longs (30s chacun)

EOF
}

# Parser les arguments
VERBOSE=0
OUTPUT_DIR="$TEST_DIR"
ENABLE_THRESHOLDS=0
REPORT_FORMAT="text"
NO_QEMU=0
TEST_DURATION=10
TESTS_TO_RUN=()

while [[ $# -gt 0 ]]; do
    case $1 in
        -h|--help)
            show_help
            exit 0
            ;;
        -v|--verbose)
            VERBOSE=1
            shift
            ;;
        -o|--output)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -t|--threshold)
            ENABLE_THRESHOLDS=1
            shift
            ;;
        -r|--report)
            REPORT_FORMAT="$2"
            shift 2
            ;;
        --no-qemu)
            NO_QEMU=1
            shift
            ;;
        --duration)
            TEST_DURATION="$2"
            shift 2
            ;;
        *)
            TESTS_TO_RUN+=("$1")
            shift
            ;;
    esac
done

# Si aucun test spécifié, prendre tous les tests
if [ ${#TESTS_TO_RUN[@]} -eq 0 ]; then
    TESTS_TO_RUN=("${!TEST_SCENARIOS[@}")
fi

# Créer le répertoire de sortie
mkdir -p "$OUTPUT_DIR"

# Fonction de simulation de test de charge
generate_test_script() {
    local test_name="$1"
    local test_scenario="${TEST_SCENARIOS[$test_name]}"
    
    log_info "Génération du test: $test_name"
    
    local test_script="$OUTPUT_DIR/test_${test_name}.rs"
    
    cat > "$test_script" << EOF
//! Test de charge: $test_name
//! Scénario: $test_scenario

#![no_std]
#![allow(dead_code)]

use crate::perf_counters::{Component, rdtsc, PERF_MANAGER};

#[no_mangle]
pub extern "C" fn run_${test_name}_test() {
    log_info!("Démarrage du test: $test_name");
    
    let start_time = rdtsc();
    
    // Réinitialiser les compteurs
    PERF_MANAGER.reset();
    
    match "$test_name" {
        "memory_stress" => {
            // Test allocation mémoire continue
            for i in 0..10000 {
                let addr = allocate_memory_page();
                if i % 1000 == 0 {
                    log_info!("Allocation #\{\}");
                }
                free_memory_page(addr);
            }
        }
        "vga_stress" => {
            // Test écriture VGA intensive
            for i in 0..5000 {
                crate::libutils::display::clear_screen();
                crate::libutils::display::write_centered(12, "STRESS TEST");
                if i % 1000 == 0 {
                    log_info!("Clear screen #\{\}");
                }
            }
        }
        "interrupt_stress" => {
            // Simulation d'interruptions
            for i in 0..10000 {
                let start = rdtsc();
                simulate_interrupt();
                let end = rdtsc();
                PERF_MANAGER.record(Component::Interrupts, end - start);
                
                if i % 1000 == 0 {
                    log_info!("Interruption #\{\}");
                }
            }
        }
        "scheduler_stress" => {
            // Test changements de contexte
            for i in 0..5000 {
                let start = rdtsc();
                simulate_context_switch();
                let end = rdtsc();
                PERF_MANAGER.record(Component::Scheduler, end - start);
                
                if i % 500 == 0 {
                    log_info!("Context switch #\{\}");
                }
            }
        }
        "syscall_stress" => {
            // Test appels système
            for i in 0..10000 {
                let start = rdtsc();
                simulate_syscall();
                let end = rdtsc();
                PERF_MANAGER.record(Component::Syscall, end - start);
                
                if i % 1000 == 0 {
                    log_info!("Syscall #\{\}");
                }
            }
        }
        "multitask_stress" => {
            // Test multitâche
            run_multitask_test();
        }
        "mixed_stress" => {
            // Test mixte
            run_mixed_stress_test();
        }
        _ => {
            log_error!("Test inconnu: $test_name");
            return;
        }
    }
    
    let end_time = rdtsc();
    let total_cycles = end_time - start_time;
    
    log_info!("Test terminé: $test_name");
    log_info!("Cycles totaux: \{\}", total_cycles);
    
    // Afficher les résultats
    crate::perf_counters::print_summary_report();
}

// Fonctions de simulation (à implémenter)
fn allocate_memory_page() -> usize {
    0x100000
}

fn free_memory_page(_addr: usize) {
    // Simulation
}

fn simulate_interrupt() {
    let _dummy = 0;
    for _i in 0..10 {
        let _val = _dummy + 1;
    }
}

fn simulate_context_switch() {
    let mut regs = [0u64; 16];
    for i in 0..regs.len() {
        regs[i] = i as u64;
    }
    for i in 0..regs.len() {
        let _ = regs[i];
    }
}

fn simulate_syscall() {
    let syscall_num = 1; // write
    match syscall_num {
        0 => { /* read */ }
        1 => { /* write */ }
        2 => { /* open */ }
        3 => { /* close */ }
        60 => { /* exit */ }
        _ => { /* unknown */ }
    }
}

fn run_multitask_test() {
    log_info!("Test multitâche démarré");
    // Simulation de 10 threads
    for _i in 0..10 {
        for _j in 0..1000 {
            simulate_syscall();
            simulate_interrupt();
        }
    }
}

fn run_mixed_stress_test() {
    log_info!("Test mixte démarré");
    
    for _i in 0..1000 {
        // VGA
        crate::libutils::display::clear_screen();
        
        // Memory
        let _addr = allocate_memory_page();
        
        // Syscall
        simulate_syscall();
        
        // Interrupt
        simulate_interrupt();
        
        // Context switch
        simulate_context_switch();
    }
}

fn log_info!(msg: &str) {
    println!("[TEST {}] {}", "$test_name", msg);
}

fn log_error!(msg: &str) {
    println!("[ERROR {}] {}", "$test_name", msg);
}
EOF

    echo "$test_script"
}

# Fonction de vérification des seuils
check_thresholds() {
    local test_result="$1"
    local test_name="$2"
    
    for threshold in "${PERF_THRESHOLDS[@]}"; do
        local component="${threshold%%:*}"
        local max_cycles="${threshold##*:}"
        
        if [[ "$test_name" == *"$component"* ]]; then
            local actual_cycles=$(echo "$test_result" | grep "$component" | awk '{print $NF}')
            if [ -n "$actual_cycles" ] && [ "$actual_cycles" -gt "$max_cycles" ]; then
                log_warning "Seuil dépassé pour $component: $actual_cycles > $max_cycles"
                return 1
            fi
        fi
    done
    
    return 0
}

# Fonction de génération de rapport
generate_report() {
    local report_file="$OUTPUT_DIR/performance_report.$REPORT_FORMAT"
    
    case "$REPORT_FORMAT" in
        html)
            generate_html_report "$report_file"
            ;;
        json)
            generate_json_report "$report_file"
            ;;
        *)
            generate_text_report "$report_file"
            ;;
    esac
    
    log_success "Rapport généré: $report_file"
}

generate_text_report() {
    local file="$1"
    {
        echo "=========================================="
        echo "RAPPORT DE TESTS DE CHARGE EXO-OS"
        echo "Date: $(date)"
        echo "Durée par test: ${TEST_DURATION}s"
        echo "=========================================="
        echo ""
        
        for test in "${TESTS_TO_RUN[@]}"; do
            echo "=== TEST: $test ==="
            echo "Scénario: ${TEST_SCENARIOS[$test]}"
            
            if [ -f "$OUTPUT_DIR/results_${test}.log" ]; then
                echo "Résultats:"
                cat "$OUTPUT_DIR/results_${test}.log"
            else
                echo "❌ Résultats non trouvés"
            fi
            echo ""
        done
        
        echo "=========================================="
        echo "Tests terminés: $(date)"
        echo "=========================================="
    } > "$file"
}

generate_html_report() {
    local file="$1"
    cat > "$file" << 'EOF'
<!DOCTYPE html>
<html>
<head>
    <title>Rapport de Performance Exo-OS</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 20px; }
        .header { background-color: #f0f0f0; padding: 20px; border-radius: 5px; }
        .test { margin: 20px 0; padding: 15px; border: 1px solid #ddd; border-radius: 5px; }
        .success { background-color: #d4edda; }
        .warning { background-color: #fff3cd; }
        .error { background-color: #f8d7da; }
        .metrics { font-family: monospace; background-color: #f8f9fa; padding: 10px; }
    </style>
</head>
<body>
    <div class="header">
        <h1>🏗️ Rapport de Performance Exo-OS</h1>
        <p>Généré le: <script>document.write(new Date().toLocaleString());</script></p>
    </div>
EOF

    for test in "${TESTS_TO_RUN[@]}"; do
        echo "    <div class=\"test success\">" >> "$file"
        echo "        <h2>📊 Test: $test</h2>" >> "$file"
        echo "        <p><strong>Scénario:</strong> ${TEST_SCENARIOS[$test]}</p>" >> "$file"
        
        if [ -f "$OUTPUT_DIR/results_${test}.log" ]; then
            echo "        <div class=\"metrics\">" >> "$file"
            echo "            <pre>$(cat "$OUTPUT_DIR/results_${test}.log")</pre>" >> "$file"
            echo "        </div>" >> "$file"
        else
            echo "        <p class=\"error\">❌ Résultats non trouvés</p>" >> "$file"
        fi
        echo "    </div>" >> "$file"
    done

    cat >> "$file" << 'EOF'
    <div class="header">
        <p>📈 Rapport généré par Exo-OS Performance Test Suite</p>
    </div>
</body>
</html>
EOF
}

generate_json_report() {
    local file="$1"
    {
        echo "{"
        echo "  \"report_date\": \"$(date -Iseconds)\","
        echo "  \"test_duration\": $TEST_DURATION,"
        echo "  \"tests\": ["
        
        local first=1
        for test in "${TESTS_TO_RUN[@]}"; do
            [ $first -eq 0 ] && echo ","
            echo "    {"
            echo "      \"name\": \"$test\","
            echo "      \"scenario\": \"${TEST_SCENARIOS[$test]}\","
            
            if [ -f "$OUTPUT_DIR/results_${test}.log" ]; then
                echo "      \"results\": \"$(cat "$OUTPUT_DIR/results_${test}.log" | sed 's/"/\\"/g' | tr '\n' ' ')\","
            else
                echo "      \"results\": null,"
            fi
            
            echo "      \"status\": \"completed\""
            echo -n "    }"
            first=0
        done
        
        echo "  ]"
        echo "}"
    } > "$file"
}

# Fonction principale
main() {
    log_info "🚀 Démarrage des tests de charge Exo-OS"
    log_info "Tests à exécuter: ${TESTS_TO_RUN[*]}"
    log_info "Durée par test: ${TEST_DURATION}s"
    log_info "Vérification des seuils: $([ $ENABLE_THRESHOLDS -eq 1 ] && echo "Activée" || echo "Désactivée")"
    
    # Générer les scripts de test
    for test in "${TESTS_TO_RUN[@]}"; do
        generate_test_script "$test"
    done
    
    if [ $NO_QEMU -eq 1 ]; then
        log_info "Tests préparés dans: $OUTPUT_DIR"
        log_info "Pour lancer: ./scripts/run-tests.sh"
        return
    fi
    
    # Lancer les tests via QEMU
    for test in "${TESTS_TO_RUN[@]}"; do
        log_info "🧪 Lancement du test: $test"
        
        # Créer un script QEMU pour ce test
        local qemu_script="$OUTPUT_DIR/run_${test}.sh"
        cat > "$qemu_script" << EOF
#!/bin/bash
# Test QEMU pour: $test
cd "$PROJECT_ROOT"
./scripts/profile-kernel.sh -t $TEST_DURATION -o "$OUTPUT_DIR/results_$test"
EOF
        chmod +x "$qemu_script"
        
        # Exécuter le test
        if bash "$qemu_script" > "$OUTPUT_DIR/results_${test}.log" 2>&1; then
            log_success "Test $test terminé"
            
            # Vérifier les seuils si activé
            if [ $ENABLE_THRESHOLDS -eq 1 ]; then
                if check_thresholds "$(cat "$OUTPUT_DIR/results_${test}.log")" "$test"; then
                    log_success "Seuil respecté pour $test"
                else
                    log_warning "Seuil dépassé pour $test"
                fi
            fi
        else
            log_error "Échec du test $test"
        fi
    done
    
    # Générer le rapport final
    generate_report
    
    log_success "✅ Tous les tests terminés"
    log_info "📊 Résultats dans: $OUTPUT_DIR"
}

# Point d'entrée
main "$@"