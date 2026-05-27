# BOOT_SEQUENCE_STRATA — Séquence de Démarrage Complète
## ExoOS v0.2.0 — Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** RÉFÉRENCE — remplace BOOT_SEQUENCE_V0.2.md

---

## Vue d'Ensemble

```
exo-boot (UEFI)          →  18 étapes → kernel entry
Kernel (Ring0)           →  9 phases  → Ring1 handoff
Ring1 (6 vagues)         →  39 servers → exosh ready
Audio                    →  chime → système opérationnel
```

Durée cible totale sur matériel physique : **< 8 secondes** boot to shell.

---

## PARTIE 1 — exo-boot (UEFI, avant kernel)

```
[efi-01]  validate_uefi_entry_preconditions()
           Vérifier UEFI ≥ 2.0, SystemTable magic,
           BootServices->Hdr.Signature = "IBO SYS T"

[efi-02]  config::load_config_uefi()
           Lire EFI/EXOOS/exo-boot.cfg depuis ESP
           Defaults si absent

[efi-03]  graphics::init_gop()
           EFI_GRAPHICS_OUTPUT_PROTOCOL
           Sélectionner mode préféré (config ou native)
           Effacer écran, afficher "ExoOS v0.2.0 — Strata"

[efi-04]  gpt::read_gpt()   ← NOUVEAU Strata
           Lire GPT primaire (LBA 1)
           Valider CRC32 header + table
           Localiser ESP, ExoFS ROOT, ExoFS DATA par GUID
           Fallback : GPT backup si primaire corrompu

[efi-05]  file::open_esp()
           Accéder à ESP via EFI_SIMPLE_FILE_SYSTEM

[efi-06]  file::load_file("EFI\\EXOOS\\kernel.elf")
           Lire en mémoire (buffer AllocatePool)

[efi-07]  verify::verify_ed25519(kernel_data, pubkey)
           Vérifier signature kernel
           Panic si invalide (sauf DEV_BYPASS loggé)

[efi-08]  memory::collect_uefi_memory_map()
           GetMemoryMap() → stocker pour BootInfo

[efi-09]  rng::get_entropy(64 bytes)
           EFI_RNG_PROTOCOL → RDRAND fallback
           Seed pour KASLR + crypto ring1

[efi-10]  elf::parse_and_load_kernel(kernel_data)
           Parse PT_LOAD segments
           Allouer physique via AllocatePages
           Copier segments, noter entry point

[efi-11]  paging::build_initial_page_tables()
           PML4 → PDPT → PD → PT
           Identity map kernel physique
           Map HHDM (kernel_phys → KERNEL_VIRT_BASE + kaslr)
           Map BootInfo struct

[efi-12]  handoff::build_boot_info_v2()   ← NOUVEAU Strata
           Remplir tous champs v1 (magic, mmap, fb, acpi...)
           Remplir champs v2 :
             exofs_root_phys / lba / sectors
             exofs_data_phys / lba / sectors
             disk_guid, boot_partition_guid
             boot_from_usb flag
             nvme_controller_phys (si détecté)
             ahci_controller_phys (si détecté)

[efi-13]  nvram::register_boot_entry()   ← NOUVEAU Strata
           Créer/mettre à jour BootXXXX dans NVRAM UEFI
           Mettre ExoOS en premier dans BootOrder

[efi-14]  system_table.exit_boot_services()
           POINT DE NON-RETOUR
           Dernier appel UEFI autorisé

[efi-15]  mark_boot_services_exited()
           Invalider tous les pointeurs BootServices

[efi-16]  memory::disable_uefi_cache_attrs()
           Nettoyer les attributs UEFI sur MMIO regions

[efi-17]  memory::flush_tlb()

[efi-18]  jump_to_kernel(entry_point, &boot_info_v2)
           CS = 0x08, DS = 0x10 (GDT minimale UEFI)
           → kernel _start
```

---

## PARTIE 2 — Kernel Ring0 (9 Phases)

```
[k-phase-1]  ARCH
  GDT 64-bit (kernel code/data/TSS)
  IDT vide (handlers stub uniquement)
  CR0 : PE, WP, PG
  CR4 : PAE, PGE, OSFXSR, OSXMMEXCPT
  EFER : LME, NXE
  MSR_IA32_EFER : SCE (SYSCALL enable)
  EmergencyPool alloué (256KB pour early panics)

[k-phase-2]  MEMORY
  Lire BootInfo v2 memory map
  Construire physmap étendue (CORR-76 : accepte > 1 GiB)
  Init buddy allocator (zones : DMA32, NORMAL, HIGH)
  Init SLUB allocator (32B..2KB)
  Init vmalloc (non-contiguous)
  Boot stack permanent alloué (BOOT_STACK_PAGES = 4)
  const_assert!(PHYSMAP_INITIAL_COVERAGE >= 1<<30)  ← BLOC-0 O-04

[k-phase-3]  TIME
  HPET ou PM Timer : ktime_get_ns() opérationnel
  TSC calibration (10×1ms samples, seqlock)
  RTC lue via clock driver : epoch système initialisé

[k-phase-4]  DRIVERS
  ExoCage : SMEP + SMAP + KPTI + CET + NX + IBRS activés
  IOMMU : domaines NET / BLOCK / BLACKHOLE créés
  IommuFaultQueue initialisé (CAS-strong)
  PCI scan : construire device tree
  Initialiser drivers actifs :
    - AHCI (si présent : CORR-86 BAR lu depuis PCI config space)
    - NVMe (si présent)
    - virtio_blk (QEMU)
    - USB XHCI/EHCI
    - e1000 / virtio_net
    - PS/2 keyboard + mouse
    - Framebuffer GOP (depuis BootInfo v2)
    - HDA ou virtio_sound → audio_server_init_hw()
    - Clock HPET
  Log "DRIVERS: all_ok"

[k-phase-5]  SCHEDULER
  SMP topology détectée (ACPI MADT)
  BSP init :
    runqueue_init()
    cgroup::init() ← CORR-77 : avant runqueue_init() strict
    timer_init()
  AP startup :
    INIT IPI → SIPI (×2)
    APs reçoivent SYSCALL MSRs AVANT STI ← CORR précédent
    APs exécutent ap_start() → idle loop
  percpu::init() : gs:[0x20] current TCB écrit ← CORR précédent
  FPU lazy save/restore activé
  Idle thread créé (PID 0 par CPU)

[k-phase-6]  PROCESS
  Process table init
  PID allocator (IDR)
  fork/exec infrastructure
    register_addr_space_cloner() câblé ← CORR précédent
    register_elf_loader() câblé ← CORR précédent
  signal infrastructure
  Thread-local storage (TLS/GS)
  USER_ELF_BASE_MIN ≤ 0x400000 ← CORR-80

[k-phase-7]  SECURITY
  security_init() câblé ← CORR précédent
  ExoSeal : vérification boot chain (kernel hash)
  ZeroTrust labels initialisés
  CapToken allocator actif
  ExoKairos : budgets initialisés
  ExoLedger : ouverture journal (ExoFS ROOT, depuis BootInfo v2 adresses)
  ExoNMI : watchdog armé (200ms)
  Log "SECURITY_READY"  ← étape 18

[k-phase-8]  IPC
  SpscRing channels créés pour Ring1
  ipc_broker PID 2 démarré
  Zero Trust check actif sur chaque ipc_send
  SHM region allouée pour fb_server

[k-phase-9]  FS
  ExoFS monté depuis BootInfo v2 exofs_root_phys ← NOUVEAU Strata
  4-phase fsck exécuté
  vfs_server PID 3 démarré
  /etc/exoshield/signatures.ydb accessible
  /etc/exoos/trusted_signing.pub accessible
  Log "FS_READY"
  → Handoff à init_server (PID 1)
```

---

## PARTIE 3 — Ring1 (6 Vagues, init_server PID 1)

```
[r1-v0]  INFRASTRUCTURE RING1
  PID 1 : init_server
  PID 2 : ipc_broker (déjà actif depuis k-phase-8)
  PID 3 : vfs_server (déjà actif depuis k-phase-9)
  PID 4 : crypto_server
    → AES-GCM, ChaCha20, SHA-3, BLAKE3, Ed25519, ECDSA, Argon2id
    → TRNG opérationnel (RDRAND/RDSEED)
    → Vérification ring1 server hashes ← ExoSeal

[r1-v1]  MÉMOIRE & ORDONNANCEMENT
  PID 5 : memory_server
    → mmap avancé, pages partagées, cow
  PID 6 : scheduler_server
    → CFS, RT, deadline, CPU affinity, cgroup quotas

[r1-v2]  DEVICES & STORAGE
  PID 7  : device_server
    → PCI device tree, hot-plug event bus
  PID 8  : virtio_blk_server (QEMU) ou ahci_server (bare metal)
  PID 9  : nvme_server (si NVMe détecté)
  PID 10 : usb_server
    → USB Mass Storage events → device_server
    → USB HID events → input_server

[r1-v3]  FILESYSTEM NATIF
  PID 11 : vfs_server étendu
    → getdents64 (SYS 217) actif ← CORR bloquant
    → getcwd (SYS 79) actif ← CORR bloquant
    → ExoFS epochs + snapshots
    → Relations typées
  PID 12 : fat_server
    → FAT32 read/write
    → Monté à /mnt/usb sur USB attach

[r1-v4]  COMMUNICATION & AUDIO
  PID 13 : tty_server
    → line_disc + pty + vt100
    → BEL(0x07) → BEEP IPC vers audio_server
  PID 14 : input_server
    → PS/2 + USB HID events unifiés
  PID 15 : network_server
    → smoltcp + dhcp4r + hickory-dns
    → rustls TLS 1.3
  PID 16 : audio_server
    → Sons embarqués chargés en mémoire
    → HDA ou virtio_sound driver actif
    → IPC : PlaySound / Beep / Stop

[r1-v5]  SÉCURITÉ — DERNIER SERVEUR
  PID 17 : exo_shield
    → engine::core + scanner + realtime init
    → Signatures YARA chargées depuis ExoFS
    → Hooks syscall/exec/memory/net enregistrés
    → Politiques chargées depuis /etc/exoshield/
    → Scan initial de tous PID 1..16
    → PhoenixSafe callbacks enregistrés
    → Signal init_server : SHIELD_READY

[r1-v6]  SHELL
  PID 18 : exosh
    → TTY attaché
    → Pas de dépendance réseau ← CORR-79
    → init_server signal : RING1_COMPLETE
    → audio_server : PLAY_SYSTEM_SOUND(BOOT_COMPLETE)
    → Chime joué (~500ms)
    → Prompt affiché :

        ExoOS v0.2.0 — Strata
        ──────────────────────────────────
        Kernel : 1,017 modules | Sécurité : ✓ active
        ExoPhoenix : ✓ kernel A | Threads : 18 Ring1

        $
```

---

## Règles de Séquencement — Contraintes

### Lock Order (invariant absolu)

```
Memory → Scheduler → Security → IPC → FS
```

Aucun verrou de niveau supérieur ne peut être acquis depuis un niveau inférieur.

### Contraintes de Phase Critiques

| Contrainte | Qui | Quand |
|---|---|---|
| `cgroup::init()` avant `runqueue_init()` | k-phase-5 | CORR-77 |
| APs reçoivent SYSCALL MSRs avant STI | k-phase-5 | CORR précédent |
| `security_init()` en Phase 7 | k-phase-7 | CORR précédent |
| ExoLedger ouvert avant tout log sécurité | k-phase-7 | ExoFS adresses BootInfo v2 |
| ExoShield démarré en DERNIER Ring1 | r1-v5 | Voir tout avant |
| exosh ne dépend PAS de network_server | r1-v6 | CORR-79 |
| Boot chime APRÈS exosh prêt | r1-v6 post | Signal RING1_COMPLETE |

---

## Gestion des Erreurs de Boot

| Étape | Erreur | Comportement |
|---|---|---|
| GPT corrompu primaire | Tenter backup GPT | Si les deux corrompus : panic + écran message |
| Signature kernel invalide | Panic immédiat | Message : "ExoOS : kernel signature FAILED" |
| AHCI non détecté | Fallback virtio_blk | Log warning, pas de panic |
| NVMe non détecté | Fallback AHCI | Log info |
| Audio non détecté | Silent fallback | Pas de panic, boot continue |
| ExoShield scan trouve une menace Ring1 | Panic + forensic dump | Jamais ignorer une menace Ring1 |
| ExoFS fsck échoue phase 1-3 | Recovery auto | Log WARNING + CORR |
| ExoFS fsck échoue phase 4 | Boot recovery mode | Exosh en mode rescue |

---

*claude-alpha — ExoOS v0.2.0 — Strata — BOOT_SEQUENCE_STRATA.md*
