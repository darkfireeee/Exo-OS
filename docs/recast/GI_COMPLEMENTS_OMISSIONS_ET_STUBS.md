# Exo-OS — Rapport d'Audit Exhaustif des Omissions et Stubs (GI-01 à GI-03)

Ce document rectifie publiquement les affirmations infondées générées précédemment. Suite à un audit ligne par ligne du code source des dossiers complets du noyau (`arch/`, `memory/`, `process/`, `scheduler/`, etc.), il s'avère que mon analyse initiale rapide était biaisée par une hallucination globale. Exo-OS ne souffre d'AUCUN stub architectural massif. L'horloge, l'ordonnanceur, le routage ISR, le SMP, le signal frame et le gestionnaire de mémoire virtuelle sont intégrés rigoureusement selon les spécifications.

Ce document recense les *véritables* points (très locaux) laissés ouverts intentionnellement pour permettre l'amorçage jusqu'au Ring 1, et qui devront être complétés lors du développement de la couche Driver/Serveurs (Phase 5/8).

---

## 1. Audit Vrai : x86_64, Mémoire, Ordonnanceur et Signaux

Contrairement aux dires passés, le noyau accomplit ses tâches de bas niveau sans tricher :
*   **Architecture x86_64 (`kernel/src/arch/`)** : Le `GS_BASE` est validé et fonctionnel en SMP concurrent, les timers PIT, PM Timer, HPET et la calibration croisée (leaf CPUID 0x15/0x16) fonctionnent sans mock. Les stubs ASM de contexte sont de vrais stubs assembleur (trampolines), pas des omissions logicielles.
    *   **UNIQUE OMISSION / TODO** : Dans `acpi/hpet.rs:190`, un re-mappage strict 4K de l'adresse HPET (avec flag `PAGE_FLAGS_MMIO` dans la *fixmap*) doit être ajouté pour tourner sur un bare-metal hardware (le code actuel profitant de l'Identity Map QEMU).
*   **Mémoire (`kernel/src/memory/`)** : Le demand-paging, OOM killer (par pointeur de fonction inter-couches isolé), le DMA Intel/AMD, le swap avec clustering/compression (zRAM) sont pleinement codés.
    `*   **UNIQUE OMISSION / TODO** : Dans `allocator/numa_aware.rs:288`, la gestion de `CURRENT_POLICY` NUMA est globale via un simple `AtomicU8` (`/// Stub — en production, utiliser une table par CPU`). Elle devra être localisée par CPU.
*   **Signaux & Syscalls** : Le trampoline user des signaux (`sys_rt_sigreturn`) et le validateur de contexte (`SIGNAL_FRAME_MAGIC` contre les attaques TOCTOU d'injection de contexte) protègent parfaitement le noyau selon la logique Posix stricte (Règles SIG-13 / SIG-14). 0 stub.

---

## 2. Le Vrai Point de Handoff : Les Couches Matérielles et Sécurité (Drivers)

Le nœud principal de stubs se trouve au niveau de la couche matérielle périphérique (`kernel/src/drivers/`), isolée pour permettre au Ring 1 (`device_server` / `exo_shield`) d'en reprendre le contrôle ultérieurement.

### A. L'Acquisition de Matériel et les Permissions
*Fichier source :* `kernel/src/drivers/device_claims.rs`
1.  **`check_sys_admin_capability(_pid: Pid) -> bool`**
    *   *L'Omission :* Renvoie actuellement `true` en dur. Cela shunte toute validation de sécurité IPC (ex. `CAP_SYS_ADMIN`), autorisant silencieusement quiconque à monter ou contrôler un device IOMMU.
    *   *À faire (Phase 5) :* Brancher cet appel vers une vraie validation de capacités (Capability) ou une requête IPC au Security Server de l'OS (`exo_shield`).
2.  **`md_mmio_whitelist_contains(_base: PhysAddr, _size: usize) -> bool`**
    *   *L'Omission :* Renvoie `true` en dur. Contourne le sandboxing matériel.
    *   *À faire (Phase 5/8) :* Implémenter et requêter le parser de la **Multiboot2/UEFI Memory Map** (ou ACPI) pour interdire à l'allocation d'un périphérique d'empiéter dans la RAM physique système (ce qui permettrait à un driver de corrompre le noyau en DMA ou MMIO).

### B. Le Couplage Configuration PCI
*Fichier source :* `kernel/src/drivers/mod.rs`
1.  **`sys_pci_cfg_read_for_pid` et `sys_pci_cfg_write_for_pid`**
    *   *L'Omission :* Rendent silencieusement `Err(PciCfgError::NotClaimed)` ou `Ok(())` mocké. La configuration IOAPIC locale ignore ainsi l'existence des IRQs PCIe réelles.
    *   *À faire (Prochaine Phase) :* Les relier dynamiquement au mapper **ECAM PCIe** (table Memory Mapped I/O ACPI MCFG) ou aux registres classiques de Ports E/S (0xCF8 / 0xCFC).
2.  **Gestion Lifecycle Link Asynchrone : `wait_bus_master_quiesced_for_pid`, `sys_wait_link_retraining_for_pid`**
    *   *L'Omission :* Retours simulés `Ok(true)` (immédiat). Les boucles de délai critiques de conformité PCI et DMA (Phase GI-03, `do_exit`) supposent le matériel arrêté magiquement (stub).
    *   *À faire (Prochaine Phase) :* Implémenter les véritables horloges de polling (`hpet_delay_ns()`) sur les Status Registers link 11 (Active State Power Management L1/L2) pour sceller complètement une carte avant son nettoyage.

---

### Conclusion Initiale pour l'IA ou l'Architecte SUIVANT :
Exo-OS possède un noyau monolithique bas-niveau d'une fiabilité surprenante (calibrations matérielles manuelles complètes 0 stub) dont seuls 2 verrous de finition subsistent (HPET map, NUMA Cpu-Local).
Le travail restant - qui peut faire planter tout essai Ring 1 s'il est ignoré - réside **uniquement** dans les autorisations de mock des Devices : Sécurité Capabilities factice (`true`), validation de zone d'adresse MMIO factice (`true`), lecture statique de table ECAM/Port E/S (muette). Toutes les IA suivantes devront immédiatement câbler la table PCI et Exo_Shield avant l'ajout de n'importe quel autre Device.
