# 🚀 ROADMAP EXO-OS v1.0.0 "LINUX CRUSHER"

**Date de création:** 3 décembre 2025  
**Licence:** GPL-2.0 (compatible avec drivers Linux)  
**Objectif:** Kernel 100% fonctionnel, ZÉRO stub/placeholder  
**Vision:** Écraser Linux sur les métriques de performance clés

---

## 📊 ÉTAT ACTUEL vs OBJECTIF v1.0.0

### Analyse de Maturité par Composant

| Composant | État Actuel | Objectif v1.0.0 | Gap | Priorité |
|-----------|-------------|-----------------|-----|----------|
| **Fusion Rings IPC** | 70% structure | 100% + 347 cycles | 30% | 🔴 P0 |
| **Windowed Switch** | 80% ASM intégré | 100% + 304 cycles | 20% | 🔴 P0 |
| **3-Level Allocator** | 60% structure | 100% + 8 cycles | 40% | 🔴 P0 |
| **Scheduler EMA** | 75% 3-Queue | 100% + 87 cycles | 25% | 🔴 P0 |
| **Memory Virtual** | 20% structures | 100% map/unmap | 80% | 🔴 P0 |
| **VFS tmpfs/devfs** | 40% tmpfs basique | 100% complet | 60% | 🟠 P1 |
| **POSIX-X Syscalls** | 30% I/O basique | 100% ~350 syscalls | 70% | 🟠 P1 |
| **ELF Loader** | 70% parsing OK | 100% + spawn | 30% | 🟠 P1 |
| **fork/exec/wait** | 5% stubs | 100% fonctionnel | 95% | 🟠 P1 |
| **Network TCP/IP** | 10% structures | 100% stack complet | 90% | 🟡 P2 |
| **Drivers (PCI/Net/Blk)** | 20% stubs | 100% + Linux compat | 80% | 🟡 P2 |
| **Security/Capabilities** | 40% framework | 100% + TPM | 60% | 🟡 P2 |
| **SMP Multi-core** | 0% single-core | 100% per-CPU | 100% | 🟠 P1 |
| **Input (Keyboard/Mouse)** | 0% | 100% evdev | 100% | 🟡 P2 |
| **Filesystems réels** | 0% | ext4/FAT32 | 100% | 🟢 P3 |

**Progression globale estimée:** ~35%

---

## 🎯 OBJECTIFS DE PERFORMANCE "LINUX CRUSHER"

### Métriques Cibles vs Linux

| Métrique | Linux | Exo-OS Target | Ratio | Status |
|----------|-------|---------------|-------|--------|
| IPC Latence | 1247 cycles | **347 cycles** | 3.6x ✨ | 🟡 À valider |
| Context Switch | 2134 cycles | **304 cycles** | 7x ✨ | 🟡 À valider |
| Alloc Thread-Local | ~50 cycles | **8 cycles** | 6.25x ✨ | 🟡 À valider |
| Scheduler Pick | ~200 cycles | **87 cycles** | 2.3x ✨ | 🟡 À valider |
| Syscall Fast Path | ~150 cycles | **<50 cycles** | 3x ✨ | 🔴 Non mesuré |
| Boot → Shell | ~15s | **<1s** | 15x ✨ | 🟡 ~2s actuel |
| Memory Footprint | ~40MB min | **<15MB** | 2.7x ✨ | 🟡 ~22MB |

---

## 📅 PLANNING DE DÉVELOPPEMENT

### PHASE 0: Fondations Critiques (4 semaines)
**Objectif:** Kernel qui démarre et préempte correctement

#### Semaine 1-2: Timer + Context Switch Réel
```
□ Timer preemption depuis IRQ0 → schedule()
□ Benchmarks context switch (rdtsc)
□ Validation <500 cycles
□ 3+ threads qui alternent
```

#### Semaine 3-4: Mémoire Virtuelle
```
□ map_page() / unmap_page() fonctionnels
□ TLB flush (invlpg)
□ mmap() anonyme
□ mprotect() pour permissions
□ Page fault handler
```

---

### PHASE 1: Kernel Fonctionnel (8 semaines)
**Objectif:** Premier userspace + syscalls de base

#### Mois 1 - Semaine 1-2: VFS Complet
```
□ tmpfs complet avec read/write/create/delete
□ devfs avec /dev/null, /dev/zero, /dev/console
□ procfs avec /proc/self, /proc/[pid]/
□ sysfs basique
□ Mount/unmount
```

#### Mois 1 - Semaine 3-4: POSIX-X Fast Path
```
□ read/write/open/close → VFS intégré (FAIT partiellement)
□ lseek, dup, dup2
□ pipe() pour IPC
□ getpid/getppid/gettid optimisés
□ clock_gettime haute précision
```

#### Mois 2 - Semaine 1-2: Process Management
```
□ fork() - Clone address space (CoW)
□ exec() - Load ELF et remplacer
□ wait4() / waitpid()
□ exit() avec cleanup
□ Process table complète
```

#### Mois 2 - Semaine 3-4: Signals + Premier Shell
```
□ Signal delivery (SIGKILL, SIGTERM, SIGINT, etc.)
□ sigaction() / signal()
□ kill() syscall
□ Clavier PS/2 driver (IRQ1)
□ /dev/tty fonctionnel
□ Shell basique qui lit/écrit
```

---

### PHASE 2: Multi-core + Networking (8 semaines)
**Objectif:** SMP + Stack réseau fonctionnel

#### Mois 3 - Semaine 1-2: SMP Foundation
```
□ APIC local + I/O APIC
□ BSP → AP bootstrap (trampoline)
□ Per-CPU structures
□ Per-CPU run queues
□ Spinlocks SMP-safe
□ IPI (Inter-Processor Interrupts)
```

#### Mois 3 - Semaine 3-4: SMP Scheduler
```
□ Load balancing entre cores
□ CPU affinity (sched_setaffinity)
□ NUMA awareness (basique)
□ Work stealing
```

#### Mois 4 - Semaine 1-2: Network Stack Core
```
□ Socket abstraction
□ Packet buffers (sk_buff-like)
□ Network device interface
□ Ethernet frame handling
□ ARP protocol
```

#### Mois 4 - Semaine 3-4: TCP/IP
```
□ IPv4 complet (header, checksum, routing)
□ ICMP (ping)
□ UDP complet
□ TCP state machine
□ TCP congestion control (cubic)
□ Socket API (socket, bind, listen, accept, connect)
```

---

### PHASE 3: Drivers Linux + Storage (8 semaines)
**Objectif:** Hardware réel supporté via GPL-2.0 drivers

#### Mois 5 - Semaine 1-2: Driver Framework
```
□ Linux DRM compatibility layer
□ Linux driver shim (struct device, etc.)
□ PCI subsystem complet
□ MSI/MSI-X support
```

#### Mois 5 - Semaine 3-4: Network Drivers
```
□ VirtIO-Net (QEMU) - Pure Rust
□ E1000 wrapper (Linux driver)
□ RTL8139 wrapper
□ Intel WiFi (iwlwifi) wrapper
```

#### Mois 6 - Semaine 1-2: Block Drivers
```
□ VirtIO-Blk (QEMU)
□ AHCI/SATA driver
□ NVMe driver (basique)
□ Block layer (bio/request)
```

#### Mois 6 - Semaine 3-4: Filesystems Réels
```
□ FAT32 (lecture)
□ ext4 (lecture)
□ ext4 (écriture basique)
□ Page cache
```

---

### PHASE 4: Security + Polish (6 semaines)
**Objectif:** Production-ready security

#### Mois 7 - Semaine 1-2: Capabilities Complet
```
□ Capability tokens
□ Rights propagation
□ Capability revocation
□ Per-process capability tables
```

#### Mois 7 - Semaine 3-4: Isolation
```
□ Seccomp-BPF like filtering
□ Namespaces (PID, mount, network)
□ Memory protection (NX, ASLR, stack canaries)
□ LSM-like hooks
```

#### Mois 8 - Semaine 1-2: Crypto + TPM
```
□ ChaCha20-Poly1305
□ BLAKE3
□ TPM 2.0 interface
□ Sealed storage
□ Remote attestation (basique)
```

---

### PHASE 5: Performance Tuning (4 semaines)
**Objectif:** Atteindre les métriques "Linux Crusher"

#### Mois 8 - Semaine 3-4: Benchmarking
```
□ Microbenchmarks IPC (rdtsc)
□ Microbenchmarks context switch
□ Microbenchmarks allocator
□ Syscall latency profiling
□ Comparaison avec Linux
```

#### Mois 9 - Semaine 1-2: Optimization
```
□ Hot path tuning (inline, prefetch)
□ Cache line alignment
□ Lock contention reduction
□ Memory layout optimization
□ Compiler flags optimization
```

---

## 📋 DÉTAIL DES IMPLÉMENTATIONS PAR COMPOSANT

### 1. FUSION RINGS IPC (347 cycles target)

**Fichiers existants:**
- ✅ `kernel/src/ipc/fusion_ring/ring.rs` - Structure ring
- ✅ `kernel/src/ipc/fusion_ring/slot.rs` - Slots 64 bytes
- ✅ `kernel/src/ipc/fusion_ring/inline.rs` - Fast path ≤40B
- ✅ `kernel/src/ipc/fusion_ring/zerocopy.rs` - Zero-copy >40B
- ✅ `kernel/src/ipc/fusion_ring/batch.rs` - Batch processing
- ✅ `kernel/src/ipc/core/ultrafast.rs` - UltraFastRing

**À implémenter:**
```rust
// Benchmark réel avec rdtsc
pub fn benchmark_ipc() -> IpcBenchResults {
    let ring = FusionRing::new(256);
    let start = rdtsc();
    
    for _ in 0..10000 {
        ring.send_inline(b"test message");
        ring.recv_inline(&mut buffer);
    }
    
    let elapsed = rdtsc() - start;
    IpcBenchResults {
        cycles_per_roundtrip: elapsed / 10000,
        target: 347,
    }
}
```

**Optimisations requises:**
- [ ] Cache line padding (64 bytes alignment)
- [ ] Prefetch instructions
- [ ] Memory barriers optimization
- [ ] Lock-free improvements

---

### 2. WINDOWED CONTEXT SWITCH (304 cycles target)

**Fichiers existants:**
- ✅ `kernel/src/scheduler/switch/windowed.rs` - ASM via global_asm!
- ✅ `kernel/src/scheduler/switch/fpu.rs` - Lazy FPU
- ✅ `kernel/src/scheduler/switch/simd.rs` - SIMD state

**À implémenter:**
```rust
// Intégration timer → switch
// kernel/src/arch/x86_64/interrupts.rs
extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    crate::time::tick();
    
    // Preemption tous les 10ms
    if crate::time::ticks() % PREEMPT_TICKS == 0 {
        crate::scheduler::schedule();
    }
    
    unsafe { pic::end_of_interrupt(0x20); }
}

// Benchmark
pub fn benchmark_context_switch() -> u64 {
    let start = rdtsc();
    for _ in 0..1000 {
        yield_now();
    }
    (rdtsc() - start) / 2000  // 2 switches per yield
}
```

---

### 3. 3-LEVEL ALLOCATOR (8 cycles target)

**Fichiers existants:**
- ✅ `kernel/src/memory/heap/hybrid_allocator.rs`
- ✅ `kernel/src/memory/heap/thread_cache.rs`
- ✅ `kernel/src/memory/heap/cpu_slab.rs`
- ✅ `kernel/src/memory/heap/size_class.rs`

**À optimiser:**
```rust
// Thread-local fast path (target: 8 cycles)
#[inline(always)]
pub fn thread_alloc_fast(size: usize) -> *mut u8 {
    // TLS access
    let cache = THREAD_CACHE.get();
    
    // Size class lookup (compile-time if possible)
    let class = SizeClass::from_size(size);
    
    // Pop from free list (1-2 cycles)
    if let Some(ptr) = cache.pop(class) {
        return ptr;
    }
    
    // Slow path: refill from CPU slab
    thread_alloc_slow(size)
}
```

---

### 4. POSIX-X SYSCALLS COMPLETS

**État actuel (existant):**
```
kernel/src/posix_x/syscalls/
├── fast_path/     ✅ getpid, gettid, clock_gettime (stubs fonctionnels)
├── hybrid_path/   ✅ read/write/open/close (VFS intégré)
└── legacy_path/   ❌ fork/exec (ENOSYS stubs)
```

**À implémenter (priorité):**

#### Fast Path (~70% des appels)
```rust
// Déjà fait partiellement
sys_getpid()      ✅
sys_gettid()      ✅
sys_getuid()      ✅
sys_clock_gettime() ✅ (à optimiser)
sys_gettimeofday()  □
sys_time()          □
```

#### Hybrid Path (~25% des appels)
```rust
// I/O de base - partiellement fait
sys_read()        ✅ VFS intégré
sys_write()       ✅ VFS intégré
sys_open()        ✅ VFS intégré
sys_close()       ✅ VFS intégré
sys_lseek()       ✅

// À implémenter
sys_pipe()        □  // Critique pour shell
sys_dup()         □
sys_dup2()        □
sys_fcntl()       □
sys_ioctl()       □ (stub existe)
sys_stat()        □
sys_fstat()       □
sys_mkdir()       □
sys_rmdir()       □
sys_unlink()      □
sys_rename()      □
sys_readdir()     □
sys_mmap()        □ // Critique
sys_munmap()      □
sys_mprotect()    □
sys_brk()         □

// Sockets
sys_socket()      □
sys_bind()        □
sys_listen()      □
sys_accept()      □
sys_connect()     □
sys_send()        □
sys_recv()        □
sys_sendto()      □
sys_recvfrom()    □
```

#### Legacy Path (~5% des appels)
```rust
// Process - stubs ENOSYS actuels
sys_fork()        ❌ ENOSYS → À implémenter
sys_vfork()       ❌ ENOSYS → À implémenter  
sys_clone()       ❌ ENOSYS → À implémenter
sys_execve()      ❌ ENOSYS → À implémenter
sys_exit()        □
sys_wait4()       □
sys_waitpid()     □
sys_kill()        □

// Signals
sys_rt_sigaction()    □
sys_rt_sigprocmask()  □
sys_rt_sigreturn()    □
```

---

### 5. FORK/EXEC/WAIT - IMPLÉMENTATION COMPLÈTE

**fork() - Clone de processus:**
```rust
pub fn sys_fork() -> i64 {
    let current = current_process();
    
    // 1. Créer nouveau processus
    let child_pid = alloc_pid();
    let child = Process::new(child_pid, current.pid);
    
    // 2. Clone address space (Copy-on-Write)
    let child_mm = current.mm.clone_cow()?;
    child.mm = child_mm;
    
    // 3. Clone file descriptors
    child.files = current.files.clone();
    
    // 4. Clone signal handlers
    child.signals = current.signals.clone();
    
    // 5. Créer thread enfant
    let child_thread = current_thread().clone_for_fork();
    child_thread.set_return_value(0); // Child returns 0
    
    // 6. Ajouter au scheduler
    SCHEDULER.add_thread(child_thread);
    
    child_pid as i64 // Parent returns child PID
}
```

**exec() - Remplacement de programme:**
```rust
pub fn sys_execve(filename: usize, argv: usize, envp: usize) -> i64 {
    let path = read_user_string(filename)?;
    
    // 1. Lire le fichier ELF
    let elf_data = vfs::read_file(&path)?;
    
    // 2. Parser et valider ELF
    let loaded = load_elf(&elf_data, None)?;
    
    // 3. Créer nouvel address space
    let new_mm = AddressSpace::new()?;
    
    // 4. Mapper les segments ELF
    for segment in loaded.segments {
        new_mm.map_segment(&segment, &elf_data)?;
    }
    
    // 5. Setup stack avec argv/envp
    let stack = setup_user_stack(argv, envp)?;
    new_mm.map_stack(stack)?;
    
    // 6. Remplacer l'ancien address space
    let current = current_process();
    current.mm.destroy();
    current.mm = new_mm;
    
    // 7. Reset signals
    current.signals.reset_on_exec();
    
    // 8. Sauter à l'entry point
    jump_to_user(loaded.entry_point, stack.sp)
}
```

---

### 6. NETWORK STACK TCP/IP

**Structure à implémenter:**
```
kernel/src/net/
├── core/
│   ├── socket.rs       □ Socket abstraction
│   ├── buffer.rs       □ sk_buff equivalent
│   └── device.rs       □ Network device trait
├── ethernet/
│   └── mod.rs          □ Ethernet frames
├── ip/
│   ├── ipv4.rs         □ IPv4 complete
│   ├── routing.rs      □ Routing table
│   └── icmp.rs         □ ICMP (ping)
├── tcp/
│   ├── state.rs        □ TCP state machine
│   ├── connection.rs   □ TCB management
│   └── congestion.rs   □ Congestion control
├── udp/
│   └── mod.rs          □ UDP complete
└── socket_api.rs       □ BSD socket API
```

**Implémentation TCP State Machine:**
```rust
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
}

impl TcpConnection {
    pub fn handle_packet(&mut self, packet: &TcpPacket) {
        match (self.state, packet.flags) {
            (Closed, _) => self.send_rst(),
            (Listen, SYN) => {
                self.send_syn_ack();
                self.state = SynReceived;
            }
            (SynReceived, ACK) => {
                self.state = Established;
            }
            // ... autres transitions
        }
    }
}
```

---

### 7. DRIVERS LINUX (GPL-2.0)

**Structure d'intégration:**
```
kernel/
├── third_party/           # Code Linux adapté
│   ├── drm/              # DRM subsystem
│   │   ├── i915/         # Intel GPU
│   │   └── amdgpu/       # AMD GPU
│   ├── wifi/
│   │   ├── iwlwifi/      # Intel WiFi
│   │   └── ath10k/       # Qualcomm
│   └── bluetooth/
│       └── btusb.c
│
└── src/drivers/          # Wrappers Rust
    ├── gpu/
    │   ├── drm_compat.rs # Shim layer
    │   └── i915_wrapper.rs
    └── net/
        └── wifi/
            └── iwlwifi_wrapper.rs
```

**Shim Layer Pattern:**
```rust
// kernel/src/drivers/gpu/drm_compat.rs

/// Linux struct device equivalent
#[repr(C)]
pub struct Device {
    pub name: *const c_char,
    pub driver: *mut Driver,
    // ... compatible with Linux
}

/// Bridge: Exo-OS → Linux driver
extern "C" {
    fn i915_init() -> c_int;
    fn i915_gem_create(dev: *mut Device, size: u64) -> c_int;
}

/// Safe Rust wrapper
pub fn intel_gpu_init() -> Result<(), DriverError> {
    let ret = unsafe { i915_init() };
    if ret == 0 { Ok(()) } else { Err(DriverError::InitFailed) }
}
```

---

## 🔧 OUTILS ET INFRASTRUCTURE

### Build System
```bash
# Build complet v1.0.0
./scripts/build_complete.sh

# Tests unitaires
cargo test --package kernel

# Benchmarks
cargo bench --package kernel

# Coverage
cargo llvm-cov --package kernel
```

### CI/CD Pipeline
```yaml
# .github/workflows/ci.yml
jobs:
  build:
    - cargo build --release
    - cargo test
    - cargo clippy
    - cargo fmt --check
    
  benchmark:
    - cargo bench
    - Compare with Linux baseline
    
  integration:
    - Boot in QEMU
    - Run test suite
    - Check performance metrics
```

### Documentation
```
docs/
├── ROADMAP_v1.0.0_LINUX_CRUSHER.md  # Ce fichier
├── ARCHITECTURE_v1.0.0.md           # Architecture finale
├── BENCHMARKS.md                    # Résultats benchmarks
├── SYSCALL_REFERENCE.md             # Liste syscalls
└── DRIVER_PORTING_GUIDE.md          # Guide drivers Linux
```

---

## 📈 MÉTRIQUES DE SUCCÈS v1.0.0

### Critères de Release

| Critère | Exigence | Validation |
|---------|----------|------------|
| **Zéro ENOSYS** | Tous syscalls implémentés | Tests automatisés |
| **Boot stable** | 100 boots sans crash | CI nightly |
| **IPC < 400 cycles** | Mesuré avec rdtsc | Benchmark suite |
| **Switch < 350 cycles** | Mesuré avec rdtsc | Benchmark suite |
| **fork/exec** | Fonctionnel | Test process creation |
| **Network** | TCP connection works | Test HTTP GET |
| **Filesystem** | ext4 lecture | Mount + read file |
| **Multi-core** | 4+ cores | SMP stress test |
| **musl libc** | "Hello World" compile | Cross-compile test |

### Checklist Finale v1.0.0

```
□ Kernel boot < 1 seconde
□ Shell interactif fonctionnel
□ Programme "hello world" en C (musl)
□ Network ping fonctionnel
□ File I/O sur tmpfs
□ fork() + exec() fonctionnels
□ Signals POSIX de base
□ Multi-core scheduling
□ Benchmarks documentés
□ Documentation complète
□ Zéro panic en utilisation normale
```

---

## 🗓️ CALENDRIER RÉSUMÉ

| Phase | Durée | Objectif |
|-------|-------|----------|
| **Phase 0** | Semaines 1-4 | Timer + Mémoire Virtuelle |
| **Phase 1** | Semaines 5-12 | VFS + POSIX-X + fork/exec |
| **Phase 2** | Semaines 13-20 | SMP + Network |
| **Phase 3** | Semaines 21-28 | Drivers + Storage |
| **Phase 4** | Semaines 29-34 | Security |
| **Phase 5** | Semaines 35-38 | Performance Tuning |

**Durée totale estimée:** ~9-10 mois

---

## 🎖️ ÉQUIPE ET RESSOURCES

### Rôles Suggérés
- **Kernel Core**: Scheduler, Memory, IPC
- **Userspace**: POSIX-X, Syscalls, ELF loader
- **Drivers**: Linux compat, Network, Block
- **Security**: Capabilities, Crypto, TPM
- **AI Integration**: SmolagentS orchestration

### Ressources Externes (GPL-2.0)
- Linux kernel sources pour drivers
- musl libc pour userspace
- QEMU pour testing
- Benchmark suites (lmbench, sysbench)

---

**🚀 OBJECTIF FINAL: Un kernel qui ÉCRASE Linux sur les métriques de performance tout en maintenant la compatibilité POSIX.**

*"Performance is not an afterthought, it's the foundation."*
