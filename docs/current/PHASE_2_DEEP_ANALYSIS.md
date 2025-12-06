# Phase 2 - Analyse Approfondie Multi-Core (SMP)

**Date**: 6 décembre 2025  
**Version**: Exo-OS v0.5.0 "Linux Crusher"  
**Objectif**: Analyser l'état réel SMP vs. ROADMAP avant de commencer

---

## 🎯 CONSTAT

**LA PHASE 2 (SMP) A DES STRUCTURES MAIS PAS D'IMPLÉMENTATION RÉELLE**

Contrairement à Phase 1 (qui était à 98%), Phase 2 montre:
- Structures définies ✅ 
- Implémentation réelle ❌
- Code TODO/stub partout ⚠️

---

## 📊 État Réel des Composants SMP

| Composant | Structures | Implémentation | Tests | Status |
|-----------|-----------|----------------|-------|--------|
| **SMP Init** | ✅ 30% | ❌ 0% | ❌ | Stub |
| **APIC/x2APIC** | ✅ 60% | ⚠️ 20% | ❌ | Partiel |
| **I/O APIC** | ✅ 40% | ❌ 0% | ❌ | Stub |
| **AP Bootstrap** | ❌ 0% | ❌ 0% | ❌ | Manque |
| **Per-CPU Data** | ✅ 80% | ❌ 0% | ❌ | Stub |
| **IPI** | ✅ 20% | ❌ 0% | ❌ | Stub |
| **CPU Topology** | ✅ 50% | ❌ 0% | ❌ | Stub |
| **Load Balancer** | ✅ 70% | ⚠️ 30% | ❌ | Partiel |

**Conclusion**: Phase 2 à ~25% (structures) mais 0% fonctionnel

---

## 🔍 Analyse Détaillée par Composant

### 1. SMP Initialization

**Fichier**: `kernel/src/arch/x86_64/smp/mod.rs` (366 lignes)

#### ✅ Ce Qui Existe (Structures)

```rust
pub const MAX_CPUS: usize = 64;  // ✅

#[repr(C, align(64))]
pub struct CpuInfo {
    pub id: u8,                              // ✅
    pub state: AtomicU8,                     // ✅
    pub is_bsp: bool,                        // ✅
    pub apic_id: u8,                         // ✅
    pub apic_base: usize,                    // ✅
    pub features: CpuFeatures,               // ✅
    pub context_switches: AtomicUsize,       // ✅
    pub idle_time_ns: AtomicUsize,           // ✅
    pub busy_time_ns: AtomicUsize,           // ✅
}

pub struct SmpSystem {
    cpu_count: AtomicUsize,                  // ✅
    online_count: AtomicUsize,               // ✅
    bsp_id: AtomicU8,                        // ✅
    cpus: [CpuInfo; MAX_CPUS],               // ✅
    initialized: AtomicBool,                 // ✅
}
```

#### ❌ Ce Qui Manque (Implémentation)

```rust
pub fn init() -> Result<(), &'static str> {
    // TODO: Phase 4D
    // 1. Parse ACPI MADT table to get CPU count and APIC IDs
    // 2. Detect BSP (current CPU)
    // 3. Initialize BSP APIC
    // 4. For each AP:
    //    a. Send INIT IPI
    //    b. Wait 10ms
    //    c. Send SIPI IPI with startup code address
    //    d. Wait for AP to set its state to Online
    // 5. Setup per-CPU run queues in scheduler
    
    // For now, just detect BSP  ← STUB!
    let cpu_count = detect_cpu_count();  // ← Returns 1 (hardcoded)
    // ...
}

fn detect_cpu_count() -> usize {
    // TODO Phase 4D: Implement proper ACPI scanning
    1  // ← STUB! Always returns 1 CPU
}
```

**Status**: Structures 80% ✅ | Implémentation 0% ❌

---

### 2. Local APIC / x2APIC

**Fichier**: `kernel/src/arch/x86_64/interrupts/apic.rs` (225 lignes)

#### ✅ Ce Qui Existe (Partiel)

```rust
pub struct LocalApic {
    base_addr: usize,         // ✅
    x2apic_mode: bool,        // ✅
}

impl LocalApic {
    pub fn init(&mut self) {
        // Force x2APIC mode
        if X2Apic::is_supported() {  // ✅ Fonctionne
            X2Apic::enable();         // ✅ Fonctionne
            self.x2apic_mode = true;
            self.set_spurious_interrupt_vector(0xFF);  // ✅
        }
    }
    
    pub fn send_eoi(&self) {
        if self.x2apic_mode {
            unsafe { wrmsr(X2APIC_EOI, 0); }  // ✅ Fonctionne
        }
    }
}

pub struct X2Apic;
impl X2Apic {
    pub fn is_supported() -> bool {
        unsafe {
            let result = __cpuid(1);
            (result.ecx & (1 << 21)) != 0  // ✅ Fonctionne
        }
    }
    
    pub fn enable() {
        unsafe {
            let mut apic_base = rdmsr(IA32_APIC_BASE);
            apic_base |= (1 << 11) | (1 << 10);  // ✅ Fonctionne
            wrmsr(IA32_APIC_BASE, apic_base);
        }
    }
}
```

#### ❌ Ce Qui Manque

```rust
// IPI (Inter-Processor Interrupt) - MANQUE TOTALEMENT!
pub fn send_init_ipi(apic_id: u8) {
    // TODO: Send INIT IPI to AP
}

pub fn send_startup_ipi(apic_id: u8, vector: u8) {
    // TODO: Send SIPI to AP with startup code address
}

// APIC Timer - INCOMPLET
pub fn setup_timer(vector: u8) {
    // Code existe mais n'est jamais appelé
    // Pas intégré avec scheduler
}
```

**Status**: Base 60% ✅ | IPI 0% ❌ | Timer 20% ⚠️

---

### 3. I/O APIC

**Fichier**: `kernel/src/arch/x86_64/interrupts/ioapic.rs`

#### ⚠️ État Actuel

Je dois vérifier si ce fichier existe:

```bash
ls kernel/src/arch/x86_64/interrupts/ioapic.rs
```

Si existe:
- Probablement structures de base
- Pas d'implémentation réelle
- Pas de routing IRQ → Local APIC

**Status Estimé**: Structures 40% | Implémentation 0%

---

### 4. AP Bootstrap (Trampoline)

**Fichier**: `kernel/src/arch/x86_64/boot/trampoline.asm`

#### ❌ État

**MANQUE TOTALEMENT** ou est un stub

Le trampoline code doit:
1. S'exécuter en real mode (16-bit)
2. Passer en protected mode (32-bit)
3. Activer PAE et long mode (64-bit)
4. Charger GDT/IDT
5. Sauter dans code kernel 64-bit
6. Initialiser stack AP
7. Appeler ap_startup() en Rust

**Status**: 0% ❌

---

### 5. Per-CPU Data

**Fichier**: `kernel/src/arch/x86_64/percpu.rs`

#### Vérification Nécessaire

Besoin de voir ce fichier pour déterminer:
- Structure PerCpuData définie ?
- GS segment utilisé ?
- Macros per_cpu!() ?
- Integration avec scheduler ?

**Status Estimé**: Structures 60% | Implémentation 10%

---

### 6. Load Balancer

**Fichier**: `kernel/src/scheduler/core/loadbalancer.rs`

#### ✅ Ce Qui Existe (Structures)

```rust
/// Per-CPU load statistics
#[derive(Debug, Clone, Copy)]
pub struct CpuLoad {
    pub cpu_id: usize,
    pub queue_length: usize,
    pub total_load: u64,
    pub idle_time: u64,
}

pub struct LoadBalancer {
    /// Per-CPU load stats
    cpu_loads: [Mutex<CpuLoad>; MAX_CPUS],
    /// Last balance time
    last_balance: AtomicU64,
    /// Balance interval (ns)
    balance_interval: u64,
}
```

#### ❌ Ce Qui Manque

- Pas de work stealing réel
- Pas d'intégration avec per-CPU run queues
- Pas de migration thread entre CPUs
- Pas d'appel périodique du balancer

**Status**: Structures 70% ✅ | Implémentation 30% ⚠️

---

## 🚧 Ce Qui Doit Être Implémenté (VRAIMENT)

### Priority 1: ACPI MADT Parsing (Critique)

**Fichier**: `kernel/src/arch/x86_64/acpi/madt.rs` (nouveau)

```rust
/// Parse MADT table from ACPI
pub fn parse_madt() -> Result<MadtInfo, &'static str> {
    // 1. Find RSDP (Root System Description Pointer)
    //    - Search EBDA (Extended BIOS Data Area): 0x40E
    //    - Search BIOS area: 0xE0000 - 0xFFFFF
    
    // 2. Validate RSDP signature "RSD PTR "
    
    // 3. Read RSDT/XSDT address
    
    // 4. Find MADT table (signature "APIC")
    
    // 5. Parse MADT entries:
    //    - Type 0: Local APIC (CPU info)
    //    - Type 1: I/O APIC
    //    - Type 2: Interrupt Source Override
    //    - Type 4: NMI
    
    // 6. Return MadtInfo with:
    //    - CPU count
    //    - APIC IDs list
    //    - Local APIC address
    //    - I/O APIC address
}

pub struct MadtInfo {
    pub cpu_count: usize,
    pub apic_ids: [u8; 64],
    pub local_apic_addr: usize,
    pub ioapic_addr: usize,
    pub ioapic_gsi_base: u32,
}
```

**Estimation**: 2-3 jours

---

### Priority 2: AP Bootstrap Trampoline (Critique)

**Fichier**: `kernel/src/arch/x86_64/boot/trampoline.asm` (nouveau)

```nasm
[BITS 16]
section .text.trampoline
align 4096

ap_trampoline_start:
    cli                     ; Disable interrupts
    
    ; Load GDT pointer
    lgdt [ap_gdt_ptr]
    
    ; Enable protected mode
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    
    ; Jump to 32-bit code
    jmp 0x08:ap_protected_mode

[BITS 32]
ap_protected_mode:
    ; Setup segments
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    
    ; Enable PAE
    mov eax, cr4
    or eax, (1 << 5)
    mov cr4, eax
    
    ; Load PML4
    mov eax, [ap_pml4_addr]
    mov cr3, eax
    
    ; Enable long mode
    mov ecx, 0xC0000080
    rdmsr
    or eax, (1 << 8)
    wrmsr
    
    ; Enable paging
    mov eax, cr0
    or eax, (1 << 31)
    mov cr0, eax
    
    ; Jump to 64-bit code
    jmp 0x08:ap_long_mode

[BITS 64]
ap_long_mode:
    ; Setup stack (unique per AP)
    mov rsp, [ap_stack_ptr]
    
    ; Call Rust ap_startup(cpu_id)
    mov rdi, [ap_cpu_id]
    call ap_startup
    
    ; Halt if returns
    hlt

; Data section
ap_gdt_ptr:
    dw ap_gdt_end - ap_gdt - 1
    dq ap_gdt

ap_gdt:
    dq 0                    ; Null descriptor
    dq 0x00AF9A000000FFFF   ; Code segment
    dq 0x00CF92000000FFFF   ; Data segment
ap_gdt_end:

ap_pml4_addr: dq 0
ap_stack_ptr: dq 0
ap_cpu_id: dq 0

ap_trampoline_end:
```

**Fichier Rust**: `kernel/src/arch/x86_64/smp/bootstrap.rs`

```rust
/// AP startup function (called from trampoline)
#[no_mangle]
pub extern "C" fn ap_startup(cpu_id: u64) -> ! {
    // 1. Initialize Local APIC for this CPU
    apic::init();
    
    // 2. Load IDT
    idt::load();
    
    // 3. Setup per-CPU data
    percpu::init(cpu_id as usize);
    
    // 4. Mark CPU as online
    SMP_SYSTEM.cpus[cpu_id as usize].set_state(CpuState::Online);
    SMP_SYSTEM.online_count.fetch_add(1, Ordering::Release);
    
    // 5. Enable interrupts
    unsafe { core::arch::asm!("sti"); }
    
    // 6. Enter idle loop
    loop {
        if let Some(thread) = SCHEDULER.pick_next_thread(cpu_id as usize) {
            SCHEDULER.switch_to(thread);
        } else {
            unsafe { core::arch::asm!("hlt"); }
        }
    }
}

/// Copy trampoline code to low memory (< 1MB for real mode)
pub fn setup_trampoline() -> Result<(), &'static str> {
    const TRAMPOLINE_ADDR: usize = 0x8000;  // 32KB mark
    
    // 1. Get trampoline code
    extern "C" {
        static ap_trampoline_start: u8;
        static ap_trampoline_end: u8;
    }
    
    let trampoline_size = unsafe {
        &ap_trampoline_end as *const _ as usize - 
        &ap_trampoline_start as *const _ as usize
    };
    
    // 2. Copy to low memory
    unsafe {
        let src = &ap_trampoline_start as *const u8;
        let dst = TRAMPOLINE_ADDR as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, trampoline_size);
    }
    
    // 3. Setup data pointers
    unsafe {
        let pml4_addr = (TRAMPOLINE_ADDR + offset_of_pml4) as *mut u64;
        *pml4_addr = read_cr3();
    }
    
    Ok(())
}
```

**Estimation**: 3-4 jours

---

### Priority 3: IPI (Inter-Processor Interrupts)

**Fichier**: `kernel/src/arch/x86_64/interrupts/ipi.rs` (nouveau)

```rust
/// IPI types
#[derive(Debug, Clone, Copy)]
pub enum IpiType {
    Init,           // INIT IPI for AP wakeup
    Startup(u8),    // SIPI with vector
    Reschedule,     // Trigger reschedule
    TlbFlush,       // Flush TLB
    Halt,           // Halt CPU
}

/// Send IPI to specific CPU
pub fn send_ipi(target_apic_id: u8, ipi_type: IpiType) {
    // Use x2APIC ICR (Interrupt Command Register)
    const X2APIC_ICR: u32 = 0x830;
    
    let icr_value = match ipi_type {
        IpiType::Init => {
            // INIT IPI: Delivery mode 101, level assert
            ((target_apic_id as u64) << 32) | (5 << 8) | (1 << 14)
        }
        IpiType::Startup(vector) => {
            // SIPI: Delivery mode 110, vector contains address/16
            ((target_apic_id as u64) << 32) | (6 << 8) | (vector as u64)
        }
        IpiType::Reschedule => {
            // Fixed interrupt with reschedule vector
            ((target_apic_id as u64) << 32) | RESCHEDULE_VECTOR as u64
        }
        IpiType::TlbFlush => {
            ((target_apic_id as u64) << 32) | TLB_FLUSH_VECTOR as u64
        }
        IpiType::Halt => {
            ((target_apic_id as u64) << 32) | HALT_VECTOR as u64
        }
    };
    
    unsafe {
        wrmsr(X2APIC_ICR, icr_value);
    }
}

/// Send IPI to all CPUs except self
pub fn send_ipi_all_but_self(ipi_type: IpiType) {
    for cpu_id in 0..SMP_SYSTEM.cpu_count() {
        let cpu = SMP_SYSTEM.cpu(cpu_id).unwrap();
        if cpu.is_online() && !cpu.is_bsp {
            send_ipi(cpu.apic_id, ipi_type);
        }
    }
}

/// IPI handler vectors
pub const RESCHEDULE_VECTOR: u8 = 0xF0;
pub const TLB_FLUSH_VECTOR: u8 = 0xF1;
pub const HALT_VECTOR: u8 = 0xF2;

/// Reschedule IPI handler
pub extern "x86-interrupt" fn reschedule_ipi_handler(_frame: InterruptStackFrame) {
    apic::send_eoi();
    SCHEDULER.schedule();
}

/// TLB flush IPI handler
pub extern "x86-interrupt" fn tlb_flush_ipi_handler(_frame: InterruptStackFrame) {
    apic::send_eoi();
    unsafe {
        core::arch::asm!("mov rax, cr3; mov cr3, rax", out("rax") _);
    }
}
```

**Estimation**: 2 jours

---

### Priority 4: Per-CPU Run Queues

**Fichier**: `kernel/src/scheduler/core/percpu_queue.rs` (nouveau)

```rust
/// Per-CPU run queue
pub struct PerCpuQueue {
    cpu_id: usize,
    ready_threads: Mutex<VecDeque<Arc<Thread>>>,
    current_thread: AtomicPtr<Thread>,
    idle_time: AtomicU64,
}

impl PerCpuQueue {
    pub fn new(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            ready_threads: Mutex::new(VecDeque::new()),
            current_thread: AtomicPtr::new(core::ptr::null_mut()),
            idle_time: AtomicU64::new(0),
        }
    }
    
    pub fn enqueue(&self, thread: Arc<Thread>) {
        self.ready_threads.lock().push_back(thread);
    }
    
    pub fn dequeue(&self) -> Option<Arc<Thread>> {
        self.ready_threads.lock().pop_front()
    }
    
    pub fn len(&self) -> usize {
        self.ready_threads.lock().len()
    }
    
    pub fn steal_half(&self) -> Vec<Arc<Thread>> {
        let mut queue = self.ready_threads.lock();
        let steal_count = queue.len() / 2;
        queue.drain(..steal_count).collect()
    }
}

/// Global per-CPU queues
pub static PER_CPU_QUEUES: Once<[PerCpuQueue; MAX_CPUS]> = Once::new();

pub fn init() {
    PER_CPU_QUEUES.call_once(|| {
        let mut queues = ArrayVec::new();
        for i in 0..MAX_CPUS {
            queues.push(PerCpuQueue::new(i));
        }
        queues.into_inner().unwrap()
    });
}
```

**Estimation**: 2 jours

---

### Priority 5: Work Stealing Load Balancer

**Mise à jour**: `kernel/src/scheduler/core/loadbalancer.rs`

```rust
impl LoadBalancer {
    /// Balance load across CPUs
    pub fn balance(&self) {
        let cpu_count = SMP_SYSTEM.cpu_count();
        
        // 1. Collect load stats
        let mut loads: Vec<(usize, usize)> = (0..cpu_count)
            .map(|i| (i, PER_CPU_QUEUES[i].len()))
            .collect();
        
        // 2. Sort by load
        loads.sort_by_key(|(_, len)| *len);
        
        // 3. Steal from busiest to give to idlest
        let busiest = loads.last().unwrap().0;
        let idlest = loads.first().unwrap().0;
        
        let busiest_len = loads.last().unwrap().1;
        let idlest_len = loads.first().unwrap().1;
        
        // Only balance if difference > threshold
        if busiest_len > idlest_len + 4 {
            let stolen = PER_CPU_QUEUES[busiest].steal_half();
            for thread in stolen {
                PER_CPU_QUEUES[idlest].enqueue(thread);
            }
            
            // Send reschedule IPI to idlest CPU
            ipi::send_ipi(
                SMP_SYSTEM.cpu(idlest).unwrap().apic_id,
                IpiType::Reschedule
            );
        }
    }
    
    /// Periodic balancing (called from timer)
    pub fn periodic_balance() {
        static LAST_BALANCE: AtomicU64 = AtomicU64::new(0);
        const BALANCE_INTERVAL_MS: u64 = 100;  // 100ms
        
        let now = time::uptime_ms();
        let last = LAST_BALANCE.load(Ordering::Relaxed);
        
        if now - last > BALANCE_INTERVAL_MS {
            LOAD_BALANCER.balance();
            LAST_BALANCE.store(now, Ordering::Relaxed);
        }
    }
}
```

**Estimation**: 1-2 jours

---

## 📋 Plan d'Implémentation Phase 2

### Semaine 1-2: Fondations ACPI + APIC

**Jour 1-3**: ACPI MADT Parsing
- Trouver RSDP
- Parser RSDT/XSDT
- Parser MADT entries
- Extraire CPU count, APIC IDs
- **Livrable**: `parse_madt()` fonctionnel

**Jour 4-5**: IPI (Inter-Processor Interrupts)
- Implémenter INIT/SIPI
- Implémenter IPI reschedule/TLB flush
- Ajouter handlers IDT
- **Livrable**: `send_ipi()` fonctionnel

**Jour 6-7**: AP Bootstrap Trampoline
- Écrire trampoline.asm (16→32→64 bit)
- Copier vers low memory
- Setup data pointers (PML4, stack, CPU ID)
- **Livrable**: Trampoline assemblé

---

### Semaine 3-4: Bootstrap Multi-Core

**Jour 8-10**: AP Initialization
- Implémenter `bootstrap_aps()`
- Pour chaque AP:
  - Send INIT IPI
  - Wait 10ms
  - Send SIPI avec trampoline address
  - Wait for Online state
- **Livrable**: APs bootent et entrent idle loop

**Jour 11-12**: Per-CPU Data Structures
- Implémenter PerCpuQueue
- Modifier scheduler pour utiliser per-CPU queues
- GS segment pour current CPU
- **Livrable**: Chaque CPU a sa propre queue

**Jour 13-14**: Load Balancing
- Work stealing implementation
- Periodic balancing (100ms)
- Thread migration entre CPUs
- **Livrable**: Load balancer fonctionnel

---

### Estimation Totale

**Durée**: 2-3 semaines de travail concentré
**Lignes de code**: ~2000-3000 lignes
**Fichiers**: ~10 nouveaux fichiers

---

## 🎯 Métriques de Validation

### Tests Phase 2

1. **AP Boot Test**
   - Tous les CPUs détectés bootent
   - Chaque AP entre idle loop
   - Pas de panic/deadlock

2. **IPI Test**
   - INIT/SIPI réveillent APs
   - Reschedule IPI fonctionne
   - TLB flush IPI fonctionne

3. **Per-CPU Queue Test**
   - Threads s'exécutent sur différents CPUs
   - Pas de contention locks
   - Migration fonctionne

4. **Load Balancer Test**
   - Work stealing équilibre charge
   - Pas de thread starvation
   - Latence acceptable

5. **Benchmark**
   - Scalability linéaire jusqu'à 4-8 CPUs
   - Overhead IPI < 1000 cycles
   - Context switch reste < 500 cycles

---

## ⚠️ Risques et Difficultés

### Risque 1: ACPI Parsing Complexe

ACPI est notoire pour être mal documenté et plein de corner cases.

**Mitigation**:
- Commencer avec QEMU (ACPI simple)
- Logger toutes les structures trouvées
- Valider checksums rigoureusement

### Risque 2: Trampoline 16-bit Fragile

Code 16-bit real mode est difficile à débugger.

**Mitigation**:
- Tester chaque transition (16→32→64)
- Logger depuis serial dès que possible
- Utiliser QEMU monitor pour inspection

### Risque 3: Race Conditions

SMP introduit des races partout.

**Mitigation**:
- Atomics partout
- Memory barriers explicites
- Tester avec thread sanitizer si possible

### Risque 4: Deadlocks

Locks multiples = risque deadlock.

**Mitigation**:
- Lock ordering strict
- Try-lock avec timeout
- Deadlock detector (Phase 3)

---

## 📊 Conclusion

**Phase 2 nécessite une vraie implémentation from scratch**

**État actuel**:
- Structures: 25% ✅
- Implémentation: 5% ⚠️
- Tests: 0% ❌

**Ce qui doit être fait**:
1. ACPI MADT parsing (2-3 jours)
2. AP bootstrap trampoline (3-4 jours)
3. IPI implementation (2 jours)
4. Per-CPU queues (2 jours)
5. Load balancing (1-2 jours)
6. Tests et validation (2-3 jours)

**Total**: 14-18 jours de travail

**Recommandation**: 
- Commencer par ACPI MADT parsing
- Puis IPI + trampoline ensemble
- Finir avec per-CPU queues et load balancing

**Contrairement à Phase 1** (qui était déjà faite), **Phase 2 est un vrai projet d'implémentation** qui nécessite du temps et de l'attention.

**Prêt à commencer** ? Ou préférer Phase 4 (Optimizations) pour valider les métriques "Linux Crusher" d'abord ?
