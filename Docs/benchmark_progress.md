# Progression Benchmarks Exo-OS

_Date de création: 2025-11-16_

Ce document suit l'avancement vers les objectifs de performance décrits dans `exo-os-benchmarks.md`.
Il est structuré par phases et métriques. Chaque entrée possède: baseline (mesuré ou estimé), cible, statut, prochaines actions.

## Légende Statut
- ⏳ pending: pas encore instrumenté
- 🧪 measuring: instrumentation en cours
- 🚧 improving: optimisation active
- ✅ achieved: objectif atteint (à revalider périodiquement)
- ⚠ blocked: besoin dépendance / design

## Vue Synthétique (Initialisation)
| ID | Métrique | Baseline (estimée) | Cible | Statut | Prochaines actions |
|----|----------|--------------------|-------|--------|--------------------|
| 3  | IPC ≤64B round-trip | ~1100–1300 cycles (est.) | ≤ 400 | ⏳ pending | Instrumenter 1000 ping-pong (todo #3) |
| 4  | IPC 1KB zero-copy | N/A (non impl.) | ≤ 850 | ⏳ pending | Implémenter descriptor partagé (todo #4) |
| 5  | IPC batch 16×64B | N/A | ≤ 2100 total | ⏳ pending | API batch + bench (todo #5) |
| 6  | Context switch minimal | ~900 cycles (est. actuel) | ≤ 350 | 🧪 measuring | Ajouter rdtsc début/fin (todo #6) |
| 7  | Context + FPU | N/A | ≤ 3100 | ⏳ pending | XSAVE aligné + lazy (todo #7) |
| 8  | Alloc 64B | linked_list_allocator (~45 cycles) | ≤ 10 | ⏳ pending | Cache TLS prototype (todo #8) |
| 9  | Alloc 4KB | ~180 cycles | ≤ 40 | ⏳ pending | Bitmap buddy CLZ (todo #9) |
| 10 | Thread spawn | ~12000–15000 cycles (est.) | ≤ 5000 | ⏳ pending | Mesure 100 spawns (todo #10) |
| 11 | Scheduler pick_next | ~400–500 cycles (CFS-like) | ≤ 100 | ⏳ pending | Hot queue + EMA (todo #11) |
| 12 | Syscall getpid | ~150 cycles | ≤ 50 | ⏳ pending | TLS id fast path (todo #12) |
| 13 | Syscall write 64B | ~2500 cycles | ≤ 700 | ⏳ pending | Zero-copy driver path (todo #13) |
| 14 | Mutex fast path | ~25 cycles | ≤ 12 | ⏳ pending | Optimiste CAS (todo #14) |
| 15 | Mutex contended | ~1800 cycles | ≤ 400 | ⏳ pending | Backoff + futex-like (todo #14) |
| 15b| Network pps 10GbE | N/A | ≥ 15M pps | ⚠ blocked | Driver NIC virtuel requis (todo #15) |
| 16 | Boot time (ms) | ~ >600 ms (est.) | ≤ 300 ms | ⏳ pending | Classer init (critique/différé) (todo #16) |

## Phases
### Phase 1 – Instrumentation Fondamentale
Objectif: établir des baselines reproductibles.
Tâches: #2, #3, #6, #17.
Critères de complétion:
- rdtsc_precise disponible et utilisé par tous les benchmarks
- Sortie sérialisée CSV + identifiant bench
- Chaque métrique « pending » passe à « measuring »

### Phase 2 – IPC & Context Switch
Objectif: atteindre cibles IPC ≤64B et context switch ≤350 cycles.
Tâches: #4, #5, #6 (optimisation), #11 (hot queue partielle).

### Phase 3 – Mémoire & Allocations
Objectif: alloc 64B ≤10 cycles, alloc 4KB ≤40 cycles.
Tâches: #8, #9, ajustements page table si nécessaire.

### Phase 4 – Syscalls & Synchronisation
Objectif: getpid ≤50 cycles, write(64B) ≤700, mutex fast ≤12, contended ≤400.
Tâches: #12, #13, #14.

### Phase 5 – Réseau & I/O Haute Performance
Objectif: driver adaptatif concept + simulation throughput.
Tâches: #15.

### Phase 6 – Boot & Macro Benchmarks
Objectif: boot ≤300 ms, scripts macro (compilation synthétique, faux Nginx).
Tâches: #16, #18.

## Détails Tâches
### #2 Améliorer framework bench
- Ajouter fonction `rdtsc_precise()` (lfence/rdtsc ou cpuid/rdtsc pour serialiser)
- Uniformiser collecte: vector samples, tri partiel ou selection algorithm pour percentiles
- Export: ligne CSV: `BENCH,<name>,<n>,<mean>,<min>,<max>,<p50>,<p95>,<p99>`
- Ajouter timestamp TSC début bench suite

### #3 Mesure baseline IPC 64B
- Boucle ping-pong 1000 round-trips sur canal standard
- Désactiver features fusion_rings pour baseline « slow path »
- Réactiver ensuite fusion_rings et comparer

### #6 Context switch instrumentation
- Wrap appel `context_switch` avec TSC avant/après
- Stocker delta dans buffer statique (max 2048 samples)
- Impression en fin d'init scheduler

### #8 Allocateur thread-local
- TLS cache: tableau freelist par classe (ex: 16,32,64,128)
- Refill depuis allocateur global en batch (ex: 32 blocs)
- Mesure avant/après

(… Les autres suivront au fur et à mesure des phases …)

## Blocages / Risques
- NIC / réseau non implémenté → métriques network pps fictives jusqu'à driver
- XSAVE/XRSTOR nécessite gestion CR4/ XCR0 si étendu → prudence
- FPU usage absent pour l'instant → injection workload de test nécessaire

## Scripts Prévu (#17)
Nom: `scripts/extract-benches.sh`
Fonction: parse log série → CSV `bench_results.csv`
Regex ciblées: `^\[MICROBENCH]` ou `^BENCH,`

## Journal (Changelog)
- 2025-11-16: Document initial créé, tâches listées.

---
_Prochaine mise à jour programmée: après complétion tâche #2 (framework)._