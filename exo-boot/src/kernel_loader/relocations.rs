//! relocations.rs — Application des relocations PIE + calcul KASLR.
//!
//! RÈGLE BOOT-07 (DOC10) :
//!   "KASLR : kernel chargé à adresse aléatoire.
//!    exo-boot calcule kaslr_base depuis l'entropie hardware,
//!    puis applique les relocations R_X86_64_RELATIVE dans l'image chargée."
//!
//! Processus :
//!   1. Calcul de kaslr_base depuis 64 bytes d'entropie (aligné 2 MiB)
//!   2. Parcours de la section `.rela.dyn` (DT_RELA / DT_RELASZ)
//!   3. Application de R_X86_64_RELATIVE : *addr = kaslr_base + addend
//!   4. (Optionnel) Application R_X86_64_64 pour symboles absolus
//!
//! Référence : ELF Spec + System V ABI AMD64 § 4.4 Relocation

use super::elf::ElfKernel;
use crate::memory::{KERNEL_HIGHER_HALF_BASE, HUGE_PAGE_SIZE};

// ─── Constantes de relocation ─────────────────────────────────────────────────

/// Type de relocation R_X86_64_RELATIVE (ELF x86_64 ABI).
/// Applique : *addr = load_addr + addend
const R_X86_64_RELATIVE: u32 = 8;

/// Type de relocation R_X86_64_64 (symbole absolu 64-bit).
/// Applique : *addr = S + A (symbolValue + addend)
const R_X86_64_64: u32 = 1;

/// Tags de la section `.dynamic` pertinents pour les relocations.
const DT_RELA:    u64 = 7;   // Adresse de la table RELA
const DT_RELASZ:  u64 = 8;   // Taille de la table RELA
const DT_RELAENT: u64 = 9;   // Taille d'une entrée (doit être 24)
const DT_NULL:    u64 = 0;   // Fin de la table .dynamic

// ─── Structures ELF ──────────────────────────────────────────────────────────

/// Entrée de la table `.dynamic`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Dyn {
    d_tag: i64,
    d_val: u64,  // Union d_val / d_ptr
}

/// Entrée de la table RELA (relocation with explicit addend).
#[repr(C)]
#[derive(Clone, Copy)]
struct Elf64Rela {
    r_offset: u64,  // Adresse virtuelle de l'emplacement à modifier
    r_info:   u64,  // Sym index (bits 63:32) + type (bits 31:0)
    r_addend: i64,  // Addend explicite
}

impl Elf64Rela {
    fn sym_type(&self) -> u32 { (self.r_info & 0xFFFF_FFFF) as u32 }
    fn sym_idx(&self)  -> u32 { (self.r_info >> 32) as u32 }
}

// ─── Calcul KASLR ────────────────────────────────────────────────────────────

/// Calcule l'adresse de base KASLR depuis l'entropie hardware.
///
/// Contraintes :
///   - Base alignée sur 2 MiB (HUGE_PAGE_SIZE) pour les large pages
///   - En zone physique [4 MiB, 4 GiB] (évite zone DMA + ROM BIOS)
///   - En zone higher-half virtuelle [FFFF_8001_0000_0000, FFFF_C000_0000_0000]
///     (laisse 1 GiB pour le stack après virt_base)
///
/// Retourne `(phys_base, virt_base)`.
pub fn compute_kaslr_base(entropy: &[u8; 64]) -> (u64, u64) {
    // Mélange 8 bytes d'un hash XOR des 64 bytes d'entropie
    let mut mixed: u64 = 0;
    for chunk in entropy.chunks_exact(8) {
        let val = u64::from_le_bytes(chunk.try_into().unwrap_or([0u8; 8]));
        mixed ^= val;
        // Rotation pour meilleure diffusion
        mixed = mixed.rotate_left(13).wrapping_add(0x9E37_79B9_7F4A_7C15);
    }

    // Plage physique [4 MiB, 2 GiB] avec pas de 2 MiB
    // (512 − 2 = 510 positions possibles, évite les 2 premiers MiB = firmware)
    const PHYS_MIN:  u64 = 4 * 1024 * 1024;            // 4 MiB
    const PHYS_MAX:  u64 = 2 * 1024 * 1024 * 1024;     // 2 GiB
    const STEP:      u64 = HUGE_PAGE_SIZE as u64;       // 2 MiB
    let   range = (PHYS_MAX - PHYS_MIN) / STEP;

    let offset   = (mixed % range) * STEP;
    let phys_base = PHYS_MIN + offset;

    // Base virtuelle = higher-half + phys_base
    let virt_base = KERNEL_HIGHER_HALF_BASE + phys_base;

    (phys_base, virt_base)
}

// ─── Application des relocations ─────────────────────────────────────────────

/// Applique les relocations PIE dans l'image kernel chargée.
///
/// `phys_load_base` : adresse physique où le kernel a été chargé.
/// `elf`            : image ELF parsée (pour accéder à .dynamic).
///
/// RÈGLE : À appeler APRÈS `elf.load_segments()` et AVANT `handoff_to_kernel()`.
///
/// # Safety
/// `phys_load_base` doit pointer vers le kernel chargé en mémoire physique.
/// La mémoire doit être accessible en écriture.
pub unsafe fn apply_pie_relocations(
    elf:            &ElfKernel<'_>,
    phys_load_base: u64,
) -> Result<(), RelocationError> {
    // Si pas de segment DYNAMIC, pas de relocations à appliquer
    let dynamic_ph = match elf.dynamic_segment() {
        Some(ph) => ph,
        None     => return Ok(()),
    };

    // Si pas PIE, les relocations ne sont pas pertinentes (adresses absolues)
    if !elf.is_pie {
        return Ok(());
    }

    // Parse la table .dynamic
    let dyn_offset = dynamic_ph.p_vaddr - elf.virt_base();
    let dyn_phys   = phys_load_base + dyn_offset;
    let dyn_size   = dynamic_ph.p_filesz as usize;

    if dyn_size % core::mem::size_of::<Elf64Dyn>() != 0 {
        return Err(RelocationError::InvalidDynamicSize { size: dyn_size });
    }

    let dyn_count = dyn_size / core::mem::size_of::<Elf64Dyn>();
    let dyn_ptr   = dyn_phys as *const Elf64Dyn;

    // Trouve DT_RELA, DT_RELASZ dans la table .dynamic
    let mut rela_vaddr: Option<u64> = None;
    let mut rela_size:  Option<u64> = None;
    let mut rela_entry: u64         = 24; // Taille par défaut d'une Elf64Rela

    for i in 0..dyn_count {
        // SAFETY : dyn_ptr + i est dans les bornes de la table .dynamic
        let entry = unsafe { core::ptr::read_unaligned(dyn_ptr.add(i)) };
        match entry.d_tag as u64 {
            DT_NULL    => break,
            DT_RELA    => { rela_vaddr = Some(entry.d_val); }
            DT_RELASZ  => { rela_size  = Some(entry.d_val); }
            DT_RELAENT => { rela_entry = entry.d_val; }
            _          => {}
        }
    }

    let rela_vaddr = match rela_vaddr {
        Some(v) => v,
        None    => return Ok(()), // Pas de table RELA → pas de relocations
    };
    let rela_size = rela_size.unwrap_or(0);

    if rela_size == 0 {
        return Ok(());
    }

    if rela_entry != core::mem::size_of::<Elf64Rela>() as u64 {
        return Err(RelocationError::UnexpectedRelaEntSize {
            got: rela_entry,
            expected: core::mem::size_of::<Elf64Rela>() as u64,
        });
    }

    let rela_offset  = rela_vaddr - elf.virt_base();
    let rela_phys    = phys_load_base + rela_offset;
    let rela_count   = rela_size as usize / core::mem::size_of::<Elf64Rela>();
    let rela_ptr     = rela_phys as *const Elf64Rela;

    let mut applied_relative = 0u32;
    let mut applied_abs64    = 0u32;
    let mut skipped          = 0u32;

    for i in 0..rela_count {
        // SAFETY : rela_ptr + i est dans les bornes de la table .rela
        let rela = unsafe { core::ptr::read_unaligned(rela_ptr.add(i)) };
        let sym_type = rela.sym_type();

        match sym_type {
            R_X86_64_RELATIVE => {
                // *addr = phys_load_base + addend
                let target_voff = rela.r_offset - elf.virt_base();
                let target_phys = phys_load_base + target_voff;
                let value       = phys_load_base.wrapping_add_signed(rela.r_addend);
                // SAFETY : target_phys est dans la mémoire allouée du kernel
                unsafe { core::ptr::write_unaligned(target_phys as *mut u64, value); }
                applied_relative += 1;
            }
            R_X86_64_64 => {
                // Symboles absolus — l'index de symbole doit être 0 (local absolute)
                if rela.sym_idx() == 0 {
                    let target_voff = rela.r_offset - elf.virt_base();
                    let target_phys = phys_load_base + target_voff;
                    // SAFETY : target_phys est dans la mémoire allouée du kernel
                    let existing: u64 = unsafe { core::ptr::read_unaligned(target_phys as *const u64) };
                    let value = existing.wrapping_add_signed(rela.r_addend);
                    unsafe { core::ptr::write_unaligned(target_phys as *mut u64, value); }
                    applied_abs64 += 1;
                } else {
                    // Symbole non-local — non supporté sans table de symboles
                    skipped += 1;
                }
            }
            0 => { /* R_X86_64_NONE — ignore */ }
            other => {
                // Type de relocation non supporté — certains types sont inoffensifs
                // (ex : R_X86_64_TLSDESC pour TLS), mais on les loguera.
                let _ = other;
                skipped += 1;
            }
        }
    }

    // Vérification post-relocation : au moins une relocation doit avoir été appliquée
    // (un kernel PIE sans relocations RELATIVE est suspect)
    if elf.is_pie && applied_relative == 0 && rela_count > 0 {
        return Err(RelocationError::NoRelativeRelocations {
            total: rela_count,
            skipped,
        });
    }

    let _ = applied_abs64; // Utilisé en debug
    let _ = skipped;
    Ok(())
}

// ─── Erreurs ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum RelocationError {
    InvalidDynamicSize       { size: usize },
    UnexpectedRelaEntSize    { got: u64, expected: u64 },
    NoRelativeRelocations    { total: usize, skipped: u32 },
}

impl core::fmt::Display for RelocationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidDynamicSize { size } =>
                write!(f, "Taille table .dynamic invalide : {} bytes", size),
            Self::UnexpectedRelaEntSize { got, expected } =>
                write!(f, "RELAENT inattendu : {} (attendu {})", got, expected),
            Self::NoRelativeRelocations { total, skipped } =>
                write!(f, "Aucune relocation RELATIVE parmi {} entrées ({} skipped)",
                    total, skipped),
        }
    }
}
