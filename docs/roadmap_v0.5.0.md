# 🚀 Exo-OS v0.5.0 - Roadmap & Objectifs

**Version cible** : v0.5.0 "Stellar Engine"  
**Date de démarrage** : 25 novembre 2025  
**Statut actuel** : v0.4.0 "Quantum Leap" complétée (81% implémentation)  
**Objectif principal** : Atteindre 95%+ d'implémentation avec userspace fonctionnel

---

## 📊 État actuel v0.4.0

### ✅ Fonctionnalités complétées
- **Gestion mémoire** : Frame allocator, heap allocator, page tables
- **Système de temps** : TSC, HPET, RTC, timers POSIX
- **Interruptions** : IDT, GDT, PIC 8259, handlers de base
- **IPC** : Channels, messages, zerocopy
- **Syscalls** : Dispatch table, 50+ syscalls définis
- **Sécurité** : Capabilities, credentials, seccomp/pledge
- **Documentation** : 7 fichiers, 90KB de docs

### ⚠️ À compléter (19% restant)
- **Scheduler** : Thread management incomplet
- **VFS** : Non monté, pas de filesystem actif
- **Drivers** : Keyboard et disk non implémentés
- **Userspace** : Aucun programme utilisateur
- **Tests** : Suite de tests manquante

---

## 🎯 Objectifs v0.5.0

### 1. **Scheduler Multi-Core Complet** (Priorité: CRITIQUE)

#### État actuel
- ✅ Structure `Thread` définie
- ✅ Context switch en assembleur
- ❌ Round-robin non implémenté
- ❌ Pas de support multi-core
- ❌ Priorités statiques uniquement

#### Objectifs v0.5.0
```rust
// Fonctionnalités à implémenter
- [ ] Round-robin scheduler fonctionnel
- [ ] Thread yield/sleep/wake
- [ ] Queues de threads par priorité
- [ ] Load balancing multi-core
- [ ] Migration de threads entre cores
- [ ] Timer tick pour preemption
- [ ] Statistiques par thread (CPU time, context switches)
```

#### Métriques de succès
- 10+ threads concurrent sans deadlock
- Latence de context switch < 5μs
- Fairness ratio > 0.95
- Support de 4+ CPU cores

---

### 2. **Virtual File System (VFS)** (Priorité: CRITIQUE)

#### État actuel
- ✅ Architecture VFS définie
- ✅ Cache inode/dentry
- ❌ Aucun filesystem monté
- ❌ Pas de driver block
- ❌ `/` non initialisé

#### Objectifs v0.5.0
```rust
// Système de fichiers cible
- [ ] Monter initramfs (archive TAR en mémoire)
- [ ] Implémenter TarFS (lecture seule)
- [ ] Opérations: open(), read(), close()
- [ ] Répertoire racine / avec structure
- [ ] Support de /dev/null, /dev/zero
- [ ] Paths absolus et relatifs
```

#### Architecture TarFS
```
/
├── bin/
│   ├── init          # Premier programme userspace
│   └── shell         # Shell interactif
├── dev/
│   ├── null
│   ├── zero
│   └── tty
├── etc/
│   └── config
└── tmp/
```

#### Métriques de succès
- Lecture de 100+ fichiers/seconde
- Cache hit ratio > 90%
- Latence open() < 100μs

---

### 3. **Drivers Essentiels** (Priorité: HAUTE)

#### 3.1 Keyboard Driver
```rust
// Fonctionnalités
- [ ] IRQ handler pour IRQ1 (keyboard)
- [ ] Scan code -> ASCII conversion
- [ ] Buffer circulaire de 256 caractères
- [ ] Support Shift/Ctrl/Alt
- [ ] read() depuis /dev/tty
```

#### 3.2 ATA/IDE Disk Driver
```rust
// Fonctionnalités
- [ ] Détection de disques IDE
- [ ] PIO mode lecture/écriture
- [ ] Block cache (4KB pages)
- [ ] DMA support (optionnel)
- [ ] read_sector(), write_sector()
```

#### Métriques de succès
- Keyboard input latency < 10ms
- Disk read throughput > 5MB/s
- Zéro data corruption

---

### 4. **Premier Programme Userspace** (Priorité: CRITIQUE)

#### État actuel
- ❌ Pas de support ELF loader
- ❌ Pas de user mode transition
- ❌ Pas de binaire userspace

#### Objectifs v0.5.0
```c
// /bin/init - Premier programme
#include <exo.h>

int main() {
    sys_write(1, "Exo-OS v0.5.0 userspace!\n", 26);
    
    // Lancer le shell
    sys_exec("/bin/shell", NULL);
    
    // Loop infini si exec échoue
    while(1) sys_yield();
}
```

#### Shell basique
```rust
// Fonctionnalités shell
- [ ] Prompt interactif "exo> "
- [ ] Commandes: ls, cat, echo, help, reboot
- [ ] Parsing de commandes simple
- [ ] Exécution de binaires /bin/*
```

#### Métriques de succès
- Transition kernel -> user mode réussie
- Au moins 3 programmes userspace fonctionnels
- Shell responsive (< 50ms par commande)

---

### 5. **Tests & Stabilité** (Priorité: MOYENNE)

#### Suite de tests
```rust
// Tests unitaires
- [ ] test_scheduler_fairness()
- [ ] test_memory_allocator()
- [ ] test_filesystem_operations()
- [ ] test_syscall_interface()
- [ ] test_ipc_channels()

// Tests d'intégration
- [ ] test_userspace_execution()
- [ ] test_concurrent_threads()
- [ ] test_file_io_stress()
```

#### Fuzzing & Stress Tests
```bash
# Stress tests cibles
- 100 threads concurrents pendant 60s
- 10000 fichiers ouverts/fermés
- Allocation/free de 1GB en boucle
- 1000 syscalls/seconde par thread
```

---

## ⚡ Techniques d'Optimisation

### 1. **Optimisations Mémoire**

#### A. Copy-on-Write (COW) pour fork()
```rust
// Implémentation
- Partager les pages read-only entre parent/enfant
- Marquer les pages comme COW (bit custom)
- Page fault handler pour copie à l'écriture
- Gain: fork() passe de O(n) à O(1)
```

**Impact attendu** : 
- fork() latency: 5ms → 50μs (100x plus rapide)
- Memory usage: -70% pour processus similaires

#### B. Slab Allocator pour objets kernel
```rust
// Caches spécialisés
- thread_cache: 128B objects (2048 per slab)
- inode_cache: 256B objects (1024 per slab)
- dentry_cache: 64B objects (4096 per slab)
```

**Impact attendu** :
- Allocation latency: 500ns → 50ns (10x)
- Fragmentation: -80%
- Cache line efficiency: +90%

#### C. Huge Pages (2MB) pour heap
```rust
// Configuration
- Heap >= 10MB: utiliser huge pages
- TLB miss reduction: 512x pour grandes allocations
- Automatique via page allocator
```

**Impact attendu** :
- TLB misses: -95% pour gros buffers
- Memory throughput: +40%

---

### 2. **Optimisations Scheduler**

#### A. Per-CPU Run Queues
```rust
// Architecture
struct CpuRunQueue {
    active: [ThreadList; 140],    // 140 niveaux de priorité
    expired: [ThreadList; 140],
    current: *mut Thread,
    idle: *mut Thread,
}

static CPU_QUEUES: [CpuRunQueue; MAX_CPUS] = [...];
```

**Avantages** :
- Pas de contention sur queue globale
- Cache locality: thread reste sur même CPU
- Scaling linéaire jusqu'à 64 cores

#### B. O(1) Scheduler Algorithm
```rust
// Principe
- Bitmap 140 bits pour trouver thread en O(1)
- find_first_bit() utilise instruction BSF (x86)
- Swap active/expired queues quand active vide
```

**Impact attendu** :
- Schedule decision: < 1μs (constant time)
- Context switch overhead: < 2%
- Throughput: 100K+ context switches/sec

#### C. Load Balancing Paresseux
```rust
// Stratégie
- Équilibrage seulement si imbalance > 25%
- Check toutes les 100ms (pas à chaque tick)
- Migration par groupes de 4 threads
```

**Impact attendu** :
- Load variance entre cores: < 10%
- Migration overhead: < 0.5% CPU time

---

### 3. **Optimisations VFS**

#### A. Dentry Cache avec LRU
```rust
// Configuration
- Taille: 16384 entrées (1MB)
- Hash table: 4096 buckets
- LRU aging: éviction après 60s sans accès
```

**Impact attendu** :
- Path lookup: 100μs → 2μs (50x)
- Cache hit ratio: > 95% workloads typiques

#### B. Page Cache unifié
```rust
// Architecture
- Partagé entre VFS et mémoire virtuelle
- 4KB pages alignées
- Write-back avec flush périodique (5s)
```

**Impact attendu** :
- Read throughput: +300% (cached reads)
- Write latency: 1ms → 50μs (async)

#### C. Readahead prédictif
```rust
// Stratégie
- Détection de lecture séquentielle (2+ pages)
- Préchargement de 8-32 pages
- Adaptatif selon hit rate
```

**Impact attendu** :
- Sequential read bandwidth: +250%
- Random read: pas d'impact négatif

---

### 4. **Optimisations Interruptions**

#### A. Interrupt Coalescing
```rust
// Configuration
- Timer: 100Hz au lieu de 1000Hz
- Keyboard: batch de 8 scancodes
- Disk: interrupt par 16KB au lieu de 512B
```

**Impact attendu** :
- Interrupts/sec: 10000 → 1000 (10x reduction)
- CPU overhead: -2% disponible pour userspace

#### B. Threaded IRQ Handlers
```rust
// Architecture
- Top half: minimal work, wake thread
- Bottom half: thread dédié par IRQ
- Priorité temps-réel pour IRQ threads
```

**Impact attendu** :
- IRQ latency: < 10μs (top half)
- Système reste responsive sous charge

---

### 5. **Optimisations Compilateur**

#### A. Link-Time Optimization (LTO)
```toml
[profile.release]
lto = "fat"              # Cross-crate inlining
codegen-units = 1        # Meilleure optimisation
opt-level = 3            # Max optimisations
```

**Impact attendu** :
- Binary size: -15%
- Performance: +5-10% général

#### B. Profile-Guided Optimization (PGO)
```bash
# Workflow
1. cargo build --profile pgo-generate
2. ./run_benchmarks.sh  # Collecter profil
3. cargo build --profile pgo-use

# Gains attendus
- Branch prediction: +40% accuracy
- Instruction cache: -30% misses
- Perf globale: +10-15%
```

#### C. CPU-specific optimizations
```bash
# Target native CPU
RUSTFLAGS="-C target-cpu=native" cargo build --release

# Activer: AVX2, BMI2, POPCNT, etc.
```

**Impact attendu** :
- Mémoire: +20% bandwidth (AVX)
- Crypto: +100% (AES-NI)

---

### 6. **Optimisations Système**

#### A. Zero-Copy I/O
```rust
// Techniques
- sendfile() syscall pour copie kernel-kernel
- mmap() pour éviter read()/write() buffers
- DMA direct vers userspace
```

**Impact attendu** :
- Network/disk I/O: +80% throughput
- CPU usage: -50% pour file transfers

#### B. RCU (Read-Copy-Update)
```rust
// Utilisation
- Structures partagées lecture intensive
- Thread list, routing table, dentry cache
- Pas de locks pour lecteurs
```

**Impact attendu** :
- Scalabilité lecture: linéaire jusqu'à 64 cores
- Contentions: -99%

#### C. Lock-Free Data Structures
```rust
// Implémentations
- MPSC queue pour IPC (sans lock)
- Atomic refcounting partout
- Per-CPU variables
```

**Impact attendu** :
- Scalabilité: > 90% jusqu'à 32 cores
- Tail latency: -60%

---

## 📈 Métriques de Performance Cibles

### Latences
| Opération | v0.4.0 | v0.5.0 Cible | Amélioration |
|-----------|--------|--------------|--------------|
| Context switch | N/A | < 5μs | New |
| Syscall | N/A | < 500ns | New |
| Page fault | ~100μs | < 50μs | 2x |
| open() | N/A | < 100μs | New |
| read() (cached) | N/A | < 2μs | New |
| fork() | N/A | < 50μs (COW) | New |

### Throughput
| Métrique | v0.5.0 Cible |
|----------|--------------|
| Context switches/sec | > 100K |
| Syscalls/sec | > 1M |
| File operations/sec | > 50K |
| Network packets/sec | > 10K |
| Disk I/O (sequential) | > 50MB/s |

### Scalabilité
| Cores | Efficiency Target |
|-------|-------------------|
| 1 | 100% (baseline) |
| 2 | > 95% |
| 4 | > 90% |
| 8 | > 85% |
| 16 | > 75% |

---

## 🛠️ Plan d'Implémentation

### Phase 1 : Scheduler (Semaines 1-2)
```
Jour 1-3:   Round-robin basique, thread_yield()
Jour 4-6:   Priorities, preemption timer
Jour 7-10:  Multi-core, per-CPU queues
Jour 11-14: Load balancing, tests
```

### Phase 2 : VFS (Semaines 3-4)
```
Jour 15-17: TarFS parser, mount root
Jour 18-20: open()/read()/close() syscalls
Jour 21-23: /dev/ devices (null, zero, tty)
Jour 24-28: Tests, optimisations cache
```

### Phase 3 : Drivers (Semaine 5)
```
Jour 29-31: Keyboard driver + IRQ handler
Jour 32-35: ATA/IDE disk driver basique
```

### Phase 4 : Userspace (Semaine 6)
```
Jour 36-38: ELF loader, user mode transition
Jour 39-40: /bin/init program
Jour 41-42: Shell basique
```

### Phase 5 : Tests & Polish (Semaine 7)
```
Jour 43-45: Suite de tests complète
Jour 46-47: Bug fixes
Jour 48-49: Documentation, benchmarks
```

---

## 🎯 Critères de Succès v0.5.0

### Must-Have (Bloquant release)
- ✅ Scheduler round-robin fonctionnel
- ✅ Au moins 1 filesystem monté (TarFS)
- ✅ Keyboard driver opérationnel
- ✅ 1+ programme userspace qui tourne
- ✅ Shell interactif basique
- ✅ 0 erreurs de compilation
- ✅ Boot stable sans crash

### Should-Have (Important)
- ✅ Multi-core scheduler (2+ cores)
- ✅ Disk driver (lecture)
- ✅ 50+ tests unitaires passing
- ✅ Implémentation ≥ 95%

### Nice-to-Have (Bonus)
- ⭐ Network driver virtio-net
- ⭐ 10+ programmes userspace
- ⭐ Benchmarks vs Linux/FreeBSD
- ⭐ Performance profiling tools

---

## 📊 Suivi Progression

### Dashboard Métriques
```rust
// À afficher au boot
[v0.5.0] Implementation: 95%
[v0.5.0] Threads running: 12
[v0.5.0] Filesystems mounted: 1 (tarfs)
[v0.5.0] Drivers loaded: 3 (keyboard, ata, serial)
[v0.5.0] Userspace processes: 2 (init, shell)
[v0.5.0] Uptime: 1234s | Context switches: 456789
```

### Tests Coverage
```bash
# Objectif
- Unit tests: > 80% coverage
- Integration tests: > 60% coverage
- Critical paths: 100% coverage
```

---

## 🔧 Outils de Développement

### Profiling
```bash
# CPU profiling avec perf
perf record -g qemu-system-x86_64 ...
perf report

# Memory profiling
valgrind --tool=massif kernel.bin
```

### Debugging
```bash
# GDB remote debugging
qemu -s -S & gdb kernel.bin
(gdb) target remote :1234
(gdb) break rust_main
```

### Benchmarking
```bash
# Suite de benchmarks
./benchmarks/scheduler_bench.sh
./benchmarks/vfs_bench.sh
./benchmarks/syscall_bench.sh
```

---

## 📝 Documentation v0.5.0

### Nouveaux documents à créer
- [ ] `scheduler_design.md` - Architecture complète
- [ ] `vfs_api.md` - API VFS et drivers filesystem
- [ ] `userspace_abi.md` - ABI et conventions syscall
- [ ] `performance_tuning.md` - Guide optimisations
- [ ] `testing_guide.md` - Comment écrire tests

### Updates documents existants
- [ ] `readme_kernel.txt` - Ajouter stats v0.5.0
- [ ] `readme_memory_and_scheduler.md` - Compléter scheduler
- [ ] `readme_syscall_et_drivers.md` - Ajouter drivers

---

## 🎓 Références Techniques

### Scheduler
- Linux O(1) scheduler (2.6.0-2.6.22)
- FreeBSD ULE scheduler
- "The Linux Scheduler: a Decade of Wasted Cores" (2016)

### VFS
- Linux VFS architecture
- Plan9 filesystem interface
- "The Design and Implementation of the 4.4BSD OS"

### Optimizations
- "What Every Programmer Should Know About Memory" (Drepper)
- "Systems Performance" (Brendan Gregg)
- Intel Optimization Manual (Volume 3)

---

## 🚀 Vision Long-Terme

### v0.6.0 "Cosmic Nexus" (Future)
- Network stack TCP/IP
- SMP scheduling avancé
- ext2/ext4 filesystem support
- Dynamic module loading
- Power management

### v0.7.0 "Galactic Core" (Future)
- Graphical framebuffer driver
- USB stack
- POSIX compatibility layer
- Multi-user support
- Package manager

### v1.0.0 "Universe Engine" (Long-terme)
- Production-ready
- Full POSIX compliance
- Desktop environment
- Self-hosting (compile lui-même)
- Community ecosystem

---

**Prochaine action** : Démarrer Phase 1 - Implémentation Scheduler ⚡

_Document créé le 25 novembre 2025_  
_Version: 1.0_  
_Auteur: Équipe Exo-OS_
