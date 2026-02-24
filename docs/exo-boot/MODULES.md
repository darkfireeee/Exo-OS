# Référence des modules exo-boot

---

## Module `config`

**Fichier** : `exo-boot/src/config/`  
**Rôle** : Lecture et parsing du fichier de configuration `exo-boot.cfg`

### Types publics

```rust
pub struct BootConfig {
    pub kernel_path:    ArrayString<128>, // ex: "\EFI\EXOOS\kernel.elf"
    pub verbose:        bool,
    pub kaslr_enabled:  bool,
    pub timeout_secs:   u32,
}
```

### Fonctions publiques

```rust
/// Charge la config depuis l'ESP via UEFI SimpleFileSystem.
/// Retourne BootConfig::default_config() si le fichier est absent.
pub fn load_config_uefi(
    bt: &BootServices,
    image_handle: Handle,
) -> BootConfig

/// Construit une config par défaut (kernel_path = "\EFI\EXOOS\kernel.elf")
pub fn BootConfig::default_config() -> Self
```

### Format du fichier `exo-boot.cfg`

```ini
kernel_path=\EFI\EXOOS\kernel.elf
verbose=true
kaslr=true
timeout=5
```

---

## Module `display`

**Fichier** : `exo-boot/src/display/`  
**Rôle** : Abstraction framebuffer pour affichage avant handoff

### Types publics

```rust
pub struct BootDisplay {
    fb: *mut u32,
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
}

pub struct BootWriter { /* impl Write pour texte */ }
pub struct PanicWriter { /* panic handler post-ExitBootServices */ }
```

### Fonctions publiques

```rust
/// Initialise le display depuis les données GOP.
pub fn init_display_from_gop(
    phys_addr: u64,
    width: u32, height: u32, stride: u32,
    format: PixelFormat,
)

/// Remplit l'écran avec une couleur de fond.
pub fn BootDisplay::clear(&mut self, color: u32)

/// Dessine le logo ASCII exo-OS dans le coin supérieur gauche.
pub fn BootDisplay::draw_boot_logo(&mut self)

/// Retourne le FramebufferInfo compatible BootInfo.
pub fn get_boot_info_framebuffer() -> FramebufferInfo

/// Retourne un BootDisplay::absent() (aucun GPU, pas de panique).
pub fn BootDisplay::absent() -> Self
```

---

## Module `memory`

**Fichier** : `exo-boot/src/memory/`

### Sous-module `map` — Carte mémoire UEFI

```rust
pub struct MemoryMapCollector {
    regions: ArrayVec<MemoryRegion, 1024>,
}

/// Collecte la carte mémoire via BootServices.memory_map().
/// Ne fait aucune allocation heap après cet appel.
pub fn collect_uefi_memory_map(bt: &BootServices) -> MemoryMapCollector

/// Convertit les descripteurs UEFI en MemoryRegion.
/// Applique la fusion des régions contiguës de même type.
pub fn convert_uefi_to_memory_regions(
    collector: &MemoryMapCollector,
    kernel_phys_base: u64,
    kernel_size: u64,
    page_table_phys: u64,
    page_table_size: u64,
    framebuffer: &FramebufferInfo,
) -> ([MemoryRegion; 256], u32)
```

### Sous-module `paging` — Tables de pages

```rust
/// Types de drapeaux de page
pub struct PageFlags(u64);
impl PageFlags {
    pub const PRESENT:     PageFlags = PageFlags(1 << 0);
    pub const WRITABLE:    PageFlags = PageFlags(1 << 1);
    pub const USER:        PageFlags = PageFlags(1 << 2);
    pub const HUGE:        PageFlags = PageFlags(1 << 7);
    pub const NO_EXECUTE:  PageFlags = PageFlags(1 << 63);
}

/// Construit les PML4 initiales (identité + higher-half).
/// Retourne l'adresse physique du PML4.
pub fn setup_initial_page_tables(
    bt: &BootServices,
    kernel_phys_base: u64,
    kernel_pages: usize,
) -> u64
```

### Sous-module `acpi` — Détection RSDP

```rust
/// Recherche la RSDP via les EFI Configuration Tables (UEFI).
/// Préfère ACPI 2.0 (XSDP) sur ACPI 1.0 (RSDP).
pub fn find_acpi_rsdp_uefi(system_table: &SystemTable<Boot>) -> u64

/// Scan BIOS traditionnel : 0xE0000 → 0xFFFFF, signature "RSD PTR ".
pub fn find_acpi_rsdp_bios() -> u64
```

---

## Module `uefi`

**Fichier** : `exo-boot/src/uefi/`

### Sous-module `entry` — Validation entrée EFI

```rust
#[derive(Debug)]
pub struct EntryDiagnostics {
    pub uefi_revision: u32,
    pub secure_boot_var: Option<u8>, // lue depuis variable EFI SecureBoot
    pub setup_mode: Option<u8>,
    pub conout_available: bool,
}

#[derive(Debug)]
pub enum EntryError {
    UefiVersionTooOld { got: u32, required: u32 },
    ConOutMissing,
}

/// Valide que l'environnement UEFI est suffisant pour exo-boot.
pub fn validate_uefi_entry_preconditions(
    st: &SystemTable<Boot>,
) -> Result<EntryDiagnostics, EntryError>
```

### Sous-module `exit` — ExitBootServices

```rust
/// Atomic qui passe à `false` après ExitBootServices.
static BOOT_SERVICES_ACTIVE: AtomicBool;

/// Panics si BootServices ne sont plus actifs.
pub fn assert_boot_services_active()

/// Marque la sortie (appelé juste après exit_boot_services).
pub fn mark_boot_services_exited()
```

### Sous-module `protocols/graphics` — GOP

```rust
/// Localise EFI_GRAPHICS_OUTPUT_PROTOCOL et lit le framebuffer.
pub fn init_gop(bt: &BootServices) -> Option<GopInfo>

pub struct GopInfo {
    pub phys_base: u64,
    pub width: u32, pub height: u32, pub stride: u32,
    pub format: PixelFormat,
}
```

### Sous-module `protocols/rng` — Entropie

```rust
/// Collecte `count` octets d'entropie via EFI_RNG_PROTOCOL.
/// Fallback RDRAND/RDSEED/TSC si protocole indisponible.
pub fn collect_entropy(bt: &BootServices, count: usize) -> [u8; 64]
```

### Sous-module `protocols/file` — Lecture ESP

```rust
pub enum FileError { NotFound, ReadError(usize), TooLarge }

/// Ouvre et lit un fichier depuis le volume de l'image chargée.
pub fn load_file(
    bt: &BootServices,
    image_handle: Handle,
    path: &str,
) -> Result<&'static mut [u8], FileError>
```

---

## Module `bios`

**Fichier** : `exo-boot/src/bios/`

### Sous-module `disk` — Lecture secteurs BIO

```rust
/// Lit `count` secteurs à partir du LBA `lba` via BIOS INT 13h/AH=0x42.
/// Pas disponible après passage en long mode (utilisé par stage2.asm uniquement).
pub unsafe fn bios_read_sectors(
    drive: u8, lba: u64, count: u16, buf: *mut u8,
)
```

### Sous-module `vga` — Affichage texte BIOS

```rust
/// Buffer VGA à 0xB8000, 80×25 caractères 16 couleurs.
pub struct VgaWriter { col: u8, row: u8, color: u8 }
impl fmt::Write for VgaWriter {}

/// Boucle infinie (CLI + HLT) — panic BIOS.
pub fn halt_forever() -> !
```

---

## Module `kernel_loader`

**Fichier** : `exo-boot/src/kernel_loader/`

### Sous-module `verify` — Vérification signature

```rust
#[repr(C)]
pub struct KernelSignature {
    pub marker:    [u8; 8],   // "EXOSIG01"
    pub version:   u32,
    pub flags:     u32,
    pub sig_type:  u32,       // 1 = Ed25519
    pub _reserved: u32,
    pub signature: [u8; 64],  // signature Ed25519 (64 bytes)
    pub pubkey:    [u8; 32],  // clé publique Ed25519 (32 bytes)
    pub hash:      [u8; 64],  // SHA-512 du kernel (64 bytes)
    pub _pad:      [u8; 72],  // padding → 256 bytes total
}

pub const SIGNATURE_MARKER: &[u8; 8] = b"EXOSIG01";

/// Vérifie la signature du kernel. Panique si invalide (BOOT-02).
/// NOP si feature dev-skip-sig est activée (affiche warning).
pub fn verify_kernel_or_panic(kernel_data: &[u8])
```

### Sous-module `relocations` — KASLR

```rust
/// Applique les relocations PIE au kernel chargé en mémoire.
/// Supporte R_X86_64_RELATIVE (8) et R_X86_64_64 (1).
pub unsafe fn apply_pie_relocations(
    elf: &ElfKernel,
    load_base: u64,
)

/// Calcule une adresse physique aléatoire pour le kernel.
/// Alignée 2 MiB, dans la plage [1 GiB, 256 GiB].
pub fn compute_kaslr_base(entropy: &[u8; 64]) -> u64
```

### Sous-module `handoff` — Passage au kernel

```rust
/// Effectue le handoff final : cr3 + rsp + jmp kernel.
/// Ne retourne jamais.
pub unsafe fn handoff_to_kernel(
    pml4_phys:   u64,
    boot_info:   *const BootInfo,
    kernel_stack: u64,
    entry_phys:  u64,
) -> !
```

---

## `main.rs` — Point d'entrée global

```rust
#[entry]
fn efi_main(image_handle: Handle, mut system_table: SystemTable<Boot>) -> Status

/// Fonction principale UEFI — orchestre les 12 étapes du boot.
/// Retourne uniquement en cas d'erreur (handoff → kernel ne retourne pas).
fn boot_main(
    image_handle: Handle,
    system_table: SystemTable<Boot>,
) -> Result<!, BootError>

/// Chemin alternatif BIOS (compilé avec feature bios-boot uniquement).
fn exoboot_main_bios() -> !
```

---

## `panic.rs` — Handlers de panique

```rust
/// Appelé avant ExitBootServices.
/// Affiche via ConOut + UEFI RuntimeServices::ResetSystem.
#[cfg(not(feature = "bios-boot"))]
fn uefi_panic_handler(info: &PanicInfo) -> !

/// Appelé après ExitBootServices.
/// Affiche via framebuffer GOP (PanicWriter).
fn post_exit_panic_handler(info: &PanicInfo) -> !

/// Fallback BIOS : VGA 0xB8000 + halt.
#[cfg(feature = "bios-boot")]
fn bios_panic_handler(info: &PanicInfo) -> !
```
