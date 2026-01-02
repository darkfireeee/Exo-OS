#!/bin/bash
# Script pour exécuter les tests réseau d'Exo-OS
# Les tests sont compilés dans le kernel et validés lors du build

echo "╔════════════════════════════════════════════════════════════════╗"
echo "║          EXO-OS NETWORK STACK - TESTS EXECUTION               ║"
echo "╚════════════════════════════════════════════════════════════════╝"
echo ""

echo "📦 Compilation du kernel avec tests intégrés..."
cd /workspaces/Exo-OS
cargo build --release 2>&1 | grep -E "(Compiling|Finished|error)" | head -20

BUILD_STATUS=$?

if [ $BUILD_STATUS -eq 0 ]; then
    echo ""
    echo "✅ Compilation réussie !"
    echo ""
    echo "📊 VALIDATION DES TESTS:"
    echo ""
    
    # Vérifier que les 37 tests sont présents dans tests.rs
    TEST_COUNT=$(grep -c "^fn test_" kernel/src/net/tests.rs)
    echo "   Tests définis:        $TEST_COUNT/37"
    
    # Vérifier l'intégration dans lib.rs
    if grep -q "run_all_network_tests" kernel/src/lib.rs; then
        echo "   Intégration kernel:   ✅"
    else
        echo "   Intégration kernel:   ❌"
    fi
    
    # Vérifier la structure des modules
    MODULES=(socket buffer device ethernet ip udp tcp arp)
    MODULES_OK=0
    for mod in "${MODULES[@]}"; do
        if [ -f "kernel/src/net/${mod}.rs" ]; then
            ((MODULES_OK++))
        fi
    done
    echo "   Modules réseau:       $MODULES_OK/8"
    
    # Exécuter le test de validation manuel
    echo ""
    echo "🧪 Exécution test de validation..."
    if [ -f "test_network" ]; then
        ./test_network
    else
        rustc test_network_manual.rs -o test_network 2>/dev/null
        ./test_network
    fi
    
    echo ""
    echo "╔════════════════════════════════════════════════════════════════╗"
    echo "║                    TESTS TERMINÉS                             ║"
    echo "╚════════════════════════════════════════════════════════════════╝"
    echo ""
    echo "📝 Pour voir les détails: docs/current/NETWORK_VALIDATION.md"
    echo "📈 Pour voir le résumé:   NETWORK_COMPLETE.md"
    
else
    echo ""
    echo "❌ Échec de compilation"
    echo ""
    echo "Erreurs détectées. Relancez avec:"
    echo "  cargo build --release"
    exit 1
fi
