// kernel/src/fs/exofs/epoch/epoch_root_chain.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Sérialisation/désérialisation de l'EpochRoot en chaîne de pages disque
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Quand le nombre d'entrées dépasse ce qu'une seule page peut tenir,
// l'EpochRoot est fragmenté en une chaîne de pages.
//
// Structure de chaque page :
//   [EpochRootPageHeader (64B)] [N × EpochRootEntry (48B each)] [padding] [checksum (32B)]
//
// RÈGLE CHAIN-01 : next_page EST INCLUS dans le checksum de la page courante.
// RÈGLE EPOCH-07 : magic 0x45504F43 dans chaque page.
// RÈGLE OOM-02   : try_reserve avant push.

use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
    EPOCH_ROOT_MAGIC,
};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::epoch::epoch_root::{
    EpochRootInMemory, EpochRootEntry, EpochRootPageHeader,
};
use crate::fs::exofs::epoch::epoch_checksum::seal_epoch_root_page;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes de layout de page
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une page EpochRoot (1 bloc de 4096 octets).
pub const EPOCH_ROOT_PAGE_SIZE: usize = 4096;

/// Taille de l'en-tête + du checksum.
const OVERHEAD: usize = core::mem::size_of::<EpochRootPageHeader>() + 32;

/// Nombre maximum d'EpochRootEntry par page.
const ENTRIES_PER_PAGE: usize =
    (EPOCH_ROOT_PAGE_SIZE - OVERHEAD) / core::mem::size_of::<EpochRootEntry>();

// ─────────────────────────────────────────────────────────────────────────────
// Sérialisation de la chaîne
// ─────────────────────────────────────────────────────────────────────────────

/// Sérialise un EpochRootInMemory en une liste de pages de 4096 octets.
///
/// Chaque page est autonome (magic + entries + checksum).
/// La dernière page a next_page = 0.
///
/// RÈGLE CHAIN-01 : next_page calculé et inclus AVANT le checksum.
/// RÈGLE OOM-02   : try_reserve(EPOCH_ROOT_PAGE_SIZE)? avant push.
pub fn serialize_epoch_root_chain(root: &EpochRootInMemory) -> ExofsResult<Vec<Vec<u8>>> {
    // Construire la liste complète des entrées (modifiés + supprimés comme entries DELETE).
    let total = root.total_entries();
    let page_count = if total == 0 {
        1
    } else {
        total.div_ceil(ENTRIES_PER_PAGE)
    };

    let mut pages: Vec<Vec<u8>> = Vec::new();
    pages.try_reserve(page_count).map_err(|_| ExofsError::NoMemory)?;

    for page_idx in 0..page_count {
        let mut page = Vec::with_capacity(EPOCH_ROOT_PAGE_SIZE);
        page.try_reserve(EPOCH_ROOT_PAGE_SIZE).map_err(|_| ExofsError::NoMemory)?;
        page.resize(EPOCH_ROOT_PAGE_SIZE, 0u8);

        // En-tête.
        let entry_start = page_idx * ENTRIES_PER_PAGE;
        let entry_end = ((page_idx + 1) * ENTRIES_PER_PAGE).min(total);
        let page_entry_count = entry_end.saturating_sub(entry_start);
        let next_page: u64 = if page_idx + 1 < page_count { 0xDEAD_BEEF_0000_0000 } else { 0 };
        // NOTE : next_page = 0xDEAD_BEEF... est un placeholder — l'appelant
        //        doit remplacer après allocation disque.

        let hdr = EpochRootPageHeader {
            magic:        EPOCH_ROOT_MAGIC,
            version:      1,
            flags:        root.flags.0,
            epoch_id:     root.epoch_id.0,
            entry_count:  page_entry_count as u32,
            page_index:   page_idx as u32,
            next_page,
            checksum:     [0u8; 32],
        };

        // Sérialiser l'en-tête dans la page.
        let hdr_size = core::mem::size_of::<EpochRootPageHeader>();
        // SAFETY: hdr est un #[repr(C, packed)] de taille 64 octets.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, hdr_size)
        };
        page[..hdr_size].copy_from_slice(hdr_bytes);

        // Sérialiser les entrées.
        let modified_entries = &root.modified_objects;
        let mut offset = hdr_size;
        for i in entry_start..entry_end {
            if i < modified_entries.len() {
                let entry = &modified_entries[i];
                let entry_size = core::mem::size_of::<EpochRootEntry>();
                // SAFETY: EpochRootEntry est #[repr(C, packed)], Copy, 48 octets.
                let bytes = unsafe {
                    core::slice::from_raw_parts(entry as *const _ as *const u8, entry_size)
                };
                page[offset..offset + entry_size].copy_from_slice(bytes);
                offset += entry_size;
            }
        }

        // Sceller la page (écrire le checksum dans les 32 derniers octets).
        seal_epoch_root_page(&mut page)?;

        pages.push(page);

        if page_count > 1 {
            EPOCH_STATS.inc_chained_root_pages();
        }
    }

    Ok(pages)
}

// ─────────────────────────────────────────────────────────────────────────────
// Désérialisation d'une chaîne de pages
// ─────────────────────────────────────────────────────────────────────────────

/// Désérialise une chaîne de pages EpochRoot en liste d'EpochRootEntry.
///
/// RÈGLE CHAIN-01 : chaque page est vérifiée (magic + checksum) avant lecture.
/// Retourne Err si une page est corrompue.
pub fn deserialize_epoch_root_chain(pages: &[Vec<u8>]) -> ExofsResult<Vec<EpochRootEntry>> {
    use crate::fs::exofs::epoch::epoch_root::verify_epoch_root_page;

    let mut result: Vec<EpochRootEntry> = Vec::new();

    for page in pages {
        // RÈGLE CHAIN-01 : vérification avant lecture.
        verify_epoch_root_page(page)?;

        let hdr_size = core::mem::size_of::<EpochRootPageHeader>();
        if page.len() < hdr_size + 32 {
            return Err(ExofsError::CorruptedStructure);
        }

        // Lecture de l'en-tête.
        // SAFETY: page[..hdr_size] est aligné et correctement dimensionné.
        let hdr: EpochRootPageHeader = unsafe {
            core::ptr::read_unaligned(page.as_ptr() as *const EpochRootPageHeader)
        };

        let entry_count = hdr.entry_count as usize;
        let entry_size  = core::mem::size_of::<EpochRootEntry>();
        let max_entries = (page.len().saturating_sub(hdr_size + 32)) / entry_size;

        if entry_count > max_entries {
            return Err(ExofsError::CorruptedStructure);
        }

        result.try_reserve(entry_count).map_err(|_| ExofsError::NoMemory)?;

        let mut offset = hdr_size;
        for _ in 0..entry_count {
            // SAFETY: slice de entry_size octets aligné sur un EpochRootEntry #[repr(C, packed)].
            let entry: EpochRootEntry = unsafe {
                core::ptr::read_unaligned(page[offset..].as_ptr() as *const EpochRootEntry)
            };
            result.push(entry);
            offset += entry_size;
        }
    }

    Ok(result)
}
