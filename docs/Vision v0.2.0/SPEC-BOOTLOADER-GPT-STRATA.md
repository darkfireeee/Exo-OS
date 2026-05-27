# SPEC-BOOTLOADER-GPT-STRATA — UEFI Natif + Partitions ExoOS
## exo-boot v0.2.0 — ExoOS Strata

**Auteur :** claude-alpha
**Date :** 2026-05-26
**Statut :** NOUVEAU — complète et étend exo-boot existant

---

## 1. Objectif

exo-boot passe de bootloader UEFI v0.1 (kernel + config depuis ESP, BootInfo v1) à un **bootloader UEFI de production** capable de :
- Lire et valider une table GPT
- Localiser et transmettre les partitions ExoFS au kernel
- S'enregistrer dans la NVRAM UEFI
- Démarrer depuis une clé USB (installation + rescue)
- Transmettre BootInfo v2 avec tous les champs Strata

GRUB est archivé. `BOOTX64.EFI` est le seul point d'entrée.

---

## 2. Schéma de Partitions ExoOS

### 2.1 — Disque Standard (installation)

```
Disque physique (SATA / NVMe / USB)
┌─────────────────────────────────────────────────────────┐
│  LBA 0        : MBR protecteur (GPT obligatoire)        │
│  LBA 1        : GPT Header primaire                     │
│  LBA 2..33    : GPT Partition Table (128 entrées × 128B)│
├─────────────────────────────────────────────────────────┤
│  Partition 1 : ESP                                      │
│  GUID type   : C12A7328-F81F-11D2-BA4B-00A0C93EC93B    │
│  Taille      : 256 MB (min 100 MB)                      │
│  FS          : FAT32                                    │
│  Contenu     :                                          │
│    EFI/EXOOS/BOOTX64.EFI   ← exo-boot signé (PE32+)   │
│    EFI/EXOOS/kernel.elf    ← kernel signé Ed25519      │
│    EFI/EXOOS/exo-boot.cfg  ← configuration             │
│    EFI/BOOT/BOOTX64.EFI    ← copie fallback            │
├─────────────────────────────────────────────────────────┤
│  Partition 2 : ExoFS ROOT                               │
│  GUID type   : [UUID ExoOS ROOT custom]                 │
│    = 45584F52-4F4F-5300-524F-4F540000002  (EXOOS·ROOT) │
│  Taille      : 4 GB minimum                             │
│  FS          : ExoFS v5                                 │
│  Contenu     :                                          │
│    /servers/  ring1 servers (ELF signés)                │
│    /lib/      exo-crates, musl-exo                      │
│    /bin/      exosh, exo, outils système                │
│    /etc/      config, ExoShield policies, keystore      │
│    /var/      ExoLedger (sealed), logs monitor_server   │
├─────────────────────────────────────────────────────────┤
│  Partition 3 : ExoFS DATA                               │
│  GUID type   : 45584F52-4F4F-5300-4441-540000000003    │
│    = (EXOOS·DATA)                                       │
│  Taille      : tout l'espace restant                    │
│  FS          : ExoFS v5                                 │
│  Contenu     :                                          │
│    /home/     données utilisateur                       │
│    /apps/     packages installés (exo install)          │
│    /tmp/      temporaire (epoch-cleared au boot)        │
│    /snapshots/ snapshots ExoFS                          │
├─────────────────────────────────────────────────────────┤
│  LBA -33..-2  : GPT Partition Table backup             │
│  LBA -1       : GPT Header backup                      │
└─────────────────────────────────────────────────────────┘
```

### 2.2 — Clé USB (Installation / Rescue)

Même schéma que le disque standard. Détecté automatiquement par exo-boot.

```
ESP    → BOOTX64.EFI + kernel.elf (rescue ou installer)
ROOT   → rootfs minimal ExoOS (rescue shell)
DATA   → optionnel (payload d'installation)
```

### 2.3 — GUIDs ExoOS

```rust
// arch/constants.rs
pub const GUID_ESP: [u8; 16] =
    [0x28, 0xa2, 0x12, 0xc1, 0x1f, 0xf8, 0xd2, 0x11,
     0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b];

pub const GUID_EXOOS_ROOT: [u8; 16] =
    [0x52, 0x4F, 0x58, 0x45, 0x4F, 0x4F, 0x00, 0x53,
     0x52, 0x4F, 0x4F, 0x54, 0x00, 0x00, 0x00, 0x02];

pub const GUID_EXOOS_DATA: [u8; 16] =
    [0x52, 0x4F, 0x58, 0x45, 0x4F, 0x4F, 0x00, 0x53,
     0x44, 0x41, 0x54, 0x41, 0x00, 0x00, 0x00, 0x03];
```

---

## 3. GPT Reader — Implémentation

**Fichier :** `exo-boot/src/gpt/mod.rs` (nouveau)

```rust
pub struct GptHeader {
    pub signature:          [u8; 8],   // "EFI PART"
    pub revision:           u32,       // 0x00010000
    pub header_size:        u32,       // 92 bytes
    pub header_crc32:       u32,
    pub _reserved:          u32,
    pub my_lba:             u64,
    pub alternate_lba:      u64,
    pub first_usable_lba:   u64,
    pub last_usable_lba:    u64,
    pub disk_guid:          [u8; 16],
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub sizeof_partition_entry: u32,
    pub partition_table_crc32: u32,
}

pub struct GptPartitionEntry {
    pub type_guid:   [u8; 16],
    pub unique_guid: [u8; 16],
    pub start_lba:   u64,
    pub end_lba:     u64,
    pub attributes:  u64,
    pub name:        [u16; 36],  // UTF-16LE
}

pub fn read_gpt(bs: &BootServices, disk_handle: Handle)
    -> Result<GptLayout, GptError>
{
    // 1. Lire LBA 1 via EFI_DISK_IO_PROTOCOL
    // 2. Valider signature "EFI PART"
    // 3. Valider CRC32 header
    // 4. Lire partition table (LBA 2..33)
    // 5. Valider CRC32 partition table
    // 6. Parser les entrées non-vides
    // 7. En cas d'erreur primaire : essayer GPT backup (LBA -1)
    // 8. Retourner GptLayout avec partitions localisées
}
```

**Règle :** En cas de corruption GPT primaire, exo-boot tente le GPT backup. Si les deux sont corrompus, panic avec message explicite + adresse NVRAM pour recovery.

---

## 4. BootInfo v2

**Fichier :** `exo-boot/src/kernel_loader/handoff.rs` — struct étendue

```rust
pub const BOOT_INFO_VERSION: u32 = 2;  // Strata

#[repr(C)]
pub struct BootInfoV2 {
    // ── Champs v1 (inchangés) ────────────────────────────────────────
    pub magic:               u64,    // BOOT_INFO_MAGIC
    pub version:             u32,    // = 2
    pub flags:               u32,
    pub memory_map:          MemoryMap,
    pub framebuffer:         FramebufferInfo,
    pub acpi_rsdp_phys:      u64,
    pub kernel_phys_base:    u64,
    pub kernel_virt_base:    u64,
    pub kaslr_offset:        u64,
    pub entropy_bytes:       [u8; 64],
    pub secure_boot_active:  bool,

    // ── Nouveaux champs v2 (Strata) ─────────────────────────────────
    pub exofs_root_phys:     u64,    // Base physique partition ROOT
    pub exofs_root_lba:      u64,    // LBA de départ ROOT
    pub exofs_root_sectors:  u64,    // Taille en secteurs
    pub exofs_data_phys:     u64,    // Base physique partition DATA
    pub exofs_data_lba:      u64,    // LBA de départ DATA
    pub exofs_data_sectors:  u64,    // Taille en secteurs
    pub disk_guid:           [u8; 16], // GUID du disque booté
    pub boot_partition_guid: [u8; 16], // GUID partition source du boot
    pub boot_from_usb:       bool,   // true si démarrage depuis USB
    pub nvme_controller_phys: u64,   // Adresse physique NVMe si détecté
    pub ahci_controller_phys: u64,   // Adresse physique AHCI si détecté

    pub _reserved:           [u8; 64],
}

// Invariant : sizeof(BootInfoV2) doit être multiple de 8
const _: () = assert!(core::mem::size_of::<BootInfoV2>() % 8 == 0);
```

---

## 5. Entrée NVRAM UEFI

exo-boot s'enregistre dans la NVRAM UEFI au **premier démarrage** depuis un nouveau disque.

```rust
// exo-boot/src/uefi/nvram.rs (nouveau)
fn register_boot_entry(bs: &BootServices, image_handle: Handle)
    -> Result<(), UefiError>
{
    // 1. Vérifier si une entrée "ExoOS" existe déjà dans BootXXXX
    // 2. Si oui : mettre à jour si le path a changé
    // 3. Si non : créer BootXXXX avec :
    //    - Description : "ExoOS v0.2.0 — Strata"
    //    - DevicePath  : le path complet vers BOOTX64.EFI
    // 4. Mettre à jour BootOrder pour placer ExoOS en premier
    //    (si Secure Boot désactivé ou clé ExoOS enrollée)
    // 5. Persister via SetVariable(L"Boot####", EFI_GLOBAL_VARIABLE)
}
```

**Protection :** Si Secure Boot est actif et la clé ExoOS n'est pas enrollée, ne pas modifier BootOrder — émettre un warning uniquement.

---

## 6. Séquence de Boot UEFI Complète (Strata)

```
Firmware UEFI
    │
    ├─ [1] EFI/EXOOS/BOOTX64.EFI chargé
    │
    └─ efi_main(image_handle, system_table)
         │
         ├─ [2] validate_uefi_entry_preconditions() — UEFI ≥ 2.0
         ├─ [3] config::load_config_uefi()          — exo-boot.cfg
         ├─ [4] graphics::init_gop()                — framebuffer GOP
         ├─ [5] memory::collect_uefi_memory_map()   — carte mémoire
         ├─ [6] gpt::read_gpt()                     — NOUVEAU Strata
         │        ├─ Localise ESP, ROOT, DATA
         │        └─ Stocke LBA/phys dans layout
         ├─ [7] file::load_file(kernel.elf)          — kernel depuis ESP
         ├─ [8] verify::verify_ed25519(kernel_data)  — signature
         ├─ [9] rng::get_entropy(64 bytes)           — KASLR seed
         ├─ [10] elf::load_kernel(kernel_data)       — parse + alloue
         ├─ [11] paging::build_page_tables()         — tables initiales
         ├─ [12] handoff::build_boot_info_v2()       — NOUVEAU Strata
         │        ├─ Remplir champs v1 (inchangés)
         │        └─ Remplir champs v2 : exofs_root, exofs_data, GUIDs
         ├─ [13] nvram::register_boot_entry()        — NOUVEAU Strata
         ├─ [14] system_table.exit_boot_services()   — POINT DE NON-RETOUR
         ├─ [15] mark_boot_services_exited()
         ├─ [16] memory::flush_tlb()
         └─ [17] jump_to_kernel(entry_point, &boot_info_v2)
```

---

## 7. Configuration exo-boot.cfg (Strata)

```toml
# /EFI/EXOOS/exo-boot.cfg
[boot]
kernel_path = \EFI\EXOOS\kernel.elf
timeout_seconds = 3
default_entry = 0

[security]
verify_kernel_signature = true
signature_pubkey_path = \EFI\EXOOS\exoos_signing_key.pub
kaslr = true
secure_boot_require = false       # true sur production signée

[display]
gop_mode_prefer = 1920x1080       # résolution préférée
gop_fallback = native             # utiliser résolution native si indisponible

[debug]
# serial_debug = false            # désactivé en production
# dev_skip_signature = false      # JAMAIS en production
```

---

## 8. Build System — Image Strata

```makefile
# Makefile targets Strata

iso-strata:
    # 1. Build exo-boot (UEFI)
    cargo build --target x86_64-unknown-uefi --release -p exo-boot
    # 2. Build kernel
    cargo build --target x86_64-unknown-none --release -p kernel
    # 3. Signer kernel avec clé Ed25519 de dev
    exo-sign --key tools/keys/dev_signing.key kernel.elf
    # 4. Créer image GPT
    tools/mkimage.sh \
        --bootloader target/.../exo-boot.efi \
        --kernel target/.../kernel.elf \
        --rootfs target/rootfs/ \
        --output target/exoos-strata.img
    # 5. Créer ISO UEFI (El Torito UEFI)
    xorriso -as mkisofs \
        -e EFI/EXOOS/BOOTX64.EFI -no-emul-boot \
        -o target/exoos-strata.iso target/iso-root/

qemu-strata:
    qemu-system-x86_64 \
        -bios /usr/share/ovmf/OVMF.fd \
        -drive file=target/exoos-strata.img,format=raw \
        -m 2G -smp 4 \
        -device e1000 \
        -device usb-ehci -device usb-storage,...
```

---

## 9. Tests Requis

```
bootloader_test::gpt_header_valid_crc             PASS
bootloader_test::gpt_find_esp_partition           PASS
bootloader_test::gpt_find_exofs_root              PASS
bootloader_test::gpt_find_exofs_data              PASS
bootloader_test::gpt_backup_recovery              PASS
bootloader_test::bootinfo_v2_magic_correct        PASS
bootloader_test::bootinfo_v2_exofs_addrs_nonzero  PASS
bootloader_test::kernel_signature_accepted        PASS
bootloader_test::kernel_signature_rejected_bad    PASS
bootloader_test::kaslr_base_nonzero               PASS
bootloader_test::usb_boot_detected                PASS
```

---

*claude-alpha — ExoOS v0.2.0 — Strata — SPEC-BOOTLOADER-GPT-STRATA.md*
