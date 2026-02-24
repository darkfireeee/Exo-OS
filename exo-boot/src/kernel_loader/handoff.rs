//! handoff.rs — Structure BootInfo + transfert de contrôle au kernel.
//!
//! RÈGLE BOOT-03 (DOC10) :
//!   "BootInfo = contrat formel bootloader→kernel.
//!    Tous champs initialisés, zero-fill reserved."
//!
//! RÈGLE BOOT-06 :
//!   "ExitBootServices = point of no-return. Passé ce point,
//!    le bootloader ne peut plus appeler de services UEFI."
//!
//! RÈGLE BOOT-07 :
//!   "KASLR : le kernel est chargé à une adresse aléatoire.
//!    exo-boot calcule kaslr_base, apply relocations, puis jump."
//!
//! Le kernel récupère `BootInfo` via son premier argument (registre RDI en SysV ABI).
//! La structure doit être en mémoire physique accessible (identité-mappée).
//!
//! CONTRAT : Ce fichier est la source de vérité du format BootInfo.
//!  Tout changement doit être synchronisé avec :
//!    kernel/src/arch/x86_64/boot/early_init.rs
//!    (notamment avec BOOT_INFO_MAGIC et la disposition des champs)

use crate::memory::map::MemoryRegion;
use crate::memory::MAX_MEMORY_REGIONS;
use crate::memory::paging::PageTablesSetup;

// ─── Magic & version ──────────────────────────────────────────────────────────

/// Magic number BootInfo — "EXOOS_BO" en ASCII little-endian.
/// Vérifié par le kernel au démarrage.
pub const BOOT_INFO_MAGIC: u64 = 0x4F42_5F53_4F4F_5845; // "EXOOS_BO"

/// Magic 32-bit transmis dans EAX → RDI pour que le kernel détecte exo-boot.
/// = premiers 4 octets de BOOT_INFO_MAGIC ("EXOO" LE).
/// Synchronisé avec kernel/src/arch/x86_64/boot/memory_map.rs::EXOBOOT_MAGIC_U32.
pub const EXOBOOT_MAGIC_U32: u32 = 0x4F4F_5845; // "EXOO"

/// Version du format BootInfo. Incrémentée si structure change.
pub const BOOT_INFO_VERSION: u32 = 1;

// ─── FramebufferInfo ─────────────────────────────────────────────────────────

/// Format des pixels du framebuffer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// R8G8B8X8 (octet inutilisé en fin).
    Rgbx = 0,
    /// B8G8R8X8 (octet inutilisé en fin).
    Bgrx = 1,
    /// Masques custom (rare — certains firmware exotiques).
    Custom = 2,
    /// Pas de framebuffer linéaire disponible.
    None = 0xFFFF_FFFF,
}

/// Informations sur le framebuffer transmises au kernel.
/// Permet au kernel de dessiner à l'écran sans appels firmware.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Adresse physique du début du framebuffer (linéaire).
    pub phys_addr:  u64,
    /// Largeur en pixels.
    pub width:      u32,
    /// Hauteur en pixels.
    pub height:     u32,
    /// Stride en pixels (peut être > width si padding de ligne).
    pub stride:     u32,
    /// Profondeur en bits par pixel (généralement 32).
    pub bpp:        u32,
    /// Format des pixels.
    pub format:     PixelFormat,
    /// Taille totale du framebuffer en bytes.
    pub size_bytes: u64,
}

impl FramebufferInfo {
    /// Construit un FramebufferInfo "absent" (None format).
    pub const fn absent() -> Self {
        Self {
            phys_addr:  0,
            width:      0,
            height:     0,
            stride:     0,
            bpp:        0,
            format:     PixelFormat::None,
            size_bytes: 0,
        }
    }

    /// Retourne `true` si un framebuffer valide est présent.
    pub fn is_present(&self) -> bool {
        self.format != PixelFormat::None && self.phys_addr != 0 && self.size_bytes > 0
    }

    /// Octets par ligne (stride × bpp/8).
    pub fn bytes_per_line(&self) -> u32 {
        self.stride * (self.bpp / 8)
    }
}

// ─── BootInfo ────────────────────────────────────────────────────────────────

/// Structure de transfert bootloader → kernel.
///
/// `repr(C)` obligatoire — layout ABI stable, vérifié par le kernel.
/// Taille totale : environ 8 KiB (dominée par le tableau memory_regions).
///
/// RÈGLE BOOT-03 : Tous les champs DOIVENT être initialisés.
///   Utiliser `BootInfo::new()` puis remplir les champs.
#[repr(C, align(4096))]
pub struct BootInfo {
    // ── En-tête ──────────────────────────────────────────────────────────
    /// Magic number — BOOT_INFO_MAGIC.
    /// Premier champ vérifié par le kernel — offset 0.
    pub magic:               u64,
    /// Version du format — BOOT_INFO_VERSION.
    pub version:             u32,
    /// Nombre d'entrées valides dans `memory_regions`.
    pub memory_region_count: u32,
    // ── Carte mémoire ───────────────────────────────────────────────────
    /// Régions mémoire. Seules les `memory_region_count` premières sont valides.
    pub memory_regions:      [MemoryRegion; MAX_MEMORY_REGIONS],
    // ── Framebuffer ─────────────────────────────────────────────────────
    /// Informations framebuffer (ou `absent()` si pas de GOP).
    pub framebuffer:         FramebufferInfo,
    // ── ACPI ────────────────────────────────────────────────────────────
    /// Adresse physique de l'ACPI RSDP, ou 0 si non trouvé.
    pub acpi_rsdp:           u64,
    // ── Entropie ────────────────────────────────────────────────────────
    /// 64 bytes d'entropie hardware (EFI_RNG + RDRAND/RDSEED + TSC mixés).
    /// Utilisés par le kernel pour son PRNG interne.
    /// RÈGLE BOOT-07 : Collectés AVANT kaslr (non contaminés).
    pub entropy:             [u8; 64],
    // ── Kernel load info ─────────────────────────────────────────────────
    /// Adresse physique de base de chargement du kernel (après KASLR).
    pub kernel_physical_base: u64,
    /// Offset de l'entry point depuis `kernel_physical_base`.
    pub kernel_entry_offset:  u64,
    /// Adresse physique de l'image ELF d'origine (avant relocation).
    pub kernel_elf_phys:      u64,
    /// Taille de l'image ELF en bytes.
    pub kernel_elf_size:      u64,
    // ── Configuration boot ───────────────────────────────────────────────
    /// Flags de boot (bit 0 : KASLR actif, bit 1 : Secure Boot actif).
    pub boot_flags:           u64,
    /// Timestamp TSC au moment de la collecte (utile pour profiling).
    pub boot_tsc:             u64,
    // ── Réservé ──────────────────────────────────────────────────────────
    /// Champs réservés — DOIVENT être à zéro (RÈGLE BOOT-03).
    pub _reserved:            [u64; 16],
}

/// Flags de BootInfo.boot_flags
pub mod boot_flags {
    /// KASLR actif (kernel chargé à adresse aléatoire).
    pub const KASLR_ENABLED:       u64 = 1 << 0;
    /// Secure Boot vérifié et activé.
    pub const SECURE_BOOT_ACTIVE:  u64 = 1 << 1;
    /// Chemin UEFI (vs BIOS).
    pub const UEFI_BOOT:           u64 = 1 << 2;
    /// ACPI 2.0+ RSDP trouvé (vs 1.0 ou absent).
    pub const ACPI2_PRESENT:       u64 = 1 << 3;
    /// Framebuffer GOP disponible.
    pub const FRAMEBUFFER_PRESENT: u64 = 1 << 4;
}

impl BootInfo {
    /// Construit un BootInfo entièrement zéro-initialisé avec magic/version corrects.
    ///
    /// RÈGLE BOOT-03 : Tous les champs réservés sont à 0 par défaut.
    pub fn new() -> Self {
        // SAFETY : BootInfo est repr(C) et tous les champs numériques — zéro est valide.
        let mut bi: Self = unsafe { core::mem::zeroed() };
        bi.magic   = BOOT_INFO_MAGIC;
        bi.version = BOOT_INFO_VERSION;
        bi
    }

    /// Constante de zéro-initialisation pour les statics.
    /// Utilisée pour initialiser un static mut BootInfo sans appel de fonction.
    ///
    /// ATTENTION : `magic` et `version` sont à 0 — appeler `new()` pour les corriger.
    pub const ZEROED: Self = unsafe { core::mem::transmute([0u8; core::mem::size_of::<Self>()]) };

    /// Copie les régions mémoire depuis un slice dans BootInfo.
    /// Retourne le nombre de régions copiées.
    pub fn set_memory_regions(&mut self, regions: &[MemoryRegion]) -> usize {
        let count = regions.len().min(MAX_MEMORY_REGIONS);
        self.memory_regions[..count].copy_from_slice(&regions[..count]);
        self.memory_region_count = count as u32;
        count
    }

    /// Lit le timestamp TSC actuel et le stocke dans boot_tsc.
    pub fn record_tsc(&mut self) {
        self.boot_tsc = rdtsc();
    }

    /// Vérifie l'intégrité basique de la structure (magic + version).
    pub fn is_valid(&self) -> bool {
        self.magic == BOOT_INFO_MAGIC && self.version == BOOT_INFO_VERSION
    }
}

impl Default for BootInfo {
    fn default() -> Self { Self::new() }
}

// Taille statique vérifiée à la compilation (orientatif — ne pas dépasser 64 KiB)
const _BOOT_INFO_SIZE_OK: () = assert!(core::mem::size_of::<BootInfo>() <= 65536,
    "BootInfo dépasse 64 KiB — réduire MAX_MEMORY_REGIONS ou _reserved");

// ─── Handoff vers le kernel ───────────────────────────────────────────────────

/// Transfère le contrôle au kernel.
///
/// Cette fonction est le POINT DE NON-RETOUR final du bootloader.
///
/// PRÉCONDITIONS (vérifiées par l'appelant) :
///   1. `crate::uefi::exit::boot_services_active()` == false (RÈGLE BOOT-06)
///   2. CR3 = page_tables.pml4_phys (identité [0-4GiB] + higher-half PML4[511])
///   3. GDT 64-bit chargée (CS = 0x08, DS/SS = 0x10)
///   4. Interruptions désactivées (IF = 0)
///   5. `boot_info` accessible en mémoire physique (identité-mappé, < 4 GiB)
///
/// PARAMÈTRES :
///   - `boot_info`   : Référence à la structure BootInfo allouée statiquement.
///   - `entry_point` : Adresse physique (identité-mappée) de `_start` du kernel.
///   - `_kaslr_base` : Réservé  / compatibilité appelants (non utilisé dans l'ASM).
///   - `page_tables` : Tables de pages finales (pml4_phys pour CR3).
///
/// CONVENTION D'APPEL (correspond à `_start` du kernel, kernel/src/main.rs) :
///   EAX = EXOBOOT_MAGIC_U32   → _start met dans RDI → kernel_main(mb2_magic)
///   RBX = adresse physique BootInfo → RSI → kernel_main(mb2_info)
///   xor RDX,RDX (0)           → RDX → kernel_main(rsdp_phys=0, lu depuis BootInfo.acpi_rsdp)
///
/// # Safety
/// Toutes les préconditions DOIVENT être satisfaites avant l'appel.
/// Comportement indéfini si le kernel n'est pas correctement mappé.
pub unsafe fn handoff_to_kernel(
    boot_info:    *const BootInfo,
    entry_point:  u64,
    _kaslr_base:  u64,   // Conservé pour compatibilité des appelants
    page_tables:  &PageTablesSetup,
) -> ! {
    // Charge les tables de pages finales (identité [0-4GiB] + higher-half PML4[511])
    // SAFETY : pml4_phys est une table PML4 valide pré-construite.
    unsafe {
        core::arch::asm!(
            "mov cr3, {pml4}",
            pml4 = in(reg) page_tables.pml4_phys,
            options(nostack, preserves_flags),
        );
    }

    // Convention d'appel correspondant à `_start` du kernel (kernel/src/main.rs) :
    //   _start : mov edi, eax  → RDI = mb2_magic = EXOBOOT_MAGIC_U32
    //            mov rsi, rbx  → RSI = mb2_info  = adresse physique BootInfo
    //            xor edx, edx  → RDX = rsdp_phys = 0 (lu par arch_boot_init depuis BootInfo)
    //            call kernel_main
    //
    // RÈGLE BOOT-06 : point de non-retour — `jmp`, pas `call`.
    // SAFETY : entry_point est l'adresse validée du kernel, boot_info est initialisé.
    unsafe {
        core::arch::asm!(
            "cli",
            "mov eax, {magic}",
            "mov rbx, {boot_info}",
            "xor edx, edx",
            "jmp {entry}",
            magic     = const EXOBOOT_MAGIC_U32,
            boot_info = in(reg) boot_info as u64,
            entry     = in(reg) entry_point,
            options(noreturn, nostack),
        );
    }
}

// ─── Utilitaires ─────────────────────────────────────────────────────────────

/// Lit le Time Stamp Counter (TSC).
#[inline]
pub fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY : RDTSC est disponible sur tout x86_64 post-Pentium.
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Construit l'adresse virtuelle higher-half d'un kernel chargé à `phys_base`.
#[inline]
pub fn kernel_virtual_base(phys_base: u64) -> u64 {
    phys_base + crate::memory::KERNEL_HIGHER_HALF_BASE
}
