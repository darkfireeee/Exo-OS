# Surface publique `arch` — Référence API

Ce document liste les symboles exportés par `kernel/src/arch/mod.rs` et
les façades x86_64 que le reste du noyau est autorisé à consommer.

> **Règle** : les couches supérieures importent `crate::arch::*` ou les
> re-exports publics de `arch::x86_64`. Les sous-modules internes restent
> des détails d’implémentation.

---

## Types fondamentaux

```rust
pub struct ArchInfo {
    pub cpu_count: u32,
    pub has_apic: bool,
    pub has_x2apic: bool,
    pub has_acpi: bool,
    pub page_size: usize,
}
```

| Champ | Description |
|---|---|
| `cpu_count` | Nombre de CPU logiques vus par l’architecture |
| `has_apic` | Présence d’un APIC local |
| `has_x2apic` | Présence du mode x2APIC |
| `has_acpi` | Tables ACPI détectées |
| `page_size` | Taille de page standard |

### Valeur par défaut

```rust
impl Default for ArchInfo {
    fn default() -> Self;
}
```

La valeur par défaut correspond à une machine minimale : 1 CPU, pas d’APIC,
pas d’ACPI, page size à 4096.

---

## Re-exports publics

Depuis `arch/mod.rs` :

```rust
pub use self::x86_64::{
    cpu::features::{CpuFeatures, CPU_FEATURES},
    cpu::tsc::read_tsc,
    halt_cpu,
};
```

### Rôle

- `CpuFeatures` / `CPU_FEATURES` : état de détection CPUID
- `read_tsc()` : lecture du compteur TSC
- `halt_cpu()` : arrêt irréversible du CPU courant

---

## Primitives communes

```rust
pub const PAGE_SIZE: usize = 4096;
pub const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;
pub const MAX_PHYS_ADDR: u64 = (1u64 << 48) - 1;
pub const PAGE_TABLE_LEVELS: usize = 4;
```

```rust
pub fn arch_info() -> ArchInfo;
pub fn spin_delay_cycles(cycles: u64);
pub fn memory_barrier();
pub fn load_fence();
pub fn store_fence();
pub fn invlpg(virt_addr: u64);
pub fn read_cr3() -> u64;
pub unsafe fn write_cr3(cr3_val: u64);
pub fn read_cr2() -> u64;
pub fn read_cr4() -> u64;
pub unsafe fn write_cr4(cr4_val: u64);
pub unsafe fn enable_interrupts();
pub fn disable_interrupts();
pub fn read_rflags() -> u64;
pub fn irq_save() -> u64;
pub fn irq_restore(flags: u64);
pub unsafe fn outb(port: u16, val: u8);
pub unsafe fn inb(port: u16) -> u8;
pub unsafe fn outl(port: u16, val: u32);
pub unsafe fn inl(port: u16) -> u32;
pub fn io_delay();
pub extern "C" fn arch_cpu_relax();
```

### Usage attendu

- `memory/` consomme `read_cr3`, `write_cr3`, `invlpg`
- `scheduler/` consomme `spin_delay_cycles`, `arch_cpu_relax`, `halt_cpu`
- `drivers/` consomme `outb/inb/outl/inl`
- `irq/` consomme `irq_save` / `irq_restore`

---

## Modules x86_64 exportés

Le module `arch::x86_64` expose les sous-systèmes suivants :

```rust
pub mod acpi;
pub mod apic;
pub mod boot;
pub mod boot_display;
pub mod cpu;
pub mod exceptions;
pub mod framebuffer_early;
pub mod gdt;
pub mod idt;
pub mod irq;
pub mod memory_iface;
pub mod paging;
pub mod sched_iface;
pub mod smp;
pub mod spectre;
pub mod syscall;
pub mod time;
pub mod tss;
pub mod vga_early;
pub mod virt;
```

### Lecture rapide

- `boot/` : BSP, parsing bootloader, SMP trampoline
- `cpu/` : CPUID, MSR, TSC, FPU, topologie
- `gdt.rs`, `idt.rs`, `tss.rs` : structures système de base
- `paging.rs` : tables de pages et KPTI
- `syscall.rs` / `exceptions.rs` : entrées kernel depuis userspace / matériel
- `apic/`, `acpi/`, `smp/` : démarrage et orchestration multi-CPU
- `spectre/`, `virt/` : mitigations et paravirtualisation

---

## Primitives de contrôle CPU

```rust
pub fn halt_cpu() -> !;
```

Boucle `cli; hlt` irréversible utilisée dans le panic path et les boucles idle.

```rust
pub fn io_delay();
```

Écriture sur le port `0x80` pour un délai I/O très court.

---

## Règles de conformité associées

| Règle | Conséquence |
|---|---|
| `irq_save` / `irq_restore` | Pas de logique “métier” ici, seulement la mécanique CPU |
| `write_cr3` | `unsafe` car l’appelant garantit la validité de la PML4 |
| `enable_interrupts` | Réservé aux points où IDT, TSS et piles IST sont prêts |
| `memory_barrier` | Barrière architecturale pure, sans politique mémoire |
