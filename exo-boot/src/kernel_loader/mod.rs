//! mod.rs — Module kernel_loader : chargement, vérification et handoff kernel.
//!
//! Ce module orchestre les étapes de chargement du kernel :
//!
//!   1. `verify`      : Vérification signature Ed25519 (RÈGLE BOOT-02)
//!   2. `elf`         : Parsing ELF64 et chargement des segments PT_LOAD
//!   3. `relocations` : Application des relocations PIE + calcul KASLR (BOOT-07)
//!   4. `handoff`     : Construction BootInfo + saut au kernel (BOOT-03, BOOT-06)
//!
//! CONTRAT :
//!   - `load_kernel_uefi()` : Chemin UEFI, appelé depuis `efi_main()`
//!   - `load_kernel_bios()` : Chemin BIOS, appelé depuis `exoboot_main_bios()`
//!
//! Les deux chemins convergent vers `handoff::handoff_to_kernel()`.

pub mod elf;
pub mod handoff;
pub mod relocations;
pub mod verify;

pub use elf::{ElfKernel, ElfError};
pub use handoff::{BootInfo, FramebufferInfo, PixelFormat, BOOT_INFO_MAGIC, BOOT_INFO_VERSION,
                  EXOBOOT_MAGIC_U32};
pub use relocations::{apply_pie_relocations, compute_kaslr_base};
pub use verify::verify_kernel_or_panic;



// ─── Orchestration du chargement kernel ───────────────────────────────────────

/// Paramètres de chargement kernel.
pub struct KernelLoadParams<'a> {
    /// Image ELF du kernel (buffer lu depuis le disque/ESP).
    pub elf_data:       &'a [u8],
    /// Adresse physique où le buffer ELF est stocké (pour BootInfo).
    pub elf_phys_addr:  u64,
    /// Entropie hardware (64 bytes) pour KASLR.
    pub entropy:        [u8; 64],
    /// `true` si KASLR activé (depuis config).
    pub kaslr_enabled:  bool,
    /// `true` si Secure Boot actif (Secure Boot flag dans BootInfo).
    pub secure_boot:    bool,
}

/// Résultat du chargement kernel.
pub struct KernelLoadResult {
    /// Adresse physique de base ou le kernel a été chargé.
    pub phys_base:      u64,
    /// Adresse virtuelle de base (higher-half = phys_base + FFFF_8000_0000_0000).
    pub virt_base:      u64,
    /// Adresse physique de l'entry point.
    pub entry_phys:     u64,
    /// Offset de l'entry depuis phys_base.
    pub entry_offset:   u64,
}

/// Charge le kernel depuis une image ELF.
/// Appelé depuis les deux chemins (UEFI + BIOS) après allocation mémoire.
///
/// Étapes :
///   1. Vérifie la signature (si BOOT-02 requis)
///   2. Parse l'ELF
///   3. Calcule kaslr_base depuis l'entropie
///   4. Charge les segments PT_LOAD à phys_base
///   5. Applique les relocations PIE
///   6. Retourne `KernelLoadResult`
///
/// # Safety
/// `phys_dest` doit pointer vers de la mémoire physique accessible
/// d'au moins `elf.load_size()` bytes.
pub unsafe fn load_kernel(
    params:    &KernelLoadParams<'_>,
    phys_dest: u64,
) -> Result<KernelLoadResult, KernelLoadError> {
    // ── 1. Vérification signature ──────────────────────────────────────────
    verify_kernel_or_panic(params.elf_data);

    // ── 2. Parse l'ELF ────────────────────────────────────────────────────
    let elf = ElfKernel::parse(params.elf_data)
        .map_err(|e| KernelLoadError::ElfParse(e))?;

    // ── 3. Calcul KASLR ───────────────────────────────────────────────────
    let (phys_base, virt_base) = if params.kaslr_enabled && elf.is_pie {
        let (pb, vb) = compute_kaslr_base(&params.entropy);
        // Utilise phys_dest fourni par l'appelant (déjà alloué au bon endroit)
        // Si phys_dest == 0, utilise la base KASLR calculée
        if phys_dest == 0 { (pb, vb) }
        else {
            let virt = crate::kernel_loader::handoff::kernel_virtual_base(phys_dest);
            (phys_dest, virt)
        }
    } else {
        let virt = crate::kernel_loader::handoff::kernel_virtual_base(phys_dest);
        (phys_dest, virt)
    };

    // ── 4. Charge les segments ────────────────────────────────────────────
    // SAFETY : phys_base est la mémoire physique allouée pour le kernel.
    unsafe { elf.load_segments(phys_base) }
        .map_err(|e| KernelLoadError::ElfLoad(e))?;

    // ── 5. Applique les relocations PIE ───────────────────────────────────
    // SAFETY : phys_base contient le kernel chargé, relocations applicables.
    unsafe { apply_pie_relocations(&elf, phys_base) }
        .map_err(|e| KernelLoadError::Relocation(e))?;

    // ── 6. Calcul de l'entry point ────────────────────────────────────────
    let entry_offset = elf.entry_offset();
    let entry_phys   = phys_base + entry_offset;

    Ok(KernelLoadResult {
        phys_base,
        virt_base,
        entry_phys,
        entry_offset,
    })
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum KernelLoadError {
    ElfParse(ElfError),
    ElfLoad(ElfError),
    Relocation(relocations::RelocationError),
}

impl core::fmt::Display for KernelLoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ElfParse(e)      => write!(f, "Parse ELF : {}", e),
            Self::ElfLoad(e)       => write!(f, "Chargement ELF : {}", e),
            Self::Relocation(e)    => write!(f, "Relocation : {}", e),
        }
    }
}
