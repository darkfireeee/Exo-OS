# 📋 DOC 10 — BOOTLOADER · LOADER · DRIVERS · USERSPACE
> Exo-OS · Couches en dehors du kernel Ring 0
> Cohérence architecturale · Isolation sécurité · POSIX partiel

---

## VUE D'ENSEMBLE DES COUCHES SYSTÈME

```
┌─────────────────────────────────────────────────────────────────────┐
│                    ESPACE MÉMOIRE COMPLET EXO-OS                    │
├─────────────────────────────────────────────────────────────────────┤
│  [HARDWARE]                                                         │
│       ↓ UEFI Secure Boot + BIOS                                     │
│  [BOOTLOADER]  ←  exo-boot/   · Ring -1/0 (avant kernel)           │
│       ↓ handoff → kernel _start                                     │
│  [KERNEL Ring 0]  ←  DOC2-9 (memory/scheduler/ipc/fs/security)     │
│       ↓ syscall interface                                            │
│  [Ring 1 — Drivers supervisés]  ←  drivers/  (exo-driver model)    │
│       ↓ IPC capability-gated                                        │
│  [Ring 1 — Services système]  ←  servers/   (shield, init, etc.)   │
│       ↓ libc / libexo                                               │
│  [Ring 3 — Applications]  ←  userspace/                            │
└─────────────────────────────────────────────────────────────────────┘
```

### Règle architecturale transversale DOC10

```
RÈGLE ARCH-01 : Tout ce qui peut être hors kernel DOIT être hors kernel.
  Règle de séparation maximale inspirée du modèle microkernel.
  Le kernel Ring 0 ne contient que : memory/ scheduler/ ipc/ fs/ security/ process/.
  Tout le reste : Ring 1 (drivers/servers) ou Ring 3 (userspace).

RÈGLE ARCH-02 : Drivers hors kernel = isolation de crash.
  Un driver qui crash → panique dans son processus Ring 1 → kernel récupère.
  Sans cette isolation → un driver bugué = kernel panic complet.

RÈGLE ARCH-03 : Communication inter-couches = IPC capability-gated UNIQUEMENT.
  Pas de mémoire partagée implicite entre couches.
  Pas d'accès direct au kernel depuis Ring 1 sauf via syscall bien défini.
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 1 — BOOTLOADER : `exo-boot/`
## ═══════════════════════════════════════════════════════

### Position et rôle

```
exo-boot/ est un binaire SÉPARÉ du kernel.
Il est chargé par UEFI ou BIOS, configure le matériel minimal,
puis charge le kernel depuis le disque et lui transfère le contrôle.

⚠️ Exo-boot est TEMPORAIRE dans l'espace d'exécution :
   Il s'exécute avant le kernel, n'est plus en mémoire après handoff.
   Aucune dépendance de code entre exo-boot et kernel (deux binaires distincts).
```

### Arborescence complète

```
exo-boot/
├── Cargo.toml               # crate no_std, target x86_64-unknown-uefi / i686-unknown-none
├── build.rs                 # Génère le linker script selon la cible
├── linker/
│   ├── uefi.ld              # Linker script UEFI (PE32+ format)
│   └── bios.ld              # Linker script BIOS MBR/GPT
│
└── src/
    ├── main.rs              # Point d'entrée UEFI (efi_main) ou BIOS (_start)
    │
    ├── uefi/                # Chemin UEFI (machines modernes, sécurisé)
    │   ├── mod.rs
    │   ├── entry.rs         # EFI_MAIN_ENTRY — signature UEFI obligatoire
    │   ├── services.rs      # Boot services (AllocatePages, GetMemoryMap, etc.)
    │   ├── protocols/
    │   │   ├── mod.rs
    │   │   ├── graphics.rs  # GOP — Graphics Output Protocol (framebuffer)
    │   │   ├── file.rs      # EFI_FILE_PROTOCOL — lecture FAT32/ESP
    │   │   ├── loaded_image.rs # EFI_LOADED_IMAGE — infos sur le bootloader lui-même
    │   │   └── rng.rs       # EFI_RNG_PROTOCOL — entropy initiale (KASLR)
    │   ├── secure_boot.rs   # Vérification signature kernel (Secure Boot chain)
    │   │                    # ⚠️ Ne charge JAMAIS un kernel non signé si Secure Boot actif
    │   └── exit.rs          # ExitBootServices — point de non-retour UEFI
    │
    ├── bios/                # Chemin BIOS legacy (machines anciennes / VMs)
    │   ├── mod.rs
    │   ├── mbr.asm          # MBR 512 bytes (stage 1) — charge stage 2
    │   ├── stage2.asm       # Stage 2 — active A20, passe en mode protégé
    │   ├── vga.rs           # Sortie texte VGA 80×25 (debug boot)
    │   └── disk.rs          # Lecture disque via BIOS INT 13h (CHS/LBA)
    │
    ├── memory/              # Gestion mémoire bootloader (avant kernel memory/)
    │   ├── mod.rs
    │   ├── map.rs           # Consolidation E820 + UEFI Memory Map → format unifié
    │   │                    # ⚠️ Ce format est ce que kernel/arch/boot/early_init.rs reçoit
    │   ├── paging.rs        # Setup paging identité + higher-half kernel mapping
    │   │                    # PML4 initial (4 niveaux) — le kernel reprendra avec les siens
    │   └── regions.rs       # Régions réservées (ACPI, MMIO, firmware)
    │
    ├── kernel_loader/       # Chargement du binaire kernel
    │   ├── mod.rs
    │   ├── elf.rs           # Parsing ELF64 — segments PT_LOAD uniquement
    │   │                    # Vérification : magic, machine=x86_64, type=ET_EXEC ou ET_DYN
    │   ├── verify.rs        # Vérification signature Ed25519 du kernel ELF
    │   │                    # Clé publique embarquée dans le bootloader lui-même
    │   ├── relocations.rs   # Application des relocations PIE (KASLR)
    │   │                    # Randomisation de l'adresse de chargement
    │   └── handoff.rs       # Transfert de contrôle → kernel _start
    │                        # Passe : memory_map, framebuffer_info, acpi_rsdp, entropy
    │
    ├── config/
    │   ├── mod.rs
    │   ├── parser.rs        # Parser fichier exo-boot.cfg (format minimal clé=valeur)
    │   └── defaults.rs      # Valeurs par défaut si cfg absent
    │
    ├── display/             # Affichage early boot (logo, progression)
    │   ├── mod.rs
    │   ├── framebuffer.rs   # Écriture directe dans GOP framebuffer
    │   └── font.rs          # Police bitmap embarquée (PSF1/PSF2 subset)
    │
    └── panic.rs             # Handler panic bootloader (affiche erreur + halt)
```

### Structure de handoff kernel

```rust
// exo-boot/src/kernel_loader/handoff.rs
// Structure passée au kernel au moment du handoff
// DOIT correspondre à ce qu'attend kernel/src/arch/x86_64/boot/early_init.rs

#[repr(C)]
pub struct BootInfo {
    /// Magic pour valider la structure
    pub magic: u64,                        // 0xEXO_BOOT_MAGIC_V1
    pub version: u32,

    /// Carte mémoire — format unifié E820 + UEFI
    pub memory_map: *const MemoryRegion,
    pub memory_map_count: u32,

    /// Framebuffer UEFI GOP (si disponible)
    pub framebuffer: FramebufferInfo,

    /// ACPI RSDP (pointeur vers tables ACPI)
    pub acpi_rsdp: u64,                   // adresse physique

    /// Entropy initiale pour KASLR et CSPRNG kernel
    pub entropy: [u8; 64],               // depuis EFI_RNG_PROTOCOL ou RDRAND

    /// Adresse de base réelle du kernel (après randomisation PIE)
    pub kernel_physical_base: u64,
    pub kernel_virtual_base: u64,

    /// Réservé pour extensions futures (zéro initialisé)
    pub _reserved: [u64; 16],
}

/// Régions mémoire — format unifié bootloader → kernel
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryKind,
}

#[repr(u32)]
#[derive(Copy, Clone, PartialEq)]
pub enum MemoryKind {
    Usable          = 1,  // RAM libre — donnée au buddy allocator
    Reserved        = 2,  // BIOS/firmware réservé
    AcpiReclaimable = 3,  // Tables ACPI — reclaimable après parsing
    AcpiNvs         = 4,  // ACPI NVS — NE PAS toucher
    BadMemory       = 5,  // RAM défectueuse
    Bootloader      = 6,  // Code/data bootloader — reclaimable après boot
    KernelCode      = 7,  // Sections kernel text + rodata
    KernelData      = 8,  // Sections kernel data + bss
    Framebuffer     = 9,  // Framebuffer GOP — mappé par kernel/fs/tty
}
```

### Règles bootloader

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — exo-boot/                                    │
├────────────────────────────────────────────────────────────────┤
│ BOOT-01 │ Exo-boot est un binaire SÉPARÉ du kernel             │
│           │ Aucune dépendance de code entre les deux            │
│ BOOT-02 │ Secure Boot : vérification signature Ed25519         │
│           │ du kernel AVANT chargement. Refus si invalide.      │
│ BOOT-03 │ BootInfo = contrat formel bootloader→kernel          │
│           │ Version checkée par le kernel (magic + version)     │
│ BOOT-04 │ Mémoire identifiée au type EXACT avant handoff       │
│           │ (KernelCode ≠ KernelData ≠ Bootloader reclaimable) │
│ BOOT-05 │ Entropy 64 bytes fournie au kernel (KASLR + CSPRNG)  │
│ BOOT-06 │ ExitBootServices = point de non-retour               │
│           │ Aucun accès aux UEFI Boot Services après ce point   │
│ BOOT-07 │ PIE + KASLR : adresse kernel randomisée par boot     │
│           │ Le kernel ne connaît pas sa propre adresse au boot  │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Charger un kernel non signé si Secure Boot actif            │
│ ✗  Partager du code entre exo-boot et kernel (deux binaires)   │
│ ✗  Accéder aux UEFI Runtime Services depuis le kernel          │
│    (sauf via arch/acpi/ pour SetVirtualAddressMap au boot)     │
│ ✗  BootInfo avec champs non initialisés (zéro-fill obligatoire)│
└────────────────────────────────────────────────────────────────┘
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 2 — LOADER : `loader/`
## ═══════════════════════════════════════════════════════

### Rôle et position

```
Le loader est un PROCESSUS Ring 3 démarré par PID 1 (init_server).
Il est invoqué par le kernel via execve() pour charger les binaires
ELF des applications utilisateur dans leur espace d'adressage.

Contrairement au bootloader, le loader est en USERSPACE.
Il utilise des syscalls normaux pour allouer et mapper la mémoire.

Analogie : ld.so / ld-linux.so sous Linux.
```

### Arborescence complète

```
loader/                          # Crate Rust userspace (std ou no_std + libc minimale)
├── Cargo.toml
└── src/
    ├── main.rs                  # Point d'entrée — invoqué par execve() avec PT_INTERP
    │
    ├── elf/
    │   ├── mod.rs
    │   ├── parser.rs            # Parsing ELF64 complet
    │   │                        # PT_LOAD, PT_DYNAMIC, PT_INTERP, PT_TLS, PT_GNU_STACK
    │   ├── validator.rs         # Vérification : magic, architecture, type, permissions
    │   ├── segments.rs          # Chargement PT_LOAD dans l'espace d'adressage
    │   │                        # mmap() avec flags corrects (R/W/X selon segment)
    │   ├── relocations.rs       # Résolution des relocations
    │   │                        # R_X86_64_GLOB_DAT, R_X86_64_JUMP_SLOT, R_X86_64_64
    │   ├── dynamic.rs           # Parsing section .dynamic (DT_NEEDED, DT_RPATH, etc.)
    │   └── tls.rs               # Thread-Local Storage initial (PT_TLS → GS register)
    │
    ├── dynamic_linker/
    │   ├── mod.rs
    │   ├── resolver.rs          # Résolution des symboles entre .so
    │   ├── library.rs           # Chargement récursif des dépendances (.so)
    │   ├── search_path.rs       # Chemins de recherche : RPATH, LD_LIBRARY_PATH, /lib/
    │   ├── symbol_table.rs      # Table de symboles globale (hashmap)
    │   └── version.rs           # Symbol versioning (GNU_VERSION_D / GNU_VERSION_R)
    │
    ├── security/
    │   ├── mod.rs
    │   ├── verify_signature.rs  # Vérification signature binaire (Ed25519)
    │   │                        # Utilise security::capability du kernel via syscall
    │   ├── capability_check.rs  # Vérification capabilities avant chargement
    │   └── pie_aslr.rs          # ASLR pour PIE binaires (mmap hint randomisé)
    │
    ├── aux/
    │   ├── mod.rs
    │   └── vector.rs            # Auxiliary vector AT_* — passé au programme chargé
    │                            # AT_PHDR, AT_PHENT, AT_PHNUM, AT_BASE, AT_ENTRY, etc.
    │
    └── entry.rs                 # Transfert vers l'entry point du programme chargé
                                 # Configure stack initiale (argv, envp, aux vector)
```

### Règles loader

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — loader/                                      │
├────────────────────────────────────────────────────────────────┤
│ LDR-01 │ Loader = processus Ring 3 (pas de code kernel)        │
│ LDR-02 │ Vérification signature binaire AVANT tout chargement  │
│ LDR-03 │ PT_LOAD : droits mémoire stricts (R seul si pas W/X)  │
│          │ Jamais mapper W+X simultanément (W^X strict)         │
│ LDR-04 │ ASLR obligatoire pour tous les PIE (base aléatoire)   │
│ LDR-05 │ TLS initialisé via PT_TLS avant entry point du prog   │
│ LDR-06 │ Aux vector correct — AT_ENTRY, AT_BASE, AT_PHDR...    │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Mapper une page W+X (écritable ET exécutable)               │
│ ✗  Charger un binaire sans signature valide                     │
│ ✗  Désactiver ASLR pour des PIE (sauf debug explicite)         │
└────────────────────────────────────────────────────────────────┘
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 3 — DRIVERS : `drivers/`
## ═══════════════════════════════════════════════════════

### Modèle de drivers Exo-OS

```
Exo-OS utilise un modèle de drivers EXOKERNEL :
Le kernel expose les ressources matérielles brutes via des syscalls.
Les drivers s'exécutent en Ring 1 (processus supervisés privilégiés).
Un driver crashé est tué par le kernel → relancé par driver_manager.

Avantages :
  - Isolation : un driver bugué ne plante pas le kernel
  - Sécurité : capabilities nécessaires pour accéder au hardware
  - Débogage : le driver est un processus Rust ordinaire

Communication driver ↔ kernel :
  - syscall exo_map_mmio(phys, size) → retourne VirtAddr mappée
  - syscall exo_request_irq(irq_nr, handler_ptr) → kernel appelle handler
  - syscall exo_alloc_dma_buffer(size, flags) → DMA-safe buffer
  - syscall exo_iommu_bind(device_id, domain) → isolation IOMMU
```

### Arborescence complète

```
drivers/
├── Cargo.toml              # Workspace Cargo — chaque driver = membre
│
├── framework/              # ← Crate partagée par TOUS les drivers
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── device.rs       # Trait Device (probe/remove/suspend/resume)
│       ├── resource.rs     # DeviceResource (MMIO, IRQ, DMA, capabilities)
│       ├── irq.rs          # Abstraction IRQ (register/free/enable/disable)
│       ├── dma.rs          # DMA abstraction (alloc_coherent, map_sg, etc.)
│       ├── bus/
│       │   ├── mod.rs
│       │   ├── pci.rs      # PCI/PCIe bus driver (enumerate, config space)
│       │   ├── usb.rs      # USB bus (hubs, endpoints)
│       │   └── platform.rs # Platform bus (ACPI-described devices)
│       ├── power.rs        # Power management (D-states, runtime PM)
│       ├── syscalls.rs     # Wrappers Rust des syscalls exo_* privilégiés
│       └── capability.rs   # Vérification capability avant accès hardware
│
├── storage/
│   ├── ahci/               # SATA AHCI (disques durs, SSDs SATA)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs     # Point d'entrée driver Ring 1
│   │       ├── controller.rs # HBA (Host Bus Adapter) init + config
│   │       ├── port.rs     # Ports SATA (slots de commandes)
│   │       ├── fis.rs      # Frame Information Structures (ATA commands)
│   │       ├── ncq.rs      # Native Command Queuing (32 commandes en vol)
│   │       └── error.rs    # Recovery : reset port si erreur
│   │
│   ├── nvme/               # NVMe (SSDs PCIe)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── controller.rs # Bar0 registers, Admin queue setup
│   │       ├── queue.rs    # Submission/Completion Queue pairs
│   │       ├── namespace.rs # NVMe Namespaces (NS = device logique)
│   │       ├── commands.rs # NVMe commands (Read/Write/Identify/Flush)
│   │       └── multiqueue.rs # IO queues per-CPU (performance)
│   │
│   └── virtio_blk/         # VirtIO Block (VMs)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── virtqueue.rs # VirtQueue (split ring)
│           └── blk.rs      # Block device ops sur VirtQueue
│
├── network/
│   ├── e1000/              # Intel GigE E1000/E1000E
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── hw.rs       # Registres hardware E1000
│   │       ├── tx.rs       # Transmission (descripteurs TX ring)
│   │       ├── rx.rs       # Réception (descripteurs RX ring)
│   │       └── phy.rs      # PHY autonégociation
│   │
│   ├── virtio_net/         # VirtIO Net (VMs)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── virtqueue.rs
│   │       └── net.rs      # TX/RX via VirtQueues
│   │
│   └── loopback/           # Interface loopback lo
│       ├── Cargo.toml
│       └── src/
│           └── main.rs     # Loopback simple (forward → receiver immédiatement)
│
├── input/
│   ├── ps2/                # PS/2 clavier + souris (legacy + VMs)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── keyboard.rs # Scan codes → keycodes, layout
│   │       ├── mouse.rs    # Protocole PS/2 souris (3/4 bytes paquets)
│   │       └── i8042.rs    # Contrôleur i8042 (ports 0x60/0x64)
│   │
│   ├── usb_hid/            # USB HID (clavier/souris USB)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── hid.rs      # HID report descriptor parsing
│   │       ├── keyboard.rs # HID keyboard → keycodes
│   │       └── mouse.rs    # HID mouse → movements + buttons
│   │
│   └── evdev/              # Event device — abstraction unifiée input
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── events.rs   # InputEvent unifiés vers serveur input
│
├── display/
│   ├── vga/                # VGA texte basique (fallback)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs     # 80×25 mode texte, couleurs 16 bits
│   │
│   ├── framebuffer/        # Framebuffer générique (GOP UEFI)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── fb.rs       # Abstraction framebuffer (mmap MMIO)
│   │       ├── blit.rs     # Opérations blit (copie rectangles)
│   │       └── cursor.rs   # Curseur hardware
│   │
│   └── virtio_gpu/         # VirtIO GPU (VMs, affichage accéléré)
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── virtqueue.rs
│           └── gpu.rs      # Commandes 2D (SCANOUT, RESOURCE_CREATE)
│
├── audio/
│   ├── hda/                # Intel HD Audio
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       ├── controller.rs # HDA controller (CORB/RIRB)
│   │       ├── codec.rs    # Codec discovery + verbs
│   │       └── stream.rs   # DMA streams audio (cyclic)
│   │
│   └── virtio_sound/       # VirtIO Sound (VMs)
│       ├── Cargo.toml
│       └── src/
│           └── main.rs
│
├── tty/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # Serveur TTY — gère les terminaux
│       ├── pty.rs           # Pseudo-terminals (master/slave)
│       ├── line_disc.rs     # Line discipline (echo, canonical mode)
│       ├── vt100.rs         # Émulation VT100/ANSI (séquences d'échappement)
│       └── console.rs       # Console système (dmesg, panic display)
│
├── clock/
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs          # Serveur horloge
│       ├── rtc.rs           # RTC hardware (CMOS, ports 0x70/0x71)
│       ├── hpet.rs          # HPET (High Precision Event Timer)
│       ├── tsc.rs           # TSC calibration + clocksource
│       └── ntp_sync.rs      # NTP sync (via network driver)
│
└── manager/                 # Driver Manager — supervise tous les drivers Ring 1
    ├── Cargo.toml
    └── src/
        ├── main.rs          # PID 2 — lancé par init_server immédiatement
        ├── registry.rs      # Registre driver_name → PID
        ├── probe.rs         # Probe ACPI/PCI → match → lancer le bon driver
        ├── hotplug.rs       # USB hotplug, PCIe hotplug
        ├── restart.rs       # Redémarrage automatique si driver crashe
        │                    # Politique : 3 crash en 60s → abandon + log
        └── capabilities.rs  # Attribution des capabilities hardware au driver
                             # (MMIO ranges, IRQ, DMA) via security/capability/
```

### Modèle de communication driver ↔ kernel

```rust
// drivers/framework/src/syscalls.rs
// Wrappers Rust des syscalls privilégiés (Ring 1 uniquement)

/// Mapper une région MMIO dans l'espace d'adressage du driver
/// Nécessite : CapToken avec droit EXEC sur la resource physique
pub fn exo_map_mmio(
    phys_base: PhysAddr,
    size: usize,
    cap: CapToken,
) -> Result<*mut u8, DriverError> {
    // syscall(SYS_EXO_MAP_MMIO, phys_base, size, cap)
    // Le kernel vérifie le cap avant de mapper (security/capability/verify())
    unsafe { syscall3(SYS_EXO_MAP_MMIO, phys_base.as_u64(), size as u64, cap.as_u128_lo()) }
        .map(|addr| addr as *mut u8)
        .map_err(DriverError::from)
}

/// Enregistrer un handler IRQ
/// Le kernel appelle handler_fn dans le contexte du driver (pas en Ring 0)
pub fn exo_request_irq(
    irq_nr: u32,
    handler_fn: extern "C" fn(irq_nr: u32, data: *mut ()) -> IrqAction,
    data: *mut (),
    cap: CapToken,
) -> Result<IrqHandle, DriverError> {
    // Le handler s'exécute dans un thread IRQ dédié du driver (pas dans le kernel)
    // Pas de code Ring 0 dans les drivers
    unsafe {
        syscall4(SYS_EXO_REQUEST_IRQ, irq_nr as u64,
            handler_fn as u64, data as u64, cap.as_u128_lo())
    }.map(IrqHandle::from)
     .map_err(DriverError::from)
}

/// Allouer un buffer DMA-safe (physiquement contigu, cohérent)
pub fn exo_alloc_dma_buffer(
    size: usize,
    flags: DmaFlags,
    cap: CapToken,
) -> Result<DmaBuffer, DriverError> {
    // Le kernel alloue via memory/dma/ et mappe dans l'espace du driver
    let result = unsafe {
        syscall3(SYS_EXO_ALLOC_DMA, size as u64, flags.bits(), cap.as_u128_lo())
    }?;
    Ok(DmaBuffer {
        virt: result.virt_addr as *mut u8,
        phys: PhysAddr::new(result.phys_addr),
        size,
    })
}
```

### Règles drivers

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — drivers/                                     │
├────────────────────────────────────────────────────────────────┤
│ DRV-01 │ Chaque driver = processus Ring 1 SÉPARÉ              │
│          │ (pas de code kernel, pas de Ring 0)                 │
│ DRV-02 │ Accès hardware = syscall exo_* avec CapToken valide  │
│          │ Zéro accès direct aux ports I/O ou MMIO sans cap    │
│ DRV-03 │ IRQ handler = thread du driver (pas kernel handler)   │
│          │ Exécuté dans l'espace du driver Ring 1              │
│ DRV-04 │ driver_manager surveille tous les drivers             │
│          │ Crash → relance automatique (max 3 fois / 60s)      │
│ DRV-05 │ DMA : via exo_alloc_dma_buffer() uniquement          │
│          │ Zéro accès physique direct (IOMMU protège)          │
│ DRV-06 │ Drivers découverts et lancés par driver_manager      │
│          │ (ACPI/PCI probe → match → spawn avec les bonnes caps)│
│ DRV-07 │ Pas de liaison entre drivers (tout via IPC kernel)   │
│          │ Driver A ne peut pas appeler directement Driver B    │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Code driver en Ring 0 (même pour "performance")             │
│ ✗  Accès MMIO sans capability vérifiée par kernel              │
│ ✗  IRQ handler bloquant (doit être court et non-bloquant)      │
│ ✗  Allocation mémoire dans un IRQ handler (async uniquement)   │
│ ✗  Driver qui accède aux registres d'un autre driver           │
└────────────────────────────────────────────────────────────────┘
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 4 — SERVICES SYSTÈME : `servers/`
## ═══════════════════════════════════════════════════════

### Arborescence complète

```
servers/
├── init/                   # PID 1 — Serveur d'initialisation
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # _start PID 1 — premier processus Ring 3
│       ├── boot_sequence.rs # Séquence de démarrage : lance driver_manager,
│       │                   # shield, network_manager, login_manager...
│       ├── service.rs      # Service descriptor (dépendances, restart policy)
│       ├── supervisor.rs   # Supervision des services (SIGCHLD, restart)
│       ├── ipc_registry.rs # Registre nommé des endpoints IPC (nom → CapToken)
│       └── shutdown.rs     # Séquence d'arrêt propre (SIGTERM → SIGKILL timeout)
│
├── shield/                 # Anti-malware Ring 1 (voir DOC9)
│   └── ...                 # Décrit en DOC9 — non redupliqué ici
│
├── network_manager/        # Gestionnaire réseau
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── interface.rs    # Gestion interfaces réseau (up/down/config)
│       ├── routing.rs      # Table de routage (prefix → nexthop)
│       ├── dhcp.rs         # Client DHCP v4/v6
│       ├── dns.rs          # Résolveur DNS + cache (DNS-over-TLS)
│       └── netlink.rs      # Interface avec net_stack/ via IPC
│
├── net_stack/              # Stack réseau — TCP/IP (Ring 1, isolation crash)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── eth.rs          # Ethernet (framing, ARP)
│       ├── ip/
│       │   ├── mod.rs
│       │   ├── ipv4.rs     # IPv4 (fragmentation, ICMP)
│       │   ├── ipv6.rs     # IPv6 (NDP, extension headers)
│       │   └── routing.rs  # Lookup table routage
│       ├── transport/
│       │   ├── tcp.rs      # TCP (RFC 9293) — state machine complète
│       │   ├── udp.rs      # UDP
│       │   └── icmp.rs     # ICMP v4/v6
│       └── socket_api.rs   # API socket → syscalls socket/bind/connect/send/recv
│
├── vfs_server/             # Serveur VFS userspace (montages, namespaces)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── mount.rs        # Montage / démontage (mount table)
│       ├── namespace.rs    # Mount namespaces (isolation conteneurs)
│       └── pseudo_fs/
│           ├── procfs.rs   # /proc (informations processus)
│           ├── sysfs.rs    # /sys (devices, attributs kernel)
│           └── devfs.rs    # /dev (device nodes)
│
├── crypto_server/          # Serveur cryptographie (clés, certificats)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── keystore.rs     # Stockage clés chiffré (AES-256-GCM)
│       ├── tls.rs          # TLS 1.3 (rustls ou implem native)
│       ├── certs.rs        # Gestion certificats X.509
│       └── rng_service.rs  # Service entropy (RDRAND + /dev/urandom)
│
├── power_manager/          # Gestion alimentation
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── acpi.rs         # ACPI events (lid, button, battery)
│       ├── cpufreq.rs      # Gouverneur fréquence CPU (performance/powersave)
│       ├── suspend.rs      # Suspend-to-RAM (S3) / Hibernate (S4)
│       └── thermal.rs      # Capteurs thermiques + throttling
│
├── login_manager/          # Authentification utilisateurs
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── pam.rs          # Authentification (SHA3-512 + scrypt)
│       ├── session.rs      # Sessions utilisateur (UID, GID, capabilities)
│       └── tty_assign.rs   # Attribution TTY à la session
│
└── ipc_broker/             # Broker IPC nommé (directory service)
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── registry.rs     # nom_service → endpoint_cap (registre global)
        ├── lookup.rs       # Lookup service par nom
        └── acl.rs          # ACL sur les services (qui peut se connecter à quoi)
```

### Règles servers/

```
┌────────────────────────────────────────────────────────────────┐
│ RÈGLES ABSOLUES — servers/                                     │
├────────────────────────────────────────────────────────────────┤
│ SRV-01 │ PID 1 (init) lance TOUT — pas de services auto-lancés│
│ SRV-02 │ IPC entre services = capability-gated UNIQUEMENT      │
│          │ Pas de partage mémoire implicite entre servers       │
│ SRV-03 │ net_stack/ = processus séparé — crash réseau ≠ crash  │
│          │ système. Le kernel récupère proprement.              │
│ SRV-04 │ crypto_server/ = service de confiance unique          │
│          │ Les autres services délèguent le chiffrement ici     │
│ SRV-05 │ ipc_broker/ = directory service unique                │
│          │ Tous les lookups de services passent par lui         │
│ SRV-06 │ login_manager/ = seul à avoir les droits UID/GID      │
│          │ Pas d'autre service qui setuid directement           │
├────────────────────────────────────────────────────────────────┤
│ INTERDITS                                                      │
├────────────────────────────────────────────────────────────────┤
│ ✗  Service qui se lance sans passer par init                   │
│ ✗  Service qui implémente sa propre crypto (→ crypto_server)   │
│ ✗  Services qui se parlent sans passer par ipc_broker          │
│    (sauf connexions établies via ipc_broker et maintenues)     │
└────────────────────────────────────────────────────────────────┘
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 5 — BIBLIOTHÈQUES SYSTÈME : `libs/`
## ═══════════════════════════════════════════════════════

### Arborescence complète

```
libs/
│
├── libexo/                  # Bibliothèque système principale Exo-OS
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── syscall/
│       │   ├── mod.rs
│       │   ├── raw.rs       # Wrappers ASM bruts des syscalls (inline asm)
│       │   └── safe.rs      # API safe Rust au-dessus des syscalls
│       ├── ipc/
│       │   ├── mod.rs
│       │   ├── channel.rs   # Client IPC (connect, send, recv)
│       │   └── capability.rs # Gestion CapToken utilisateur
│       ├── memory/
│       │   ├── mod.rs
│       │   ├── mmap.rs      # mmap/munmap/mprotect wrappers
│       │   └── allocator.rs # Allocateur userspace (jemalloc-like)
│       └── thread/
│           ├── mod.rs
│           ├── spawn.rs     # Création thread (clone syscall)
│           └── sync.rs      # Futex-based mutex/condvar/rwlock
│
├── libc_exo/                # Couche de compatibilité POSIX (subset)
│   ├── Cargo.toml           # Permet de compiler des programmes C/C++ existants
│   └── src/
│       ├── lib.rs
│       ├── stdio.rs         # printf, scanf, fopen, fclose, fread, fwrite
│       ├── stdlib.rs        # malloc/free (→ libexo::memory::allocator)
│       ├── string.rs        # strlen, strcpy, memcpy, memset (SIMD-optimisés)
│       ├── unistd.rs        # read, write, open, close, fork, exec
│       ├── pthread.rs       # pthread_create/join/mutex/cond → libexo::thread
│       ├── signal.rs        # signal(), sigaction() → syscall process/signal/
│       ├── errno.rs         # errno thread-local
│       ├── math.rs          # Fonctions mathématiques (soft-float + hardware)
│       └── time.rs          # gettimeofday, clock_gettime, nanosleep
│
├── libexo_net/              # Bibliothèque réseau (sockets)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── socket.rs        # socket/bind/connect/listen/accept
│       ├── io.rs            # send/recv/sendto/recvfrom
│       ├── addr.rs          # sockaddr_in, sockaddr_in6, sockaddr_un
│       └── dns.rs           # getaddrinfo/getnameinfo (→ dns resolver server)
│
└── libexo_ui/               # Bibliothèque UI minimale (pour applications graphiques)
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── window.rs        # Gestion fenêtres via compositor_server
        ├── canvas.rs        # Dessin 2D (lignes, rectangles, texte)
        ├── events.rs        # Events clavier/souris
        └── font.rs          # Rendu texte (FreeType-like, subset)
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 6 — OUTILS ET APPLICATIONS : `userspace/`
## ═══════════════════════════════════════════════════════

### Arborescence complète

```
userspace/
├── shell/                   # Shell système
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── parser.rs        # Parser commandes (POSIX sh subset)
│       ├── builtins.rs      # cd, echo, export, alias, jobs, fg, bg
│       ├── job_control.rs   # Job control (SIGTSTP, SIGCONT, groupes processus)
│       ├── completion.rs    # Auto-complétion (fichiers, commandes)
│       ├── history.rs       # Historique commandes (fichier ~/.exo_history)
│       └── readline.rs      # Édition ligne (Ctrl+A/E, flèches, Ctrl+R)
│
├── coreutils/               # Utilitaires POSIX fondamentaux
│   ├── Cargo.toml           # Workspace — chaque util = binaire séparé
│   └── src/
│       ├── ls.rs            # Listage répertoires (-l, -a, -h, --color)
│       ├── cat.rs           # Concaténation fichiers
│       ├── cp.rs            # Copie fichiers/répertoires (-r, --preserve)
│       ├── mv.rs            # Déplacement/renommage
│       ├── rm.rs            # Suppression (-r, -f, --interactive)
│       ├── mkdir.rs         # Création répertoires (-p)
│       ├── chmod.rs         # Permissions fichiers
│       ├── chown.rs         # Propriétaire fichiers
│       ├── find.rs          # Recherche fichiers (-name, -type, -exec)
│       ├── grep.rs          # Recherche regex (PCRE2 ou regex native)
│       ├── sort.rs          # Tri (-n, -r, -k, -u)
│       ├── wc.rs            # Comptage (lignes, mots, octets)
│       ├── head.rs          # Premières lignes
│       ├── tail.rs          # Dernières lignes (-f pour suivi)
│       ├── cut.rs           # Extraction colonnes
│       ├── awk.rs           # Traitement texte (awk compatible)
│       ├── sed.rs           # Éditeur flux (sed compatible)
│       ├── ps.rs            # Liste processus (infos depuis /proc)
│       ├── kill.rs          # Envoi signaux
│       ├── top.rs           # Moniteur ressources temps réel
│       ├── df.rs            # Espace disque
│       ├── du.rs            # Usage disque par répertoire
│       ├── mount.rs         # Montage systèmes de fichiers
│       ├── umount.rs        # Démontage
│       ├── date.rs          # Affichage/modification date
│       ├── hostname.rs      # Nom machine
│       ├── uname.rs         # Informations système
│       └── dmesg.rs         # Log kernel (depuis /proc/kmsg ou syscall dédié)
│
├── net_tools/               # Outils réseau
│   ├── Cargo.toml
│   └── src/
│       ├── ping.rs          # ping ICMP
│       ├── ifconfig.rs      # Configuration interfaces réseau
│       ├── route.rs         # Affichage/modification table de routage
│       ├── netstat.rs       # Connexions actives, sockets en écoute
│       ├── ss.rs            # Socket statistics (remplace netstat)
│       ├── curl.rs          # Client HTTP/HTTPS minimal
│       └── ssh.rs           # Client SSH (libssh2 ou implem native)
│
├── text_editor/             # Éditeur texte (vi-like)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── modes.rs         # Normal/Insert/Visual modes
│       ├── buffer.rs        # Buffer texte (gap buffer pour efficacité)
│       ├── display.rs       # Affichage terminal (ANSI escape codes)
│       └── commands.rs      # Commandes ex (:w, :q, :s, etc.)
│
├── package_manager/         # Gestionnaire de paquets Exo-OS
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── repository.rs    # Dépôts de paquets (HTTPS + signature)
│       ├── resolver.rs      # Résolution dépendances (SAT solver)
│       ├── installer.rs     # Installation (extract + link)
│       ├── verifier.rs      # Vérification signature Ed25519 des paquets
│       └── database.rs      # Base locale des paquets installés
│
└── compositor/              # Compositeur graphique (pour système avec GUI)
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── surface.rs       # Surfaces (fenêtres, layers)
        ├── compositor.rs    # Rendu composite (Z-order, transparence)
        ├── input_dispatch.rs # Distribution events input → window en focus
        └── protocol.rs      # Protocole IPC compositor (style Wayland réduit)
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 7 — OUTILLAGE DE DÉVELOPPEMENT : `tools/`
## ═══════════════════════════════════════════════════════

### Arborescence complète

```
tools/
├── ai_trainer/              # Entraînement offline modèles IA kernel (voir DOC1)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── trace_parser.rs  # Parse traces exo-trace
│       ├── numa_optimizer.rs # Génère numa_hints_table.gen
│       └── scheduler_trainer.rs # Calibre seuils EMA ThreadAiState
│
├── exo-trace/               # Collecteur de traces kernel (profiling)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── collector.rs     # Collecte via ring buffer kernel (syscall dédié)
│       ├── events.rs        # Types d'événements (context switch, alloc, IRQ)
│       └── exporter.rs      # Export format binaire pour ai_trainer
│
├── exo-debug/               # Débogueur kernel (style kgdb)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── gdb_stub.rs      # Protocole GDB remote (série ou TCP)
│       ├── breakpoints.rs   # Breakpoints hardware (DR0-DR3)
│       ├── memory.rs        # Lecture/écriture mémoire kernel (via syscall dédié)
│       └── symbols.rs       # Résolution symboles (DWARF parsing)
│
├── exo-bench/               # Benchmarks micro et macro
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── micro/
│       │   ├── context_switch.rs  # Mesure latence context switch
│       │   ├── ipc_latency.rs     # Mesure latence IPC
│       │   ├── alloc_perf.rs      # Mesure allocateur buddy/slab
│       │   └── scheduler_pick.rs  # Calibre seuils ThreadAiState
│       └── macro/
│           ├── io_throughput.rs   # Débit I/O
│           └── network_perf.rs    # Débit réseau
│
├── exo-proof/               # Outillage preuve formelle (interface Coq)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── extractor.rs     # Extrait le code Rust du périmètre TCB
│       ├── coq_gen.rs       # Génère stubs Coq depuis types Rust
│       └── checker.rs       # Lance Coq et vérifie les preuves
│
├── mkimage/                 # Génération image disque Exo-OS
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs
│       ├── partition.rs     # Création table GPT + partitions
│       ├── format.rs        # Formatage EXT4+ (appelle mkfs.ext4plus)
│       ├── install.rs       # Copie bootloader + kernel + rootfs
│       └── sign.rs          # Signature Ed25519 du kernel (pour Secure Boot)
│
└── exo-ci/                  # CI/CD pipeline Exo-OS
    ├── Cargo.toml
    └── src/
        ├── main.rs
        ├── build.rs         # Build kernel + drivers + userspace
        ├── test_runner.rs   # Lance tests dans QEMU
        ├── proof_checker.rs # Vérifie preuves Coq à chaque commit
        └── bench_compare.rs # Compare performances avec baseline
```

---

## ═══════════════════════════════════════════════════════
## PARTIE 8 — STRUCTURE DU PROJET COMPLET
## ═══════════════════════════════════════════════════════

### Vue globale du dépôt

```
exo-os/                      # Racine du dépôt
│
├── Cargo.toml               # Workspace Cargo racine
├── Cargo.lock
├── .cargo/
│   └── config.toml          # Cibles cross-compilation, linkers
│
├── kernel/                  # Kernel Ring 0 (DOC2-9)
│   ├── Cargo.toml
│   ├── build.rs
│   ├── linker/
│   └── src/
│       └── ... (DOC1-9)
│
├── exo-boot/                # Bootloader (DOC10 Partie 1)
│   └── ...
│
├── loader/                  # Dynamic linker Ring 3 (DOC10 Partie 2)
│   └── ...
│
├── drivers/                 # Drivers Ring 1 (DOC10 Partie 3)
│   ├── Cargo.toml           # Workspace drivers
│   ├── framework/
│   ├── storage/
│   ├── network/
│   ├── input/
│   ├── display/
│   ├── audio/
│   ├── tty/
│   ├── clock/
│   └── manager/
│
├── servers/                 # Services système Ring 1 (DOC10 Partie 4)
│   ├── Cargo.toml
│   ├── init/
│   ├── shield/              # (voir DOC9)
│   ├── network_manager/
│   ├── net_stack/
│   ├── vfs_server/
│   ├── crypto_server/
│   ├── power_manager/
│   ├── login_manager/
│   └── ipc_broker/
│
├── libs/                    # Bibliothèques partagées (DOC10 Partie 5)
│   ├── Cargo.toml
│   ├── libexo/
│   ├── libc_exo/
│   ├── libexo_net/
│   └── libexo_ui/
│
├── userspace/               # Applications Ring 3 (DOC10 Partie 6)
│   ├── Cargo.toml
│   ├── shell/
│   ├── coreutils/
│   ├── net_tools/
│   ├── text_editor/
│   ├── package_manager/
│   └── compositor/
│
├── tools/                   # Outillage développement (DOC10 Partie 7)
│   ├── Cargo.toml
│   ├── ai_trainer/
│   ├── exo-trace/
│   ├── exo-debug/
│   ├── exo-bench/
│   ├── exo-proof/
│   ├── mkimage/
│   └── exo-ci/
│
├── proofs/                  # Preuves formelles Coq/TLA+
│   ├── kernel_security/     # Preuves capability/model.rs (DOC7)
│   ├── scheduler/           # Preuves no-deadlock scheduler
│   └── memory/              # Preuves no-use-after-free
│
├── tests/                   # Tests d'intégration système
│   ├── integration/         # Tests dans QEMU
│   ├── conformance/         # Tests POSIX conformance
│   └── security/            # Tests pénétration automatisés
│
└── docs/                    # Documentation
    ├── DOC1_CORRECTIONS_ARBORESCENCE.md
    ├── DOC2_MODULE_MEMORY.md
    ├── DOC3_MODULE_SCHEDULER.md
    ├── DOC4_TO_DOC9_MODULES.md
    └── DOC10_BOOTLOADER_USERSPACE.md   ← ce document
```

---

## ORDRE DE COMPILATION ET DE BOOT

### Ordre de compilation

```
ORDRE OBLIGATOIRE (dépendances Cargo) :

1. kernel/                   # No-std — compilé en premier
   └── Produit : kernel.elf (signé par tools/mkimage/)

2. exo-boot/                 # No-std — UEFI ou BIOS
   └── Produit : BOOTX64.EFI (UEFI) ou boot.bin (BIOS)

3. libs/libexo/              # No-std + alloc
4. libs/libc_exo/            # Dépend de libexo
5. libs/libexo_net/          # Dépend de libexo + libc_exo
6. libs/libexo_ui/           # Dépend de libexo + libc_exo

7. drivers/framework/        # Dépend de libexo
8. drivers/*/                # Chaque driver dépend de drivers/framework/

9. servers/init/             # Dépend de libexo
10. servers/*/               # Dépendent de libexo + libexo_net

11. loader/                  # Dépend de libexo
12. userspace/*/             # Dépendent de libexo + libc_exo

13. tools/*/                 # Outils développement (hôte, pas cible)
```

### Séquence de boot complète (kernel + userspace)

```
SÉQUENCE BOOT COMPLÈTE EXO-OS :

[UEFI Firmware]
  → Vérifie signature exo-boot (Secure Boot)
  → Charge BOOTX64.EFI

[exo-boot — UEFI]
  1. Parse mémoire (UEFI Memory Map)
  2. Charge kernel.elf depuis ESP
  3. Vérifie signature Ed25519 kernel
  4. Configure paging identité + higher-half
  5. Collecte entropy (EFI_RNG_PROTOCOL)
  6. ExitBootServices()
  7. Transfert → kernel _start avec BootInfo*

[Kernel Ring 0 — DOC2-9]
  8.  arch::boot::early_init() + parse BootInfo
  9.  memory::physical::frame::emergency_pool::init()
  10. ... (séquence boot kernel — voir DOC4-9)
  28. memory::utils::oom_killer::start_thread()
  29. process::lifecycle::create::spawn_pid1()
       └── Lance /servers/init (PID 1) avec CapSet initial

[PID 1 — init_server]
  30. Lance /drivers/manager (PID 2) avec caps hardware
  31. Attend driver_manager prêt (IPC ack)
  32. Lance /servers/shield
  33. Lance /servers/crypto_server
  34. Lance /servers/ipc_broker
  35. Lance /servers/vfs_server
  36. Lance /servers/net_stack
  37. Lance /servers/network_manager
  38. Lance /servers/power_manager
  39. Lance /servers/login_manager

[driver_manager — PID 2]
  En parallèle avec init steps 32-39 :
  40. Probe ACPI/PCI → découverte matériel
  41. Lance drivers/storage/ahci (ou nvme)
  42. Lance drivers/display/framebuffer
  43. Lance drivers/input/ps2 + usb_hid
  44. Lance drivers/tty
  45. Lance drivers/clock
  46. Lance drivers/network/e1000 (ou virtio_net)

[login_manager]
  47. Affiche prompt login sur TTY1
  48. Authentification utilisateur
  49. Lance session : shell ou compositor
```

---

## TABLEAU DES RÈGLES TRANSVERSALES DOC10

```
┌──────────────────────────────────────────────────────────────────────┐
│ RÈGLES TRANSVERSALES — Cohérence bootloader/drivers/userspace        │
├──────────────────────────────────────────────────────────────────────┤
│ CROSS-01 │ Isolation Ring stricte : 0 (kernel), 1 (drivers/servers), │
│            │ 3 (userspace). Pas de code Ring 0 en dehors du kernel.   │
│ CROSS-02 │ Communication inter-composants = IPC capability-gated.    │
│            │ Aucune mémoire partagée implicite.                        │
│ CROSS-03 │ Tous les binaires sont signés Ed25519.                    │
│            │ Vérification : bootloader (kernel), loader (userspace).  │
│ CROSS-04 │ Crash isolation : un composant qui crash ne plante pas    │
│            │ les autres. Chaque service Ring 1 est relancé par son     │
│            │ superviseur (init pour servers/, manager pour drivers/).  │
│ CROSS-05 │ La crypto est centralisée dans crypto_server.             │
│            │ Aucun autre composant n'implémente sa propre crypto.      │
│ CROSS-06 │ BootInfo = contrat strict bootloader→kernel.              │
│            │ Versionné, vérifié à la réception (magic + version).     │
│ CROSS-07 │ W^X partout : jamais de page W+X (écrit ET exécutable).   │
│            │ Appliqué par kernel (mprotect), loader, bootloader.       │
│ CROSS-08 │ ASLR obligatoire : bootloader (kernel), loader (apps).   │
│ CROSS-09 │ Capabilities minimales : chaque composant reçoit           │
│            │ uniquement les capabilities strictement nécessaires.      │
│ CROSS-10 │ Pas de setuid dans les drivers — ils ont leurs caps dès   │
│            │ le lancement par driver_manager.                          │
└──────────────────────────────────────────────────────────────────────┘
```

---

*DOC 10 — Bootloader · Loader · Drivers · Userspace — Exo-OS*
*Complète la série : DOC1-9 (Kernel) → DOC10 (Système complet)*
