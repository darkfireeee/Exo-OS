# Module `arch` — Architecture bas niveau

## Vue d'ensemble

Le module `arch/` constitue la **façade architecture** du noyau Exo-OS. Il
fournit les primitives CPU, mémoire virtuelle très bas niveau, démarrage BSP/AP,
interruptions, tables de descripteurs, APIC, ACPI, SYSCALL, timekeeping et
mitigations micro-architecturales.

> **Règle** : les couches supérieures n'utilisent que `crate::arch::*` et les
> façades publiques de `arch/x86_64`. Les sous-modules internes restent des
> détails d'implémentation.

---

## Position dans l'architecture en couches

```
┌─────────────────────────────────────────────────────┐
│  Couche 3+  : scheduler/, process/, security/, fs/  │
├─────────────────────────────────────────────────────┤
│  Couche 2   : memory/                               │
├─────────────────────────────────────────────────────┤
│  Couche 1   : arch/               ← CE MODULE       │
├─────────────────────────────────────────────────────┤
│  Couche 0   : matériel CPU / firmware / bootloader   │
└─────────────────────────────────────────────────────┘
```

### Dépendances autorisées

| Dépendance | Usage |
|---|---|
| `memory::core` | `ArchInfo` exporte la taille de page et les infos d’adresse |
| `memory::physical` | `memory_iface` et l’initialisation du PML4 courant |
| `memory::protection` | activation NX/SMEP/SMAP/PKU après boot |
| `scheduler::sync` | uniquement via les façades appelées par `smp` / `irq` |
| `security` | publication de `SECURITY_READY` et mitigations de boot |

### Dépendances interdites

- aucune logique métier de `process/`
- aucune logique VFS / `fs/`
- aucune implémentation de scheduler ici
- aucune gestion d’allocateur de haut niveau

---

## Arborescence des sous-modules

```text
arch/
├── mod.rs                      façade transverse + `ArchInfo`
├── time.rs                     façade de timekeeping commune
├── x86_64/
│   ├── mod.rs                  primitives x86_64 + constantes globales
│   ├── boot/                   BSP, bootloader, trampoline SMP
│   │   ├── multiboot2.rs       parsing Multiboot2
│   │   ├── uefi.rs             parsing UEFI
│   │   ├── early_init.rs       orchestration BSP
│   │   ├── memory_map.rs       cartes mémoire vers `memory/`
│   │   └── trampoline_asm.rs   AP trampoline (16 → 32 → 64 bits)
│   ├── cpu/                    CPUID, MSR, TSC, FPU, topologie
│   ├── gdt.rs                  GDT
│   ├── idt.rs                  IDT
│   ├── tss.rs                  TSS + IST
│   ├── paging.rs               page tables, CR3, KPTI
│   ├── syscall.rs              entrée SYSCALL/SYSRET
│   ├── exceptions.rs           handlers d'exceptions CPU
│   ├── memory_iface.rs         pont vers `memory/`
│   ├── apic/                   LAPIC / IOAPIC / x2APIC / IPI
│   ├── acpi/                   RSDP / MADT / HPET / PM timer
│   ├── smp/                    bootstrap AP + per-CPU
│   ├── spectre/                KPTI / retpoline / SSBD / IBRS
│   ├── virt/                   détection hyperviseur et paravirt
│   ├── irq/                    routage IRQ + helpers
│   ├── boot_display.rs         affichage très précoce
│   ├── framebuffer_early.rs    framebuffer minimal
│   ├── vga_early.rs            sortie VGA simple
│   └── sched_iface.rs          pont ABI vers le scheduler
└── aarch64/
      └── mod.rs                  source de port futur, cible boot non supportee en v0.2.0
```

`x86_64` est la seule cible de boot supportee par le kernel v0.2.0. Un build
kernel non-test sur une autre architecture est bloque par `compile_error!` dans
`kernel/src/lib.rs`; le module `arch/aarch64` reste uniquement un point de
depart pour un portage ulterieur.

---

## Initialisation

Le point d’entrée de la couche architecture est `arch::x86_64::boot::early_init::arch_boot_init()`.

Cette fonction est appelée après le passage du bootloader en mode long 64 bits.
Elle prépare le CPU, les tables système, l’ACPI, l’APIC, la mémoire et le SMP.

Voir [INIT.md](INIT.md) pour la séquence complète.

---

## Constantes clés

| Constante | Valeur | Rôle |
|---|---|---|
| `PAGE_SIZE` | 4096 | Taille de page standard |
| `KERNEL_BASE` | `0xFFFF_FFFF_8000_0000` | Base virtuelle noyau |
| `MAX_PHYS_ADDR` | `(1<<48)-1` | Limite d’adresse physique x86_64 |
| `PAGE_TABLE_LEVELS` | 4 | PML4 → PDPT → PD → PT |

---

## Règles de conformité

| Règle | Résumé |
|---|---|
| SIGNAL-01 | `syscall.rs` orchestre le retour utilisateur après SYSCALL |
| SIGNAL-02 | `exceptions.rs` orchestre le retour utilisateur après exception |
| KPTI | le switch CR3 utilisateur/noyau reste au plus bas niveau |
| FPU | `cpu/fpu.rs` reste limité aux primitives matérielles |
| PER-CPU | les données CPU-locales passent par `GS` et un bootstrap sûr |
| BOOT-SMP | l’AP ne sort jamais du trampoline sans handshake BSP |

---

## Index de la documentation

| Fichier | Contenu |
|---|---|
| [OVERVIEW.md](OVERVIEW.md) | Ce fichier — vue d'ensemble |
| [API.md](API.md) | Surface publique complète |
| [INIT.md](INIT.md) | Séquence de boot et `arch_boot_init()` |
| [BOOT_TRAMPOLINE.md](BOOT_TRAMPOLINE.md) | Trampoline SMP 16 → 32 → 64 bits |
| [arborescence arch.txt](arborescence%20arch.txt) | Arbre des sous-modules |
