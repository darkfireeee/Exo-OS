//! ExoForge — reconstruction de Kernel A (Phase 3.7)
//!
//! Checklist post-reconstruction obligatoire (G9) :
//! 1. FACS RO re-marqué dans PTEs de A
//! 2. Hash MADT vérifié inchangé
//! 3. TLB shootdown IPI 0xF3 broadcast
//! 4. IDT de A contient 0xF1/0xF2/0xF3
//!
//! Erreurs couvertes : G3, G9, S-N1

use core::sync::atomic::Ordering;

use crate::arch::x86_64::apic::{self, local_apic, x2apic};
use crate::arch::x86_64::cpu::msr;
use crate::arch::x86_64::idt;
use crate::exophoenix::stage0;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::BlobId;
use crate::memory::dma::iommu::{AMD_IOMMU, INTEL_VTD};
use xmas_elf::ElfFile;

// ── MARQUEURS POUR GPT-5.3-CODEX ─────────────────────────────────────────
// Les lignes marquées [ADAPT] nécessitent la substitution des noms d'API
// réels du codebase. Tout le reste est figé et ne doit pas être modifié.
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeError {
    ExoFsLoadFailed,
    ElfParseFailed,
    MerkleVerifyFailed,
    DriverResetFailed,
    ChecklistFailed(&'static str),
}

// Hash Blake3 connu de l'image propre de A — établi au Stage 0
// [ADAPT] : remplacer par la vraie constante ou statique du codebase
static A_IMAGE_HASH: [u8; 32] = [0u8; 32]; // [ADAPT] hash réel ici

// Racine Merkle connue de .text + .rodata de A
// [ADAPT] : remplacer par la vraie constante ou statique du codebase  
static A_MERKLE_ROOT: [u8; 32] = [0u8; 32]; // [ADAPT] hash réel ici

// ── Étape 1 : charger l'image de A depuis ExoFS ───────────────────────────

fn load_a_image_from_exofs() -> Result<&'static [u8], ForgeError> {
    let blob_id = BlobId(A_IMAGE_HASH);
    let data = BLOB_CACHE
        .get(&blob_id)
        .ok_or(ForgeError::ExoFsLoadFailed)?;
    let leaked: &'static mut [u8] = alloc::boxed::Box::leak(data);
    Ok(leaked)
}

// ── Étape 2 : parser ELF — safe Rust uniquement ───────────────────────────

struct ElfImage<'a> {
    text:   &'a [u8],
    rodata: &'a [u8],
    data:   &'a [u8],
    bss_start: u64,
    bss_size:  usize,
    entry:     u64,
}

fn parse_elf_safe(image: &[u8]) -> Result<ElfImage<'_>, ForgeError> {
    // Vérification magic ELF en-tête
    if image.len() < 64 {
        return Err(ForgeError::ElfParseFailed);
    }
    const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
    if &image[0..4] != &ELF_MAGIC {
        return Err(ForgeError::ElfParseFailed);
    }
    let elf = ElfFile::new(image).map_err(|_| ForgeError::ElfParseFailed)?;

    let mut text: Option<&[u8]> = None;
    let mut rodata: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    let mut bss_start: u64 = 0;
    let mut bss_size: usize = 0;

    for section in elf.section_iter() {
        let Ok(name) = section.get_name(&elf) else { continue; };

        match name {
            ".text" | ".rodata" | ".data" => {
                let off = usize::try_from(section.offset()).map_err(|_| ForgeError::ElfParseFailed)?;
                let sz = usize::try_from(section.size()).map_err(|_| ForgeError::ElfParseFailed)?;
                let end = off.checked_add(sz).ok_or(ForgeError::ElfParseFailed)?;
                if end > image.len() {
                    return Err(ForgeError::ElfParseFailed);
                }
                let slice = &image[off..end];
                match name {
                    ".text" => text = Some(slice),
                    ".rodata" => rodata = Some(slice),
                    ".data" => data = Some(slice),
                    _ => {}
                }
            }
            ".bss" => {
                bss_start = section.address();
                bss_size = usize::try_from(section.size()).map_err(|_| ForgeError::ElfParseFailed)?;
            }
            _ => {}
        }
    }

    Ok(ElfImage {
        text: text.ok_or(ForgeError::ElfParseFailed)?,
        rodata: rodata.ok_or(ForgeError::ElfParseFailed)?,
        data: data.unwrap_or(&[]),
        bss_start,
        bss_size,
        entry: elf.header.pt2.entry_point(),
    })
}

// ── Étape 3 : vérification Merkle ─────────────────────────────────────────

fn verify_merkle(elf: &ElfImage<'_>) -> Result<(), ForgeError> {
    // Hash Blake3 de .text ++ .rodata comparé à A_MERKLE_ROOT
    // [ADAPT] : utiliser le blake3 existant du codebase
    // Pattern attendu :
    //   let mut hasher = blake3::Hasher::new();
    //   hasher.update(elf.text);
    //   hasher.update(elf.rodata);
    //   let hash = hasher.finalize();
    //   if hash.as_bytes() != &A_MERKLE_ROOT {
    //       return Err(ForgeError::MerkleVerifyFailed);
    //   }
    let mut hasher = crate::security::crypto::blake3::Blake3Hasher::new();
    hasher.update(elf.text).update(elf.rodata);
    let mut computed = [0u8; 32];
    hasher.finalize(&mut computed);

    if computed != A_MERKLE_ROOT {
        return Err(ForgeError::MerkleVerifyFailed);
    }
    Ok(())
}

fn validate_elf_layout(elf: &ElfImage<'_>) -> Result<(), ForgeError> {
    // Validation structurelle minimale du parser ELF avant reconstruction.
    if elf.text.is_empty() || elf.rodata.is_empty() {
        return Err(ForgeError::ElfParseFailed);
    }
    if elf.entry == 0 {
        return Err(ForgeError::ElfParseFailed);
    }

    // Bornes défensives sur .bss (anti-overflow / anti-objet malformé).
    let _bss_end = elf
        .bss_start
        .checked_add(elf.bss_size as u64)
        .ok_or(ForgeError::ElfParseFailed)?;
    if elf.bss_size > (64 * 1024 * 1024) {
        return Err(ForgeError::ElfParseFailed);
    }

    // Touch explicite de .data: section valide mais possiblement vide.
    let _data_len = elf.data.len();
    Ok(())
}

// ── Étape 4 : reset drivers Ring 1 (G3) ──────────────────────────────────

fn pci_function_level_reset(bus: u8, device: u8, func: u8) -> Result<(), ForgeError> {
    // FLR : écrire bit 15 du PCI Express Device Control Register
    // [ADAPT] : utiliser l'API PCI existante du codebase
    // Pattern attendu :
    //   pci::config_write16(bus, device, func, PCI_EXP_DEVCTL, PCI_EXP_DEVCTL_BCR_FLR)
    let _ = (bus, device, func);
    Ok(())
}

fn drain_dma_queues(bus: u8, device: u8, func: u8) {
    // Attendre que les DMA en vol se terminent
    // [ADAPT] : utiliser l'API DMA existante du codebase si disponible
    // Fallback : busy-wait 200µs (timeout drain par device class)
    let deadline = stage0::ticks_per_us()
        .saturating_mul(200)
        .saturating_add(read_apic_ticks() as u64);
    while (read_apic_ticks() as u64) < deadline {
        core::hint::spin_loop();
    }
    let _ = (bus, device, func);
}

#[inline(always)]
fn read_apic_ticks() -> u32 {
    match stage0::B_FEATURES.apic_mode() {
        stage0::BootApicMode::X2Apic => unsafe {
            msr::read_msr(x2apic::X2APIC_TIMER_CCR) as u32
        },
        stage0::BootApicMode::XApic => local_apic::timer_current_count(),
    }
}

fn iotlb_flush_after_flr() {
    let blocked = stage0::blocked_domain_id();
    if INTEL_VTD.is_initialized() && INTEL_VTD.unit_count() > 0 {
        unsafe { INTEL_VTD.flush_iotlb_domain(blocked as u16, 0); }
    } else if AMD_IOMMU.is_initialized() {
        core::sync::atomic::fence(Ordering::SeqCst);
    }
}

fn reload_driver_binary_from_exofs(
    bus: u8, device: u8, func: u8,
) -> Result<(), ForgeError> {
    // [ADAPT] : charger le binaire du driver depuis ExoFS par son hash connu
    // Pattern attendu :
    //   let hash = DRIVER_HASH_TABLE.get(bus, device, func)?;
    //   exofs::load_and_map_driver(hash)
    let _ = (bus, device, func);
    Ok(())
}

fn reset_all_ring1_drivers() -> Result<(), ForgeError> {
    // Itérer sur les devices connus de B_DEVICE_TABLE (construite au Stage 0)
    let device_count = stage0::b_device_count();
    for i in 0..device_count {
        let Some(dev) = stage0::b_device(i) else { continue };
        // G3 : séquence obligatoire — FLR → drain → IOTLB → reload
        pci_function_level_reset(dev.bus, dev.device, dev.function)?;
        drain_dma_queues(dev.bus, dev.device, dev.function);
        iotlb_flush_after_flr();
        reload_driver_binary_from_exofs(dev.bus, dev.device, dev.function)?;
    }
    Ok(())
}

// ── Étape 5 : checklist post-reconstruction (G9) ─────────────────────────

fn checklist_facs_ro() -> Result<(), ForgeError> {
    // Re-marquer FACS read-only dans les PTEs de A
    // [ADAPT] : appeler la fonction déjà implémentée dans stage0.rs
    // Pattern attendu :
    //   stage0::mark_facs_ro_in_a_pts(&stage0::ACPI_FACS_PHYS);
    let acpi = stage0::parse_stage0_acpi();
    if acpi.facs_phys == 0 {
        return Err(ForgeError::ChecklistFailed("facs_missing"));
    }
    if !stage0::mark_facs_ro_in_a_pts(acpi.facs_phys) {
        return Err(ForgeError::ChecklistFailed("facs_ro_failed"));
    }
    Ok(())
}

fn checklist_madt_hash() -> Result<(), ForgeError> {
    // Vérifier que le hash MADT stocké au Stage 0 n'a pas changé
    // [ADAPT] : comparer stage0::MADT_HASH avec le hash recalculé
    // Pattern attendu :
    //   let current = stage0::hash_madt_current();
    //   if current != stage0::MADT_HASH.load(Ordering::Acquire) {
    //       return Err(ForgeError::ChecklistFailed("madt_hash_mismatch"));
    //   }
    let acpi = stage0::parse_stage0_acpi();
    if acpi.madt_phys == 0 {
        return Err(ForgeError::ChecklistFailed("madt_missing"));
    }

    // MADT SDT length à +4.
    let madt_len = unsafe {
        core::ptr::read_unaligned((acpi.madt_phys as usize + 4) as *const u32)
    } as usize;
    if !(36..=256 * 1024).contains(&madt_len) {
        return Err(ForgeError::ChecklistFailed("madt_len_invalid"));
    }

    let madt_bytes = unsafe {
        core::slice::from_raw_parts(acpi.madt_phys as *const u8, madt_len)
    };
    let current = crate::security::crypto::blake3::blake3_hash(madt_bytes);
    let expected = stage0::madt_hash();
    if current != expected {
        return Err(ForgeError::ChecklistFailed("madt_hash_mismatch"));
    }
    Ok(())
}

fn checklist_tlb_shootdown() {
    // IPI 0xF3 broadcast — invalider TLB de tous les cores de A
    if apic::is_x2apic() {
        x2apic::broadcast_ipi_except_self_x2apic(idt::VEC_EXOPHOENIX_TLB);
    } else {
        local_apic::broadcast_ipi_except_self(idt::VEC_EXOPHOENIX_TLB);
    }
    // Attendre les ACK TLB dans la SSR
    let ticks_per_us = stage0::ticks_per_us();
    let deadline = (read_apic_ticks() as u64)
        .saturating_add(ticks_per_us.saturating_mul(100));
    while (read_apic_ticks() as u64) < deadline {
        core::hint::spin_loop();
    }
}

fn checklist_idt_has_exophoenix_vectors() -> Result<(), ForgeError> {
    // Vérifier que l'IDT de A contient les vecteurs 0xF1/0xF2/0xF3
    // [ADAPT] : lire les entrées IDT de A via accès physique direct
    // Pattern attendu :
    //   let idt_phys = read_a_idtr();
    //   verify_idt_entry(idt_phys, 0xF1)?;
    //   verify_idt_entry(idt_phys, 0xF2)?;
    //   verify_idt_entry(idt_phys, 0xF3)?;
    #[repr(C, packed)]
    struct Idtr {
        limit: u16,
        base: u64,
    }

    let mut idtr = Idtr { limit: 0, base: 0 };
    // SAFETY: lecture de l'IDTR courant en ring0.
    unsafe {
        core::arch::asm!(
            "sidt [{ptr}]",
            ptr = in(reg) &mut idtr,
            options(nostack, preserves_flags)
        );
    }

    let has_vector = |vector: u8| -> bool {
        let entry_size = 16usize;
        let off = (vector as usize).saturating_mul(entry_size);
        if off + (entry_size - 1) > idtr.limit as usize {
            return false;
        }
        let flags_ptr = (idtr.base as usize + off + 5) as *const u8;
        let flags = unsafe { core::ptr::read_volatile(flags_ptr) };
        let present = (flags & 0x80) != 0;
        let gate = flags & 0x0F;
        present && (gate == 0x0E || gate == 0x0F)
    };

    if !has_vector(idt::VEC_EXOPHOENIX_FREEZE) {
        return Err(ForgeError::ChecklistFailed("idt_missing_f1"));
    }
    if !has_vector(idt::VEC_EXOPHOENIX_PMC) {
        return Err(ForgeError::ChecklistFailed("idt_missing_f2"));
    }
    if !has_vector(idt::VEC_EXOPHOENIX_TLB) {
        return Err(ForgeError::ChecklistFailed("idt_missing_f3"));
    }

    Ok(())
}

fn run_postconstruction_checklist() -> Result<(), ForgeError> {
    // Ordre obligatoire — ne pas modifier
    checklist_facs_ro()?;
    checklist_madt_hash()?;
    checklist_tlb_shootdown();   // pas de ? — toujours exécuté
    checklist_idt_has_exophoenix_vectors()?;
    Ok(())
}

// ── Point d'entrée principal ──────────────────────────────────────────────

/// Reconstruction de Kernel A depuis image propre ExoFS.
/// Appelé par handoff.rs après isolation confirmée.
/// Si Ok(()) → handoff.rs passe PHOENIX_STATE = Restore.
/// Si Err    → handoff.rs compte l'échec vers Degraded.
pub fn reconstruct_kernel_a() -> Result<(), ForgeError> {
    // 1. Charger depuis ExoFS
    let image = load_a_image_from_exofs()?;

    // 2. Parser ELF (safe Rust)
    let elf = parse_elf_safe(image)?;

    // Validation complémentaire des sections/entry extraites.
    validate_elf_layout(&elf)?;

    // 3. Vérifier Merkle
    verify_merkle(&elf)?;

    // 4. Reset drivers Ring 1 (G3)
    reset_all_ring1_drivers()?;

    // 5. Checklist post-reconstruction (G9)
    run_postconstruction_checklist()?;

    Ok(())
}
