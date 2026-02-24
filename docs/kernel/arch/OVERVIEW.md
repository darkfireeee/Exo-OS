# Module arch/x86_64 — Documentation Technique
> Extrait de `kernel/src/arch/x86_64/` et `docs/kernel/arch/arborescence arch.txt`

---

## 1. Rôle de la couche arch

`arch/` est la couche la plus basse du kernel : elle fournit les
**primitives CPU brutes** (GDT, IDT, APIC, paging, SYSCALL, SMP) aux
couches supérieures. Elle ne contient **aucune logique métier** de
processus, scheduler ou IPC.

Constantes globales (`mod.rs`) :
| Constante | Valeur | Description |
|-----------|--------|-------------|
| `PAGE_SIZE` | 4096 | Taille de page standard |
| `KERNEL_BASE` | `0xFFFF_FFFF_8000_0000` | Base de l'espace virtuel noyau |
| `MAX_PHYS_ADDR` | `(1<<48)-1` | 48-bit PA x86_64 |
| `PAGE_TABLE_LEVELS` | 4 | PML4 → PDPT → PD → PT |

---

## 2. Arborescence détaillée

```
arch/
├── mod.rs
├── x86_64/
│   ├── mod.rs              API publique, ArchInfo, arch_info(), halt_cpu()
│   │
│   ├── boot/               Séquences de démarrage
│   │   ├── multiboot2.rs       Parsing Multiboot2 header
│   │   ├── uefi.rs             UEFI boot protocol
│   │   ├── early_init.rs       Init très bas niveau (avant paging)
│   │   ├── memory_map.rs       Traduit E820/UEFI map → memory/ (EMERGENCY-01 en premier)
│   │   └── trampoline_asm.rs   AP trampoline SMP (real mode → 64-bit)
│   │
│   ├── cpu/                Détection et contrôle CPU
│   │   ├── features.rs     CPUID, feature detection (APIC, x2APIC, SSE…)
│   │   ├── msr.rs          Model-Specific Registers (RDMSR/WRMSR)
│   │   ├── fpu.rs          Instructions ASM brutes XSAVE/XRSTOR/FXSAVE
│   │   │                   ⚠️ NE contient PAS la logique d'état → scheduler/fpu/
│   │   ├── tsc.rs          TSC calibration, rdtsc wrapper
│   │   └── topology.rs     CPU topology (cores, HT, NUMA)
│   │
│   ├── gdt.rs              GDT (Global Descriptor Table) + segments Ring 0/3
│   ├── idt.rs              IDT (Interrupt Descriptor Table) 256 vecteurs
│   ├── tss.rs              TSS (Task State Segment) + IST stacks (NMI, #DF)
│   ├── paging.rs           Page tables x86_64 (PML4, CR3, KPTI)
│   │
│   ├── syscall.rs          SYSCALL/SYSRET entry (MSR_STAR, MSR_LSTAR)
│   │                       ➜ Orchestre livraison signaux (RÈGLE SIGNAL-01)
│   │
│   ├── exceptions.rs       #PF #GP #DF #NMI #MC handlers
│   │                       ➜ Second point signal après préemption (RÈGLE SIGNAL-02)
│   │
│   ├── memory_iface.rs     Interface arch → memory/ (alloc_frame, free_frame…)
│   │
│   ├── apic/               Contrôleurs d'interruptions
│   │   ├── local_apic.rs   Local APIC (timer, EOI, IPI send)
│   │   ├── io_apic.rs      I/O APIC (IRQ routing, GSI mapping)
│   │   ├── x2apic.rs       x2APIC mode (>255 CPUs)
│   │   └── ipi.rs          IPI send/receive + batching
│   │
│   ├── acpi/               Parsing tables ACPI
│   │   ├── parser.rs       RSDP → RSDT/XSDT → table discovery
│   │   ├── madt.rs         MADT (SMP topology, APIC IDs)
│   │   ├── hpet.rs         HPET timer (High Precision Event Timer)
│   │   └── pm_timer.rs     ACPI Power Management timer
│   │
│   ├── smp/                Multiprocesseur symétrique
│   │   ├── init.rs         AP startup sequence (INIT + SIPI IPI)
│   │   ├── percpu.rs       Per-CPU data via GS segment (PER_CPU_TABLE)
│   │   └── hotplug.rs      CPU hotplug online/offline
│   │
│   ├── spectre/            Mitigations Spectre/Meltdown
│   │   ├── kpti.rs         KPTI — split page tables user/kernel (CR3 switch)
│   │   ├── retpoline.rs    Retpoline macro pour indirect calls (Spectre v2)
│   │   ├── ssbd.rs         SSBD per-thread (Spectre v4, SSBD MSR)
│   │   └── ibrs.rs         IBRS / IBPB / STIBP mitigations
│   │
│   └── virt/               Détection et support hyperviseur
│       ├── detect.rs       Hypervisor detect via CPUID leaf 0x40000000
│       ├── paravirt.rs     Paravirt ops (KVM clock, PV TLB flush)
│       └── stolen_time.rs  Stolen time accounting (pour ordonnanceur RT)
│
└── aarch64/                Placeholder ARM64 (future)
    └── mod.rs
```

---

## 3. Module `syscall.rs` — ABI et livraison de signaux

### ABI Syscall Exo-OS (Linux-compatible)
| Registre | Rôle |
|----------|------|
| `rax` | Numéro de syscall |
| `rdi, rsi, rdx, r10, r8, r9` | Arguments (1–6) |
| `rcx` | Sauvé par SYSCALL (RIP retour userspace) |
| `r11` | Sauvé par SYSCALL (RFLAGS userspace) |

### Séquence SYSCALL
```
userspace → SYSCALL
  1. CPU sauve RIP→rcx, RFLAGS→r11, switch CS/SS
  2. Kernel : SWAPGS (accès per-CPU via GS)
  3. Dispatch vers handler Rust
  4. [syscall_return_to_user()] ← RÈGLE SIGNAL-01
     → `process::signal::delivery::handle_pending_signals()`
  5. SWAPGS inverse
  6. SYSRET → userspace
```

**KPTI** : le switch CR3 user↔kernel est effectué dans le code ASM de
bas niveau (`switch_asm.s`) ; `syscall.rs` prend en charge la logique
Rust après le passage en mode kernel.

---

## 4. Module `exceptions.rs` — Handlers CPU

### Séquence générique d'exception
```
Ring 3 exception → vecteur IDT
  1. CPU push erreur+RIP+CS+RFLAGS+RSP+SS sur pile kernel
  2. ASM entry : PUSH registres sauvegardés
  3. SWAPGS si provenança Ring 3 (test CS sauvé)
  4. Appel handler Rust
  5. [exception_return_to_user()] ← RÈGLE SIGNAL-02
     → vérifie `signal_pending`, orchestre livraison
  6. SWAPGS inverse
  7. IRETQ
```

**RÈGLE SIGNAL-02** : `exception_return_to_user()` est le **second**
point d'orchestration des signaux (avec `syscall_return_to_user()`).
Après toute exception depuis Ring 3, arch/ est responsable de la
livraison.

---

## 5. Module `smp/percpu.rs` — Données per-CPU

Les données per-CPU sont accédées via le segment `GS` :
```rust
// SAFETY : addr_of! évite UB cast &T → *mut T
let cpu = unsafe { &mut *(PER_CPU_TABLE.0.as_ptr().add(cpu_id) as *mut PerCpuData) };
```

`PER_CPU_TABLE` est un tableau statique de `PerCpuData` indexé par
`cpu_id`. Le pattern `as_ptr().add()` est correct car la table est
initialisée séquentiellement (un CPU à la fois lors du démarrage SMP).

---

## 6. Module `memory_iface.rs` — Interface vers memory/

Pont entre arch/ et le sous-système mémoire :
```rust
// Allocation d'une frame physique
pub fn alloc_frame(flags: AllocFlags) -> Option<Frame>

// Libération — résultat ignoré volontairement (ligne de démarrage)
pub fn free_frame(frame: Frame) {
    let _ = memory::physical::allocator::buddy::free_page(frame);
}
```

---

## 7. Règles de conformité (voir Audit complet)

| Règle | Statut |
|-------|--------|
| SIGNAL-01 — arch/ orchestre livraison syscall | ✅ `syscall.rs` appelle `handle_pending_signals()` |
| SIGNAL-02 — arch/ orchestre livraison exception | ✅ `exceptions.rs` `exception_return_to_user()` |
| FPU — instructions ASM dans cpu/fpu.rs only | ✅ logique état → scheduler/fpu/ |
| KPTI — CR3 switch dans ASM uniquement | ✅ switch avant entrée Rust |
| Per-CPU — GS segment, addr_of! pour init | ✅ percpu.rs corrigé |
