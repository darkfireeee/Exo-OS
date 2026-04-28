--- docs/audit/AUDIT_02_Scripts_CI_CD.md (原始)


+++ docs/audit/AUDIT_02_Scripts_CI_CD.md (修改后)
# ExoOS — Scripts d'Audit Automatisé CI/CD

## 🎯 Objectif : Validation automatique 100% conformité

**Scripts bash/rust** pour intégration continue
**Détection** : unwrap(), static mut, TODOs, heap en ISR, Ordering::Relaxed
**Intégration** : GitHub Actions / GitLab CI / Jenkins

---

## 📜 Script 1 : audit_unwrap.sh

```bash
#!/bin/bash
# scripts/audit_unwrap.sh — Compter et localiser unwrap() en production

set -e

echo "🔍 Audit unwrap() — Code production uniquement"
echo "================================================"

# Dirs à auditer (exclure tests)
DIRS="kernel/src servers/*/src drivers/*/src libs/*/src"

TOTAL=0
CRITICAL=0

for dir in $DIRS; do
    if [ ! -d "$dir" ]; then
        continue
    fi

    echo ""
    echo "📁 $dir"
    echo "-------------------------------------------"

    # Trouver unwrap() hors fichiers de test
    MATCHES=$(find "$dir" -name "*.rs" \
        ! -name "*_test.rs" \
        ! -path "*/tests/*" \
        -exec grep -Hn "\.unwrap()" {} \; 2>/dev/null || true)

    if [ -n "$MATCHES" ]; then
        COUNT=$(echo "$MATCHES" | wc -l)
        TOTAL=$((TOTAL + COUNT))

        echo "$MATCHES" | head -20  # Afficher premiers 20

        if [ $COUNT -gt 20 ]; then
            echo "... et $((COUNT - 20)) autres"
        fi

        # Considérer critique si > 5 unwrap() dans un même fichier
        for file in $(echo "$MATCHES" | cut -d: -f1 | sort | uniq -c | awk '$1 > 5 {print $2}'); do
            echo "⚠️  CRITIQUE: $file contient $(grep -c '\.unwrap()' "$file") unwrap()"
            CRITICAL=$((CRITICAL + 1))
        done
    else
        echo "✅ Aucun unwrap() trouvé"
    fi
done

echo ""
echo "================================================"
echo "📊 RÉSULTATS GLOBAUX"
echo "================================================"
echo "Total unwrap() production : $TOTAL"
echo "Fichiers critiques (>5)   : $CRITICAL"

if [ $TOTAL -gt 10 ]; then
    echo ""
    echo "❌ ÉCHEC : Trop de unwrap() en production (limite: 10)"
    exit 1
elif [ $CRITICAL -gt 0 ]; then
    echo ""
    echo "⚠️  WARNING : Fichiers critiques identifiés"
    exit 0  # Warning mais pas bloquant
else
    echo ""
    echo "✅ SUCCÈS : Conformité unwrap() atteinte"
    exit 0
fi
```

---

## 📜 Script 2 : audit_static_mut.sh

```bash
#!/bin/bash
# scripts/audit_static_mut.sh — Vérifier commentaires SAFETY sur static mut

set -e

echo "🔍 Audit static mut — Commentaires SAFETY requis"
echo "=================================================="

DIRS="kernel/src servers/*/src drivers/*/src libs/*/src"

TOTAL=0
DOCUMENTED=0
UNDOCUMENTED=0

for dir in $DIRS; do
    if [ ! -d "$dir" ]; then
        continue
    fi

    echo ""
    echo "📁 $dir"
    echo "-------------------------------------------"

    # Trouver toutes les déclarations static mut
    while IFS=: read -r file line_num content; do
        TOTAL=$((TOTAL + 1))

        # Vérifier si ligne précédente ou commentaire au-dessus contient "SAFETY:"
        PREV_LINES=$(head -n "$line_num" "$file" | tail -n 5)

        if echo "$PREV_LINES" | grep -qi "SAFETY:"; then
            DOCUMENTED=$((DOCUMENTED + 1))
            echo "✅ L.$line_num: static mut documenté"
        else
            UNDOCUMENTED=$((UNDOCUMENTED + 1))
            echo "❌ L.$line_num: static mut SANS commentaire SAFETY"
            echo "   → $content"
        fi
    done < <(find "$dir" -name "*.rs" -exec grep -Hn "static mut " {} \; 2>/dev/null | sed 's/:/ /' | while read file line rest; do
        echo "$file:$line:$rest"
    done)
done

echo ""
echo "================================================"
echo "📊 RÉSULTATS GLOBAUX"
echo "================================================"
echo "Total static mut      : $TOTAL"
echo "Documentés (SAFETY:)  : $DOCUMENTED"
echo "Non documentés        : $UNDOCUMENTED"

PERCENT=$((DOCUMENTED * 100 / TOTAL))
echo "Conformité            : ${PERCENT}%"

if [ $UNDOCUMENTED -gt 0 ]; then
    echo ""
    echo "❌ ÉCHEC : $UNDOCUMENTED static mut sans commentaire SAFETY"
    exit 1
else
    echo ""
    echo "✅ SUCCÈS : Tous static mut sont documentés"
    exit 0
fi
```

---

## 📜 Script 3 : audit_todo.sh

```bash
#!/bin/bash
# scripts/audit_todo.sh — Détecter TODOs actifs en production

set -e

echo "🔍 Audit TODOs — Code production uniquement"
echo "============================================"

DIRS="kernel/src servers/*/src drivers/*/src libs/*/src"

TOTAL=0
FEATURE_GATED=0
ACTIVE=0

declare -A FILES_WITH_TODOS

for dir in $DIRS; do
    if [ ! -d "$dir" ]; then
        continue
    fi

    echo ""
    echo "📁 $dir"
    echo "-------------------------------------------"

    # Trouver TODOs hors tests
    while IFS=: read -r file line_num content; do
        TOTAL=$((TOTAL + 1))

        # Vérifier si dans bloc #[cfg(feature = "...")]
        # Approche simplifiée : chercher feature gate dans les 10 lignes précédentes
        PREV_LINES=$(head -n "$line_num" "$file" | tail -n 10)

        if echo "$PREV_LINES" | grep -q "#\[cfg(feature"; then
            FEATURE_GATED=$((FEATURE_GATED + 1))
            echo "🔵 L.$line_num: TODO feature-gated"
        else
            ACTIVE=$((ACTIVE + 1))
            echo "🔴 L.$line_num: TODO ACTIF (non feature-gated)"
            echo "   → $content"

            # Tracker fichier
            FILES_WITH_TODOS["$file"]=$((${FILES_WITH_TODOS["$file"]:-0} + 1))
        fi
    done < <(find "$dir" -name "*.rs" \
        ! -name "*_test.rs" \
        ! -path "*/tests/*" \
        -exec grep -Hn "// TODO" {} \; 2>/dev/null)
done

echo ""
echo "================================================"
echo "📊 RÉSULTATS GLOBAUX"
echo "================================================"
echo "Total TODOs             : $TOTAL"
echo "Feature-gated           : $FEATURE_GATED"
echo "Actifs (problématiques) : $ACTIVE"

if [ $ACTIVE -gt 0 ]; then
    echo ""
    echo "📁 Fichiers avec TODOs actifs :"
    for file in "${!FILES_WITH_TODOS[@]}"; do
        echo "   - $file (${FILES_WITH_TODOS[$file]} TODOs)"
    done

    echo ""
    echo "❌ ÉCHEC : $ACTIVE TODOs actifs en production"
    echo "Action requise : Soit implémenter, soit ajouter #[cfg(feature = \"...\")]"
    exit 1
else
    echo ""
    echo "✅ SUCCÈS : Tous TODOs sont feature-gated ou implémentés"
    exit 0
fi
```

---

## 📜 Script 4 : audit_heap_isr.sh

```bash
#!/bin/bash
# scripts/audit_heap_isr.sh — Détecter allocations heap en contexte ISR

set -e

echo "🔍 Audit heap en ISR — Zéro tolérance"
echo "======================================"

# Fichiers ISR critiques
ISR_FILES=(
    "kernel/src/arch/x86_64/irq/dispatch.rs"
    "kernel/src/arch/x86_64/irq/handler.S"
    "kernel/src/arch/x86_64/irq/mod.rs"
)

VIOLATIONS=0

echo ""
echo "Fichiers audités :"
for file in "${ISR_FILES[@]}"; do
    if [ -f "$file" ]; then
        echo "   - $file"
    fi
done

echo ""
echo "Recherche de motifs interdits..."
echo "-------------------------------------------"

for file in "${ISR_FILES[@]}"; do
    if [ ! -f "$file" ]; then
        continue
    fi

    # Motifs interdits
    PATTERNS=(
        "Vec::new"
        "Box::new"
        "vec!\["
        "String::new"
        "format!("
        "HashMap::new"
        "BTreeMap::new"
        "LinkedList::new"
    )

    for pattern in "${PATTERNS[@]}"; do
        MATCHES=$(grep -n "$pattern" "$file" 2>/dev/null || true)

        if [ -n "$MATCHES" ]; then
            echo ""
            echo "❌ VIOLATION dans $file :"
            echo "$MATCHES" | while read line; do
                echo "   L.$line"
            done
            VIOLATIONS=$((VIOLATIONS + $(echo "$MATCHES" | wc -l)))
        fi
    done
done

# Vérifier usage de heapless (recommandé)
echo ""
echo "Vérification alternatives heapless..."
echo "-------------------------------------------"

HEAPLESS_USAGE=$(grep -r "heapless::" kernel/src/arch/x86_64/irq/ 2>/dev/null | wc -l)
echo "✅ Occurrences heapless:: trouvées : $HEAPLESS_USAGE"

if [ $HEAPLESS_USAGE -eq 0 ]; then
    echo "⚠️  WARNING : Aucune utilisation de heapless dans IRQ handlers"
    echo "   Recommandation : utiliser heapless::Vec au lieu de Vec"
fi

echo ""
echo "================================================"
echo "📊 RÉSULTATS GLOBAUX"
echo "================================================"
echo "Violations détectées : $VIOLATIONS"

if [ $VIOLATIONS -gt 0 ]; then
    echo ""
    echo "❌ ÉCHEC CRITIQUE : Allocations heap en contexte ISR"
    echo "Impact : Violation S-01 (TCB), risque de panic en production"
    echo "Correction : Remplacer par heapless::Vec ou tableaux fixes"
    exit 1
else
    echo ""
    echo "✅ SUCCÈS : Zéro allocation heap en ISR"
    exit 0
fi
```

---

## 📜 Script 5 : audit_ordering_relaxed.sh

```bash
#!/bin/bash
# scripts/audit_ordering_relaxed.sh — Vérifier commentaires sur Ordering::Relaxed

set -e

echo "🔍 Audit Ordering::Relaxed — Justifications requises"
echo "===================================================="

DIRS="kernel/src servers/*/src drivers/*/src libs/*/src"

TOTAL=0
COMMENTED=0
UNCOMMENTED=0

for dir in $DIRS; do
    if [ ! -d "$dir" ]; then
        continue
    fi

    echo ""
    echo "📁 $dir"
    echo "-------------------------------------------"

    # Trouver Ordering::Relaxed
    while IFS=: read -r file line_num content; do
        TOTAL=$((TOTAL + 1))

        # Chercher commentaire sur ligne précédente ou même ligne
        PREV_LINE=$(head -n "$line_num" "$file" | tail -n 1)
        SAME_LINE_COMMENT=$(echo "$content" | grep "//" || true)

        # Mots-clés de justification acceptables
        JUSTIFICATIONS=(
            "Relaxed OK"
            "statistique"
            "monotone"
            "metrics"
            "debug"
            "pas de synchro"
            "lecture seule"
            "après init"
        )

        IS_JUSTIFIED=false

        # Vérifier même ligne
        if [ -n "$SAME_LINE_COMMENT" ]; then
            for keyword in "${JUSTIFICATIONS[@]}"; do
                if echo "$SAME_LINE_COMMENT" | grep -qi "$keyword"; then
                    IS_JUSTIFIED=true
                    break
                fi
            done
        fi

        # Vérifier ligne précédente
        if [ "$IS_JUSTIFIED" = false ]; then
            for keyword in "${JUSTIFICATIONS[@]}"; do
                if echo "$PREV_LINE" | grep -qi "$keyword"; then
                    IS_JUSTIFIED=true
                    break
                fi
            done
        fi

        if [ "$IS_JUSTIFIED" = true ]; then
            COMMENTED=$((COMMENTED + 1))
            echo "✅ L.$line_num: justifié"
        else
            UNCOMMENTED=$((UNCOMMENTED + 1))
            echo "❌ L.$line_num: SANS justification"
            echo "   → $content"
        fi
    done < <(find "$dir" -name "*.rs" -exec grep -Hn "Ordering::Relaxed" {} \; 2>/dev/null)
done

echo ""
echo "================================================"
echo "📊 RÉSULTATS GLOBAUX"
echo "================================================"
echo "Total Ordering::Relaxed : $TOTAL"
echo "Justifiés               : $COMMENTED"
echo "Non justifiés           : $UNCOMMENTED"

if [ $TOTAL -gt 0 ]; then
    PERCENT=$((COMMENTED * 100 / TOTAL))
    echo "Conformité              : ${PERCENT}%"
fi

if [ $UNCOMMENTED -gt 10 ]; then
    echo ""
    echo "❌ ÉCHEC : Trop de Ordering::Relaxed non justifiés"
    echo "Seuil acceptable : 10 (tolérance pour code legacy)"
    exit 1
elif [ $UNCOMMENTED -gt 0 ]; then
    echo ""
    echo "⚠️  WARNING : $UNCOMMENTED Ordering::Relaxed non justifiés"
    echo "Action requise : Ajouter commentaires expliquant pourquoi Relaxed est safe"
    exit 0  # Warning mais pas bloquant
else
    echo ""
    echo "✅ SUCCÈS : Tous Ordering::Relaxed sont justifiés"
    exit 0
fi
```

---

## 🔧 Script 6 : audit_complet.sh (Master script)

```bash
#!/bin/bash
# scripts/audit_complet.sh — Exécuter tous les audits en séquence

set -e

SCRIPT_DIR="$(dirname "$0")"

echo "🚀 Exécution de l'audit complet ExoOS"
echo "======================================"
echo ""

PASS=0
FAIL=0
WARN=0

run_audit() {
    local script="$1"
    local name="$2"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📋 AUDIT : $name"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    if bash "$SCRIPT_DIR/$script"; then
        PASS=$((PASS + 1))
        echo ""
        echo "✅ $name : SUCCÈS"
    else
        EXIT_CODE=$?
        if [ $EXIT_CODE -eq 0 ]; then
            PASS=$((PASS + 1))
            echo ""
            echo "✅ $name : SUCCÈS"
        elif [ $EXIT_CODE -eq 1 ]; then
            FAIL=$((FAIL + 1))
            echo ""
            echo "❌ $name : ÉCHEC"
        fi
    fi
}

# Exécuter tous les audits
run_audit "audit_unwrap.sh" "unwrap() production"
run_audit "audit_static_mut.sh" "static mut SAFETY"
run_audit "audit_todo.sh" "TODOs actifs"
run_audit "audit_heap_isr.sh" "Heap en ISR"
run_audit "audit_ordering_relaxed.sh" "Ordering::Relaxed"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 RÉSUMÉ FINAL"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Audits réussis    : $PASS"
echo "Audits échoués    : $FAIL"
echo "Avertissements    : $WARN"
echo ""

if [ $FAIL -gt 0 ]; then
    echo "❌ AUDIT GLOBAL : ÉCHEC"
    echo ""
    echo "Actions correctives requises avant merge :"
    echo "   1. Corriger toutes les violations critiques"
    echo "   2. Re-exécuter audit_complet.sh"
    echo "   3. Obtenir validation 100% conformité"
    exit 1
else
    echo "✅ AUDIT GLOBAL : SUCCÈS"
    echo ""
    echo "Le code respecte les standards ExoOS 100%"
    exit 0
fi
```

---

## 📦 Intégration GitHub Actions

```yaml
# .github/workflows/audit.yml

name: ExoOS Compliance Audit

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:
  compliance-audit:
    runs-on: ubuntu-latest

    steps:
    - name: Checkout code
      uses: actions/checkout@v4

    - name: Setup Rust
      uses: dtolnay/rust-action@stable
      with:
        components: clippy, rustfmt

    - name: Run compliance audits
      run: |
        chmod +x scripts/audit_*.sh
        ./scripts/audit_complet.sh

    - name: Cargo checks
      run: |
        cargo build --release --no-default-features
        cargo test --lib
        cargo clippy -- -D warnings
        cargo fmt --check

    - name: Upload audit report
      if: always()
      uses: actions/upload-artifact@v4
      with:
        name: audit-report
        path: |
          docs/audit/*.md
```

---

## 📈 Métriques de suivi

### Tableau de bord recommandé

| Métrique | Semaine 1 | Semaine 2 | Semaine 3 | Semaine 4 | Cible |
|----------|-----------|-----------|-----------|-----------|-------|
| unwrap() production | 40 | 20 | 5 | 0 | 0 |
| static mut documentés | 0% | 50% | 80% | 100% | 100% |
| TODOs actifs | 15 | 8 | 2 | 0 | 0 |
| Heap en ISR | 2 | 0 | 0 | 0 | 0 |
| Ordering::Relaxed justifiés | 5% | 40% | 80% | 100% | 100% |

---

*Scripts d'audit automatisé — Prêts pour CI/CD*
*Dernière mise à jour : Avril 2026*