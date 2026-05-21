# Séquence de Boot Kernel ExoOS v0.2.0

**Auteur :** claude iota  
**Date :** 2026-05-20  
**Statut :** Document de référence — à valider lors de la revue de code v0.2.0

> Ce document décrit la séquence de boot **attendue** après application de toutes
> les corrections CORR-IOTA. Il sert de référence pour les invariants de boot,
> les tests de non-régression, et la rédaction des tests TLA+.

---

## Vue d'Ensemble : Deux Phases de Boot

```
Entrée BIOS/UEFI
       │
       ▼
┌─────────────────────────────────────────────────────────────────────┐
│  PHASE A — arch_boot_init() [early_init.rs]                         │
│  Exécution avant heap. Purement hardware et mémoire physique.       │
│                                                                     │
│  A-01  GDT, IDT, TSS                                                │
│  A-02  Serial UART (debug console)                                  │
│  A-03  Memory map parsing (E820)                                    │
│  A-04  Boot page tables (1 GiB identity map)                        │
│  A-05  PHYSMAP étendue via install_extended_physmap()               │
│         → set_physmap_limit() mis à jour                            │
│  A-06  ACPI parser (RSDP → XSDT → MADT → HPET)                    │
│         → accepte maintenant adresses > 1 GiB (CORR-IOTA-03)       │
│  A-07  LAPIC init (MSR IA32_APIC_BASE)                              │
│  A-08  IO-APIC init                                                 │
│  A-09  HPET calibration TSC                                         │
│  A-10  SYSCALL/SYSRET MSRs BSP (STAR, LSTAR, SFMASK)               │
│  A-11  IBRS, SSBD mitigations BSP                                   │
│  A-12  SMEP + SMAP + KPTI (apply_mitigations_bsp())                 │
│  A-13  security_init() — ExoCage, ExoSeal, ExoNMI, ExoLedger        │
│         [premier appel — is_security_ready() → false]               │
│  A-14  Transition vers kernel_init()                                 │
└─────────────────────────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────────────────────────┐
│  PHASE B — kernel_init() [lib.rs]                                   │
│  Exécution avec heap disponible. Initialisation des sous-systèmes.  │
│                                                                     │
│  B-2a  EmergencyPool (128 KiB statique pre-heap)                    │
│  B-2b  Heap allocateur (LockedBumpAllocator → SlabAllocator)        │
│  B-2c  CoW setup                                                    │
│  B-2d  LAPIC fixmap (re-mappage virtuel)                            │
│  B-2e  time_init (HPET, TSC, RTC)                                   │
│  B-2f  drivers::init()                                              │
│         └─ iommu_init()          ← ExoShield IOMMU                  │
│         └─ pci_enumerate()       ← PCI topology                     │
│         └─ dma_init()            ← DMA engine                       │
│                                                                     │
│  B-3   scheduler::init()                                            │
│         └─ cgroup::init()        ← ROOT CGROUP EN PREMIER            │
│         └─ runqueue_init()       ← APRÈS cgroup (CORR-IOTA-02)      │
│         └─ timer::init()                                            │
│         └─ idle::create() × N    ← idle threads attachés au cgroup  │
│         └─ fork::init()                                              │
│                                                                     │
│  B-4   process::init()                                              │
│         └─ pid::init()                                               │
│         └─ registry::init()                                          │
│         └─ maps::init()                                              │
│         └─ futex::init()                                             │
│         └─ acl::init()                                               │
│         [cgroup::init() SUPPRIMÉ ici — voir B-3]                    │
│                                                                     │
│  B-5   security_init()                                              │
│         [is_security_ready() == true → skip si déjà fait en A-13]   │
│                                                                     │
│  B-6   ipc::init()                                                  │
│         └─ ring_buffer::init()                                       │
│         └─ endpoint_registry::init()                                 │
│                                                                     │
│  B-7   fs::init()                                                   │
│         └─ exofs_init()                                              │
│              └─ init_global_disk() ← BAR PCI dynamique              │
│                                     (CORR-IOTA-01)                  │
│                                                                     │
│  B-8   exophoenix::init()                                           │
│         └─ validate_ssr_region()  ← vérifie E820 + heap overlap      │
│         └─ ssr_map_virt()         ← mappe la SSR en virtuel         │
│         └─ arm_watchdog()         ← NMI watchdog armé               │
│                                                                     │
│  B-9   SMP — spawn APs                                              │
│         └─ Pour chaque AP :                                          │
│              ├─ SYSCALL MSRs      ← AVANT STI                       │
│              ├─ apply_mitigations_ap()                               │
│              ├─ TCB idle bootstrap                                   │
│              └─ STI                                                  │
│                                                                     │
│  B-10  userspace_boot::spawn_init_server()                          │
│         └─ Ring0 → Ring3 transition                                  │
│         └─ init_server (PID 1) lancé                                 │
└─────────────────────────────────────────────────────────────────────┘
       │
       ▼
┌─────────────────────────────────────────────────────────────────────┐
│  PHASE C — init_server (Ring3, PID 1)                               │
│  Démarrage des Ring1 servers par vagues parallèles.                 │
│                                                                     │
│  Vague 1 (sans dépendances) :                                       │
│    ipc_router, scheduler_server                                      │
│                                                                     │
│  Vague 2 (dépendent de ipc_router) :                               │
│    tty_server, memory_server, crypto_server                          │
│                                                                     │
│  Vague 3 (dépendent de vague 2) :                                   │
│    vfs_server, device_server, ipc_router_ext                         │
│                                                                     │
│  Vague 4 :                                                          │
│    exo_shield, virtio_drivers                                        │
│                                                                     │
│  Vague 5 (dépendent de exo_shield) :                               │
│    network_server (non critique), exosh                              │
│                                                                     │
│  Temps total cible : < 2 secondes (boot à exosh prompt)             │
│  Temps recovery Phoenix : < 500ms (vagues 3→5 uniquement)           │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Détail : security_init() — Ordre Interne

```
security_init() [kernel/src/security/mod.rs]
│
│  Guard : if is_security_ready() { return; }
│
├─ Step 1   capabilities::cap_table_init()
│            Initialise la table des capability tokens.
│
├─ Step 2   exocage::exocage_global_enable()
│            Active CET shadow stack (MSR IA32_U_CET, IA32_S_CET).
│            Note : SMEP/SMAP/KPTI déjà activés en A-12.
│
├─ Step 3   crypto::init()
│            Seed le CSPRNG depuis RDRAND + TSC entropy.
│
├─ Step 4   exoseal::verify_chain()
│            Vérifie la chaîne de confiance des binaires Ring1.
│
├─ Step 5   kaslr::finalize()
│            Publie l'offset KASLR pour les modules.
│
├─ Step 6   exoledger::init()
│            Initialise le journal d'audit immuable.
│
├─ Step 7   exokairos::init()
│            Initialise le gestionnaire de capabilities temporelles.
│            CORR-IOTA-11 : avec reset de fenêtre KAIROS_WINDOW_NS.
│
├─ Step 8   zero_trust::init()
│            Charge la politique Zero Trust.
│
├─ Step 9   ipc_policy::init()
│            Charge les règles de filtrage IPC par PID.
│
├─ Step 10  exploit_mitigations::mitigations_init()
│            Vérifie les mitigations déjà appliquées en A-12.
│            Applique les mitigations manquantes si chemin UEFI alternatif.
│
├─ Step 11  exonmi::exonmi_init()
│            Programme le watchdog NMI (LAPIC déjà disponible depuis A-07).
│
├─ Step 12  exocage::enable_cet_for_thread(current_tcb)
│            Active CET sur le TCB BSP courant.
│
└─ Step 13  mark_security_ready()
             Positionne is_security_ready() = true.
             Tout appel ultérieur à security_init() est ignoré (idempotent).
```

---

## Invariants de Boot — Assertions Critiques

Ces invariants doivent être vérifiables à tout moment après la phase B.

```rust
// Après B-3 scheduler::init() :
debug_assert!(crate::process::resource::cgroup::root().is_valid());
debug_assert!(crate::scheduler::core::runqueue::is_initialized());
// Ordre garanti : root cgroup valide AVANT runqueue (CORR-IOTA-02)

// Après B-2e time_init() :
debug_assert!(crate::time::is_calibrated());

// Après B-2f drivers::init() :
debug_assert!(crate::drivers::iommu::is_initialized());

// Après A-12 apply_mitigations_bsp() :
debug_assert!(crate::arch::x86_64::spectre::kpti::kpti_is_enabled()
    || crate::arch::x86_64::features::cpu_features().rdcl_no(),
    "KPTI doit être actif si Meltdown possible");

// Après B-5 security_init() :
debug_assert!(crate::security::kaslr_is_ready());
debug_assert!(crate::security::is_security_ready());

// Après B-7 fs::init() :
debug_assert!(crate::fs::exofs::is_disk_backed()
    || crate::fs::exofs::is_ram_fallback(),
    "ExoFS doit déclarer son mode (disk ou RAM fallback)");

// Après B-8 exophoenix::init() :
debug_assert!(crate::exophoenix::is_watchdog_armed());
```

---

## Points d'Attention pour la Revue de Code v0.2.0

### 1. Double Appel de `security_init()` — Idempotence Garantie ?

`security_init()` est appelée deux fois en théorie : une fois en A-13 (early_init) et une fois en B-5 (kernel_init). Le guard `is_security_ready()` protège contre la double exécution. **Vérifier** que le guard est thread-safe (AtomicBool avec Ordering::Acquire).

### 2. Fenêtre Sans SMEP/SMAP pour les Chemins UEFI Alternatifs

Si le chemin de boot UEFI (`_start_uefi`) ne passe pas par `arch_boot_init()`, `apply_mitigations_bsp()` pourrait ne jamais être appelée avant le heap. `mitigations_init()` (Step 10 de `security_init()`) doit être capable d'activer SMEP/SMAP si le guard détecte leur absence.

### 3. `install_extended_physmap()` Appelée Avant ACPI Parser ?

Confirmer dans tous les chemins de boot (`memory_map_limine`, `memory_map_multiboot2`, `memory_map_hand_crafted`) que l'ordre est :
```
install_extended_physmap() → set_physmap_limit()
         AVANT
acpi_parser_init()
```

### 4. Ring1 Recovery Post-Bascule Phoenix

Lors d'une bascule A↔B, le kernel ne redémarre PAS. Seul `init_server` est notifié via le registre SSR. L'algorithme de vagues parallèles (CORR-IOTA-18) doit être déclenché par `phoenix_restart_ring1()` et non par `boot_services()` (qui est le chemin cold boot). **Vérifier** que les deux chemins sont distincts et que les timeouts sont adaptés (cold boot : 30s max ; recovery : 500ms max).

---

## Fichiers à Créer / Compléter pour BLOC 8 Documentation

| Fichier | Contenu | Priorité |
|---|---|---|
| `docs/BOOT_SEQUENCE_V0.2.md` | Ce document | ✅ Présent |
| `docs/SECURITY_INIT_SEQUENCE_V0.2.md` | Diagramme security_init | À extraire de ce doc |
| `docs/RECOVERY_PHOENIX_V0.2.md` | Procédure bascule A↔B | À écrire |
| `docs/CHANGELOG_V0.1_TO_V0.2.md` | Toutes les corrections CORR-IOTA | À écrire |
| `docs/Vision v0.2.0/ROADMAP-IMPLEMENTATION-V0.2.md` | Marquer wgpu/iced [-] | À compléter |

---

*claude iota — BOOT_SEQUENCE_V0.2.0_CLAUDE_IOTA.md — 2026-05-20*
