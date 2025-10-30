# Benchmarks du Noyau Exo-OS

Ce document explique comment mesurer les performances des différents composants du noyau Exo-OS.

## 📊 Composants Benchmarqués

### 1. **Affichage VGA**
- `clear_screen` : Temps pour effacer l'écran VGA (2000 opérations)
- `write_banner` : Temps pour écrire le banner "EXO-OS KERNEL v0.1.0"

### 2. **Gestion des Interruptions**
- `interrupt_handler` : Temps de traitement d'une interruption (100 itérations)
- `interrupt_disable_enable` : Temps de désactivation/activation des interruptions

### 3. **Ordonnanceur (Scheduler)**
- `context_switch` : Temps de changement de contexte (sauvegarde/restauration de 16 registres)
- `schedule` : Temps d'ordonnancement de tâches (tri de 5 tâches)

### 4. **Gestion de la Mémoire**
- `frame_allocate` : Temps d'allocation de cadres mémoire (100 allocations)
- `page_table_walk` : Temps de promenade dans la table de pages (4 niveaux)
- `heap_alloc` : Temps d'allocation sur le tas (1 KB de données)

### 5. **Appels Système (Syscall)**
- `syscall_dispatch` : Temps de distribution des appels système
- `serial_write` : Temps d'écriture sur le port série (19 octets)

### 6. **Séquence de Démarrage**
- `kernel_boot_sequence` : Temps total de démarrage du noyau (simulation complète)

## 🚀 Comment Lancer les Benchmarks

### 1. Prérequis

Installez les outils nécessaires :

```bash
# Windows (PowerShell)
# Installer Rust et criterion
cargo install criterion

# Linux/macOS
cargo install criterion
```

### 2. Lancer les Benchmarks

```bash
# Dans le répertoire kernel
cd kernel

# Lancer tous les benchmarks
cargo bench

# Lancer un benchmark spécifique
cargo bench --bench kernel_benches -- vga_display

# Lancer avec plus de détails
cargo bench -- --output-format html

# Lancer avec plus d'itérations (plus précis mais plus lent)
cargo bench -- --sample-size 100
```

### 3. Analyser les Résultats

Les résultats seront affichés dans la console et sauvegardés dans :
- `kernel/target/criterion/` : Résultats détaillés (HTML, JSON)
- Console : Résumé des performances

Exemple de sortie :
```
vga_display/clear_screen      time:   [12.345 µs 12.567 µs 12.789 µs]
vga_display/write_banner      time:   [8.901 µs 9.123 µs 9.345 µs]
interrupt/interrupt_handler   time:   [45.123 µs 45.345 µs 45.567 µs]
scheduler/context_switch      time:   [123.45 µs 123.67 µs 123.89 µs]
memory/frame_allocate         time:   [2.345 µs 2.567 µs 2.789 µs]
syscall/syscall_dispatch      time:   [1.234 µs 1.456 µs 1.678 µs]
kernel_boot_sequence          time:   [2.345 ms 2.567 ms 2.789 ms]
```

## 📈 Métriques Mesurées

### Temps d'Exécution
- **Minimum** : Temps le plus rapide enregistré
- **Médiane** : Temps moyen (50% des mesures sont plus rapides)
- **Maximum** : Temps le plus lent enregistré

### Confidence Intervals (IC)
- Les intervalles de confiance à 95% montrent la variabilité des mesures
- Plus l'intervalle est petit, plus les mesures sont consistantes

## 🔧 Configuration Avancée

### Modifier les Benchmarks

Éditez `kernel/src/benches/kernel_benches.rs` pour :
- Ajuster le nombre d'itérations
- Ajouter de nouveaux benchmarks
- Modifier les scénarios de test

### Options de Compilation

```bash
# Benchmarks en mode release (optimisé)
cargo bench --release

# Benchmarks avec profilage
RUSTFLAGS="-C profile-generate=/tmp/profdata" cargo bench --release
llvm-profdata merge /tmp/profdata /tmp/profdata.profdata
llvm-profdata show /tmp/profdata.profdata
```

## 📊 Interprétation des Résultats

### Objectifs de Performance

# 🔥 Benchmarks Kernel Exo-OS - Spécifications Techniques

## 📊 Métriques de Performance du Noyau

### 🎯 **CORE KERNEL - Performances Critiques**

| Composant | Objectif | Méthode de Mesure | Tolérance |
|-----------|----------|-------------------|-----------|
| **Boot time (kernel seul)** | < 800 ms | UEFI timestamp → init_done | ±100 ms |
| **Boot time (kernel + agents)** | < 2.5 s | UEFI → all agents ready | ±300 ms |
| **Kernel binary size** | < 3 MB | `ls -lh exo-kernel.elf` | ±500 KB |
| **Kernel LOC (Rust)** | < 15,000 lignes | `tokei src/kernel/` | ±2,000 |
| **Memory footprint (idle)** | < 64 MB | Kernel heap + stacks | ±16 MB |
| **Memory footprint (agents)** | < 256 MB | Tous agents système actifs | ±64 MB |

---

### ⚡ **IPC (Inter-Process Communication)**

| Métrique | Objectif | Contexte | Baseline |
|----------|----------|----------|----------|
| **IPC latency (local)** | < 1 µs | Ring buffer, même core | Linux pipe: ~2 µs |
| **IPC latency (remote)** | < 5 µs | Entre agents, cross-core | seL4: ~3 µs |
| **IPC throughput** | > 5 GB/s | Zero-copy, mémoire partagée | memcpy: ~30 GB/s |
| **IPC overhead per message** | < 200 ns | Audit léger + token check | |
| **Max messages/sec** | > 1M msg/s | Par canal IPC | |
| **Channel creation** | < 50 µs | Setup ring buffer + perms | |
| **Channel teardown** | < 20 µs | Cleanup + notify peers | |

#### **Tests IPC Spécifiques**

- **Ping-Pong (2 agents)** : < 1 µs round-trip
- **Broadcast (1→N)** : < 5 µs pour N=10 agents
- **Pipeline (A→B→C)** : < 3 µs latency totale
- **Zero-copy transfer** : > 10 GB/s (buffer 1MB)

---

### 🔄 **SCHEDULING & CONTEXT SWITCHING**

| Métrique | Objectif | Détails | Référence |
|----------|----------|---------|-----------|
| **Context switch time** | < 3 µs | Registres + TLB flush | Linux: ~2-4 µs |
| **Scheduler overhead** | < 500 ns | Décision + queue update | |
| **Scheduler latency (EDF)** | < 10 µs | Worst-case deadline miss | |
| **Preemption latency** | < 5 µs | Interrupt → switch | RTOS: ~1-5 µs |
| **Scheduling decisions/sec** | > 100,000 | Throughput scheduler | |
| **Thread creation** | < 100 µs | Stack + TCB + register | |
| **Thread destruction** | < 50 µs | Cleanup + notify | |

#### **Tests Scheduler**

- **Round-robin fairness** : < 1% variance over 1000 switches
- **EDF deadline miss rate** : < 0.1% under 80% load
- **MLFQ adaptation time** : < 10 ms pour détecter I/O-bound
- **CPU affinity switch** : < 8 µs (cross-core migration)

---

### 🔐 **SYSCALLS & SECURITY**

| Métrique | Objectif | Composants | Comparaison |
|----------|----------|------------|-------------|
| **Syscall overhead** | < 2 µs | Entry + capability + audit | Linux: ~1.5 µs |
| **Capability check** | < 500 ns | Table lookup + token verify | seL4: ~200 ns |
| **Audit log write** | < 5 µs | SHA3 + append buffer | |
| **Seccomp filter eval** | < 300 ns | BPF-like ruleset | Linux: ~100 ns |
| **Syscall throughput** | > 500k/s | Par core CPU | |
| **Permission elevation** | < 10 µs | Temporary capability grant | |

#### **Syscalls Critiques**

| Syscall | Latency Target | Notes |
|---------|---------------|-------|
| `send_ipc` | < 1 µs | Fast path sans copie |
| `recv_ipc` | < 1 µs | Futex-based wait |
| `mmap` | < 50 µs | Page table update |
| `fork` | < 200 µs | COW + TCB clone |
| `exec` | < 5 ms | ELF load + relocate |
| `exit` | < 100 µs | Cleanup complet |

---

### 💾 **MEMORY MANAGEMENT**

| Métrique | Objectif | Algorithme | Notes |
|----------|----------|------------|-------|
| **Page fault latency** | < 10 µs | Minor fault (déjà mappé) | |
| **Page fault (major)** | < 500 µs | Disk I/O + map | SSD assumed |
| **TLB flush** | < 2 µs | invlpg + shootdown IPI | |
| **malloc() overhead** | < 1 µs | jemalloc-like allocator | |
| **free() overhead** | < 500 ns | Instant coalesce | |
| **Memory allocation rate** | > 1 GB/s | Sequential allocs | |
| **Fragmentation** | < 10% | Après 1h stress test | |

#### **Tests Mémoire**

- **COW fork** : < 300 µs pour 100MB process
- **mmap anonymous** : < 20 µs pour 4KB
- **mmap file** : < 100 µs + I/O
- **Shared memory setup** : < 50 µs entre 2 agents

---

### 🔧 **AGENT MANAGEMENT**

| Métrique | Objectif | Composants | Impact |
|----------|----------|------------|--------|
| **Agent startup** | < 15 ms | Load + sandbox + IPC setup | Cold start |
| **Agent startup (cached)** | < 5 ms | Manifest cached | Warm start |
| **Agent crash detection** | < 1 ms | Heartbeat timeout | |
| **Agent recovery** | < 100 ms | Snapshot restore | Critical agents |
| **Agent hot-reload** | < 50 ms | Version swap + state migrate | |
| **Manifest parsing** | < 2 ms | TOML/JSON capabilities | |
| **Sandbox creation** | < 10 ms | cgroup + namespace setup | |

---

### 📡 **INTERRUPTS & TIMERS**

| Métrique | Objectif | Hardware | Notes |
|----------|----------|----------|-------|
| **Interrupt latency** | < 10 µs | APIC local interrupt | |
| **IRQ handler overhead** | < 2 µs | Ack + dispatch | |
| **Timer precision** | < 1 µs | HPET/TSC based | |
| **Timer resolution** | 1 µs | Min timer granularity | |
| **Max timers** | > 10,000 | Concurrent active timers | |
| **Timer creation** | < 5 µs | RB-tree insert | |

---

### 🔥 **STRESS TESTS & EXTREMES**

| Test | Objectif | Durée | Critère de Succès |
|------|----------|-------|-------------------|
| **IPC flood** | > 1M msg/s sustained | 60 min | 0 message drops |
| **Fork bomb** | Survive 10k forks | - | Graceful OOM handling |
| **Context switch storm** | > 100k/s | 10 min | < 5% CPU overhead |
| **Memory pressure** | Stable under 95% RAM | 30 min | No OOM kills critical agents |
| **Interrupt storm** | Handle 100k IRQ/s | 5 min | < 20% CPU IRQ handling |
| **Agent churn** | 1000 spawn/kill/s | 10 min | No resource leaks |

---

### 🛡️ **SECURITY & RECOVERY**

| Métrique | Objectif | Scénario | Impact |
|----------|----------|----------|--------|
| **Crash recovery time** | < 100 ms | Agent critique crash | Auto-restart |
| **Kernel panic recovery** | < 5 s | Watchdog + kexec | Full reboot |
| **Audit log integrity** | 100% | Merkle proof check | Post-incident |
| **Exploit mitigation** | 0 bypasses | Fuzzing 10M iterations | ASLR + CFI |
| **Capability revocation** | < 50 µs | Emergency permission drop | |
| **Sandbox escape detect** | < 10 ms | Memory guard violation | Kill agent |

---

### 🎯 **POWER & EFFICIENCY**

| Métrique | Objectif | Platform | Measurement |
|----------|----------|----------|-------------|
| **Idle power** | < 3 W | x86_64 laptop | C-states enabled |
| **Idle power (ARM64)** | < 0.5 W | Edge device | Deep sleep |
| **Wakeup latency** | < 2 ms | C6 → C0 | APIC timer |
| **Frequency scaling** | < 5 ms | P-state transition | |
| **Suspend (S3)** | < 500 ms | Full system suspend | |
| **Resume (S3)** | < 2 s | Back to desktop | |

---

### 📈 **SCALABILITY**

| Métrique | Objectif | Configuration | Notes |
|----------|----------|---------------|-------|
| **Max concurrent agents** | > 1,000 | 16 core system | |
| **Max IPC channels** | > 10,000 | Active channels | |
| **Max CPU cores** | 128 cores | NUMA-aware | |
| **Max RAM** | 1 TB | 64-bit addressing | |
| **Max open files** | > 1M | Per-agent limit: 65k | |
| **Max threads** | > 100k | System-wide | |

---

## 🧪 **MÉTHODOLOGIE DE TEST**

### **Outils de Mesure**

#### Hardware Counters
- **RDTSC** : Cycles CPU (latency < 50ns)
- **PMU** : Cache misses, branch prediction
- **HPET** : High-precision timestamps

#### Software Tracing
- **USDT probes** : Userspace tracing
- **eBPF** : Kernel event filtering
- **Ring buffer** : Lock-free trace log

#### Benchmarking Tools
- **criterion.rs** : Statistical analysis
- **perf** : Linux perf events
- **flamegraph** : Profiling visualization

---

### **Environnement de Test**

#### Bare Metal (Production-like)
- CPU: Intel i7-12700K / AMD Ryzen 7 5800X
- RAM: 32 GB DDR4-3200
- SSD: NVMe PCIe 4.0 (7000 MB/s)
- GPU: NVIDIA RTX 3070 / AMD RX 6800

#### QEMU/KVM (CI/CD)
- vCPU: 8 cores
- RAM: 8 GB
- virtio-blk + virtio-net
- Host: Ubuntu 22.04 LTS

---



