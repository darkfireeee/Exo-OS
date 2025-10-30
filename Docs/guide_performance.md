# Guide d'Utilisation des Outils de Performance Exo-OS

## 📊 Vue d'Ensemble

Ce guide explique comment utiliser les nouveaux outils de performance intégrés dans Exo-OS pour mesurer et analyser les performances du noyau.

## 🔧 Outils Disponibles

### 1. **Compteurs de Performance (intégrés au kernel)**
- **Fichier** : `kernel/src/perf_counters.rs`
- **Fonction** : Mesure directe des cycles CPU dans le code du noyau
- **Usage** : Automatique via macros et intégration dans les fonctions critiques

### 2. **Profilage QEMU**
- **Script** : `scripts/profile-kernel.sh`
- **Fonction** : Profilage complet via QEMU avec traces et monitoring
- **Usage** : Tests automatisés avec métriques réelles

### 3. **Tests de Charge**
- **Script** : `scripts/stress-tests.sh`
- **Fonction** : Tests de stress pour chaque composant du noyau
- **Usage** : Scénarios de charge intensive et validation de seuils

---

## 🚀 Utilisation Rapide

### Démarrage Simple
```bash
# 1. Compiler le projet
./scripts/build-iso.sh

# 2. Lancer un profilage de base (30 secondes)
./scripts/profile-kernel.sh

# 3. Lancer tous les tests de charge (10 secondes par test)
./scripts/stress-tests.sh
```

### Résultats
Tous les résultats sont stockés dans :
- **Profilage** : `profile_data/`
- **Tests de charge** : `tests/performance/`

---

## 📈 Guide Détaillé

### 1. Compteurs de Performance (Kernel)

#### Fonctionnalités Intégrées
- **Compteur RDTSC** : Mesure précise des cycles CPU
- **8 Composants** : VGA, Interrupts, Scheduler, Memory, Syscall, IPC, Drivers, KernelBoot
- **Statistiques** : Appels totaux, cycles min/max/moyenne
- **Rapports** : Synthèse et rapports détaillés

#### Utilisation dans le Code
```rust
// Mesure directe
let start = crate::perf_counters::rdtsc();
// ... code à mesurer ...
let end = crate::perf_counters::rdtsc();
crate::perf_counters::PERF_MANAGER.record(
    crate::perf_counters::Component::Vga, 
    end - start
);

// Macro pour fonctions
measure_perf_expr!(Component::Vga, {
    // Code VGA à mesurer
});

// Affichage des résultats
crate::perf_counters::print_summary_report();
```

#### Intégration VGA
```rust
// Dans libutils/display.rs, toutes les fonctions VGA sont automatiquement mesurées
pub fn clear_screen() {
    let start = crate::perf_counters::rdtsc();
    // ... logique VGA ...
    let end = crate::perf_counters::rdtsc();
    crate::perf_counters::PERF_MANAGER.record(
        crate::perf_counters::Component::Vga, 
        end - start
    );
}
```

---

### 2. Profilage QEMU

#### Options Disponibles
```bash
# Profilage de base (30 secondes)
./scripts/profile-kernel.sh

# Avec traces QEMU détaillées
./scripts/profile-kernel.sh --trace

# Profilage long avec sortie personnalisée
./scripts/profile-kernel.sh -t 60 -o /tmp/my_profile --verbose

# Avec options QEMU personnalisées
./scripts/profile-kernel.sh --qemu-opts "-no-hpet -rtc-td-hack"
```

#### Options Principales
| Option | Description | Défaut |
|--------|-------------|--------|
| `-t, --time` | Durée du profilage | 30s |
| `-o, --output` | Répertoire de sortie | `./profile_data` |
| `-v, --verbose` | Mode verbeux | Désactivé |
| `--trace` | Active les traces QEMU | Désactivé |
| `--no-build` | Ignore la compilation | Compiles |

#### Sorties Générées
```
profile_data/
├── build.log              # Log de compilation
├── serial_output.log      # Sortie série complète
├── performance_analysis.log # Analyse automatique
├── summary.txt            # Résumé des métriques
├── qemu_traces.log        # Traces QEMU (si --trace)
└── vga_performance.png    # Graphique (si gnuplot)
```

#### Exemple de Sortie
```
🎉 Profilage terminé!

📊 Résultats dans: profile_data
📄 Rapport principal: profile_data/performance_analysis.log
📋 Résumé: profile_data/summary.txt

🔍 Aperçu des résultats:
========== SYNTHESE DE PERFORMANCE ==========
VGA: 3 appels, 36900 cycles moyen (12.300 µs)
Scheduler: 0 appels, 0 cycles moyen (0.000 µs)
Syscall: 0 appels, 0 cycles moyen (0.000 µs)
Memory: 0 appels, 0 cycles moyen (0.000 µs)
==============================================
```

---

### 3. Tests de Charge

#### Scénarios de Test
| Test | Description | Intensité |
|------|-------------|-----------|
| `memory_stress` | 10,000 allocations mémoire | Élevée |
| `vga_stress` | 5,000 clear_screen VGA | Élevée |
| `interrupt_stress` | 10,000 interruptions simulées | Moyenne |
| `scheduler_stress` | 5,000 context_switch | Élevée |
| `syscall_stress` | 10,000 syscalls | Moyenne |
| `multitask_stress` | 10 threads simultanés | Très élevée |
| `mixed_stress` | Combinaison de tous | Critique |

#### Utilisation
```bash
# Tous les tests (défaut)
./scripts/stress-tests.sh

# Tests spécifiques
./scripts/stress-tests.sh memory_stress vga_stress

# Avec seuils de performance et rapport HTML
./scripts/stress-tests.sh -t -r html

# Tests longs (30s chacun) avec sortie personnalisée
./scripts/stress-tests.sh --duration 30 -o /tmp/stress_results
```

#### Options Principales
| Option | Description | Défaut |
|--------|-------------|--------|
| `-t, --threshold` | Vérifier les seuils de performance | Désactivé |
| `-r, --report` | Format du rapport (html/json/text) | text |
| `--duration` | Durée de chaque test | 10s |
| `--no-qemu` | Préparer seulement (pas d'exécution) | Exécute |

#### Seuils de Performance (configurables)
```bash
PERF_THRESHOLDS=(
    "vga_clear:10000"        # 10k cycles max
    "context_switch:200000"  # 200k cycles max
    "syscall_dispatch:5000"  # 5k cycles max
    "memory_alloc:10000"     # 10k cycles max
    "interrupt_handle:50000" # 50k cycles max
)
```

#### Exemple de Sortie
```
🎉 Profilage terminé!

📊 Résultats dans: tests/performance
📄 Rapport principal: tests/performance/performance_report.html
📋 Résumé: tests/performance/summary.txt

🧪 Lancement du test: memory_stress
[SUCCESS] Test memory_stress terminé
[SUCCESS] Seuil respecté pour memory_stress
🧪 Lancement du test: vga_stress
[SUCCESS] Test vga_stress terminé
[WARNING] Seuil dépassé pour vga_stress: 15000 > 10000
```

---

## 📊 Analyse des Résultats

### Métriques Disponibles

#### Cycles CPU
- **Mesure** : Via instruction RDTSC (Real Time Stamp Counter)
- **Précision** : Cycle CPU unique
- **Conversion** : Estimation en µs (assumant 3 GHz)

#### Composants Mesurés
1. **VGA Display** : Effacement écran, écritures texte
2. **Scheduler** : Changements de contexte, ordonnancement
3. **Syscall** : Distribution et traitement des appels système
4. **Memory** : Allocations, gestion des pages
5. **Interrupts** : Traitement des interruptions
6. **IPC** : Communication inter-processus
7. **Drivers** : Gestion des pilotes
8. **Kernel Boot** : Séquence de démarrage complète

### Interprétation

#### Objectifs de Performance
- **< 10 µs** : Excellent
- **10-50 µs** : Acceptable
- **> 50 µs** : À optimiser

#### Exemple d'Analyse
```
VGA Clear Screen: 12.3 µs (Acceptable)
→ Recommandation: Utiliser des écritures par blocs

Context Switch: 124 µs (À optimiser)  
→ Recommandation: Réduire la sauvegarde des registres
```

---

## 🔧 Configuration Avancée

### Personnalisation des Seuils
Éditez `scripts/stress-tests.sh` pour ajuster `PERF_THRESHOLDS` :

```bash
PERF_THRESHOLDS=(
    "vga_clear:8000"          # Plus strict
    "context_switch:150000"   # Plus permissif
)
```

### Ajout de Composants
Dans `kernel/src/perf_counters.rs` :

```rust
pub enum Component {
    Vga,
    Interrupts,
    Scheduler,
    Memory,
    Syscall,
    Ipc,
    Drivers,
    KernelBoot,
    Network,      // Nouveau composant
    FileSystem,   // Nouveau composant
    Unknown,
}
```

### Integration Continue
Ajoutez à votre CI/CD :

```yaml
# .github/workflows/performance.yml
- name: Performance Tests
  run: |
    ./scripts/stress-tests.sh -t -r json
    ./scripts/profile-kernel.sh -t 60
```

---

## 🐛 Dépannage

### Problèmes Courants

#### QEMU ne démarre pas
```bash
# Vérifier que QEMU est installé
qemu-system-x86_64 --version

# Utiliser KVM (Linux)
./scripts/profile-kernel.sh --qemu-opts "-enable-kvm"
```

#### Pas de données de performance
- Vérifiez que `perf_counters` est dans `lib.rs`
- Assurez-vous que les macros sont importées
- Vérifiez les logs série pour les messages de debug

#### Seuils toujours dépassés
- Ajustez les seuils dans `stress-tests.sh`
- Vérifiez la fréquence CPU réelle de votre système
- Considérez les variations dues à la virtualisation

### Debug Avancé

#### Logs Détaillés
```bash
# Mode verbeux
./scripts/profile-kernel.sh --verbose --trace

# Avec debug QEMU
./scripts/profile-kernel.sh --qemu-opts "-d guest_errors"
```

#### Analyse Manuelle
```bash
# Examiner les logs
less profile_data/serial_output.log

# Extraire les métriques VGA
grep "VGA:" profile_data/serial_output.log

# Analyser les traces QEMU
head -50 profile_data/qemu_traces.log
```

---

## 📚 Ressources Additionnelles

### Outils Externes
- **perf** : Outil Linux de profilage
- **valgrind** : Analyse de performance mémoire
- **QEMU monitor** : Monitoring en temps réel
- **gnuplot** : Génération de graphiques

### Documentation
- `Docs/benchmarks_noyau.md` : Guide des benchmarks
- `Docs/benchmarks_results.md` : Résultats d'exemple
- Code source : `kernel/src/perf_counters.rs`

### Contact
Pour questions/bugs : Consultez les logs et rapportez avec :
```bash
./scripts/profile-kernel.sh -v --trace
# Incluez profile_data/ dans votre rapport
```

---

**Version** : 0.1.0  
**Date** : 30 octobre 2025  
**Exo-OS Performance Suite**