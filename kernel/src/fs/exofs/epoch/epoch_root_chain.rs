// kernel/src/fs/exofs/epoch/epoch_root_chain.rs
//
// =============================================================================
// Sérialisation/désérialisation de l'EpochRoot en chaîne de pages disque
// Ring 0 · no_std · Exo-OS
// =============================================================================
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
// RÈGLE RECUR-01 : itération itérative, jamais récursive.
// RÈGLE ARITH-02 : saturating_add/div_ceil pour toute arithmétique.

use core::fmt;
use core::mem::size_of;

use alloc::vec::Vec;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, DiskOffset,
    EPOCH_ROOT_MAGIC,
};
use crate::fs::exofs::core::flags::EpochFlags;
use crate::fs::exofs::epoch::epoch_root::{
    EpochRootInMemory, EpochRootEntry, EpochRootPageHeader,
    verify_epoch_root_page,
};
use crate::fs::exofs::epoch::epoch_checksum::seal_epoch_root_page;
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;

// =============================================================================
// Constantes de layout de page
// =============================================================================

/// Taille d'une page EpochRoot (1 bloc de 4096 octets).
pub const EPOCH_ROOT_PAGE_SIZE: usize = 4096;

/// Taille de l'en-tête + du checksum final dans une page.
const OVERHEAD: usize = size_of::<EpochRootPageHeader>() + 32;

/// Nombre maximum d'EpochRootEntry par page.
pub const ENTRIES_PER_PAGE: usize =
    (EPOCH_ROOT_PAGE_SIZE - OVERHEAD) / size_of::<EpochRootEntry>();

/// Offset du checksum dans une page (les 32 derniers octets).
const CHECKSUM_OFFSET: usize = EPOCH_ROOT_PAGE_SIZE - 32;

// =============================================================================
// PageStats — statistiques d'une page sérialisée
// =============================================================================

/// Statistiques d'une page EpochRoot sérialisée.
#[derive(Copy, Clone, Debug)]
pub struct PageStats {
    /// Index de cette page dans la chaîne.
    pub page_index:     u32,
    /// Nombre d'entrées dans cette page.
    pub entry_count:    u32,
    /// Offset disque de cette page (0 si non encore alloué).
    pub disk_offset:    DiskOffset,
    /// Vrai si c'est la dernière page de la chaîne (next_page == 0).
    pub is_last:        bool,
    /// Taille utilisée (header + entries) en octets.
    pub used_bytes:     usize,
}

// =============================================================================
// ChainStats — statistiques de la chaîne complète
// =============================================================================

/// Statistiques de la chaîne de pages EpochRoot.
#[derive(Clone, Debug)]
pub struct ChainStats {
    pub epoch_id:      EpochId,
    pub page_count:    u32,
    pub total_entries: u32,
    pub empty_pages:   u32,
    pub full_pages:    u32,
    pub partial_pages: u32,
    pub total_bytes:   usize,
}

impl fmt::Display for ChainStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChainStats{{ epoch={} pages={} entries={} bytes={} }}",
            self.epoch_id.0,
            self.page_count,
            self.total_entries,
            self.total_bytes,
        )
    }
}

// =============================================================================
// count_pages_needed — calcul du nombre de pages
// =============================================================================

/// Calcule le nombre de pages nécessaires pour `total_entries` entrées.
///
/// RÈGLE ARITH-02 : division correcte (div_ceil).
pub fn count_pages_needed(total_entries: usize) -> usize {
    if total_entries == 0 {
        return 1;
    }
    // div_ceil(total_entries, ENTRIES_PER_PAGE)
    total_entries.saturating_add(ENTRIES_PER_PAGE - 1) / ENTRIES_PER_PAGE
}

// =============================================================================
// Sérialisation de la chaîne
// =============================================================================

/// Sérialise un EpochRootInMemory en une liste de pages de 4096 octets.
///
/// Chaque page est autonome (magic + entries + checksum).
/// La dernière page a next_page = 0.
/// Le champ `next_page` des pages intermédiaires est initialisé à
/// `EPOCH_CHAIN_NEXT_PLACEHOLDER` (sera mis à jour après allocation disque).
///
/// RÈGLE CHAIN-01 : next_page calculé et inclus AVANT le checksum.
/// RÈGLE OOM-02   : try_reserve(EPOCH_ROOT_PAGE_SIZE)? avant push.
/// RÈGLE RECUR-01 : boucle itérative.
pub fn serialize_epoch_root_chain(root: &EpochRootInMemory) -> ExofsResult<Vec<Vec<u8>>> {
    let total_modified = root.modified_objects.len();
    let total_entries  = root.total_entries();
    let page_count     = count_pages_needed(total_entries);
    let flags_raw      = root.flags.0;

    let mut pages: Vec<Vec<u8>> = Vec::new();
    pages.try_reserve(page_count).map_err(|_| ExofsError::NoMemory)?;

    for page_idx in 0..page_count {
        let mut page: Vec<u8> = Vec::new();
        page.try_reserve(EPOCH_ROOT_PAGE_SIZE).map_err(|_| ExofsError::NoMemory)?;
        page.resize(EPOCH_ROOT_PAGE_SIZE, 0u8);

        let entry_start = page_idx * ENTRIES_PER_PAGE;
        let entry_end   = (entry_start.saturating_add(ENTRIES_PER_PAGE)).min(total_entries);
        let page_entry_count = entry_end.saturating_sub(entry_start);

        // Placeholder pour next_page : l'appelant le remplaçe après allocation.
        let next_page: u64 = if page_idx + 1 < page_count {
            EPOCH_CHAIN_NEXT_PLACEHOLDER
        } else {
            0 // Dernière page.
        };

        let hdr = EpochRootPageHeader {
            magic:       EPOCH_ROOT_MAGIC,
            version:     1,
            flags:       flags_raw,
            epoch_id:    root.epoch_id.0,
            entry_count: page_entry_count as u32,
            page_index:  page_idx as u32,
            next_page,
            checksum:    [0u8; 32],
        };

        // Sérialiser l'en-tête.
        let hdr_size = size_of::<EpochRootPageHeader>();
        // SAFETY: EpochRootPageHeader est #[repr(C, packed)], Copy, 64 octets.
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(&hdr as *const _ as *const u8, hdr_size)
        };
        page[..hdr_size].copy_from_slice(hdr_bytes);

        // Sérialiser les entrées de cette page.
        let entry_size = size_of::<EpochRootEntry>();
        let mut offset = hdr_size;
        for i in entry_start..entry_end {
            let entry: Option<&EpochRootEntry> = if i < total_modified {
                Some(&root.modified_objects[i])
            } else {
                None
            };
            if let Some(e) = entry {
                // SAFETY: EpochRootEntry est #[repr(C, packed)], Copy, 48 octets.
                let bytes = unsafe {
                    core::slice::from_raw_parts(e as *const _ as *const u8, entry_size)
                };
                if offset + entry_size <= CHECKSUM_OFFSET {
                    page[offset..offset + entry_size].copy_from_slice(bytes);
                }
                offset = offset.saturating_add(entry_size);
            }
        }

        // Sceller la page (checksum Blake3 dans les 32 derniers octets).
        seal_epoch_root_page(&mut page)?;
        pages.push(page);
    }

    // Statistiques.
    if page_count > 1 {
        EPOCH_STATS.inc_chained_root_pages();
    }

    Ok(pages)
}

/// Valeur de remplacement pour `next_page` avant allocation disque.
/// L'appelant doit remplacer toutes les occurrences par l'offset réel.
pub const EPOCH_CHAIN_NEXT_PLACEHOLDER: u64 = 0xDEAD_BEEF_DEAD_BEEF;

// =============================================================================
// rebuild_chain_offsets — mise à jour des next_page après allocation disque
// =============================================================================

/// Met à jour les offsets `next_page` dans la chaîne sérialisée.
///
/// # Paramètres
/// - `pages`         : liste de pages sérialisées (sortie de serialize_epoch_root_chain).
/// - `disk_offsets`  : offsets disque correspondants (len == pages.len()).
///
/// RÈGLE CHAIN-01 : next_page est inclus dans le checksum → scellement re-nécessaire.
/// RÈGLE ARITH-02 : bornes vérifiées avant tout accès.
pub fn rebuild_chain_offsets(
    pages:        &mut [Vec<u8>],
    disk_offsets: &[DiskOffset],
) -> ExofsResult<()> {
    if pages.len() != disk_offsets.len() {
        return Err(ExofsError::CorruptedStructure);
    }
    let n = pages.len();
    // RÈGLE RECUR-01 : boucle.
    for i in 0..n {
        if pages[i].len() < EPOCH_ROOT_PAGE_SIZE {
            return Err(ExofsError::CorruptedStructure);
        }
        // Calcule l'offset next_page pour cette page.
        let next: u64 = if i + 1 < n {
            disk_offsets[i + 1].0
        } else {
            0 // Dernière page.
        };
        // Écrit next_page dans l'en-tête à l'offset correct.
        // Layout de EpochRootPageHeader : magic(4)+version(2)+flags(2)+epoch(8)+count(4)+idx(4)+next(8) = offset 24.
        let next_offset = 4 + 2 + 2 + 8 + 4 + 4; // = 24.
        let next_bytes = next.to_le_bytes();
        pages[i][next_offset..next_offset + 8].copy_from_slice(&next_bytes);
        // Re-sceller (checksum inclut next_page).
        seal_epoch_root_page(&mut pages[i])?;
    }
    Ok(())
}

// =============================================================================
// Désérialisation d'une chaîne de pages
// =============================================================================

/// Désérialise une chaîne de pages EpochRoot en liste d'EpochRootEntry.
///
/// RÈGLE CHAIN-01 : chaque page est vérifiée (magic + checksum) avant lecture.
/// RÈGLE RECUR-01 : boucle itérative.
pub fn deserialize_epoch_root_chain(pages: &[Vec<u8>]) -> ExofsResult<Vec<EpochRootEntry>> {
    let capacity = pages.len() * ENTRIES_PER_PAGE;
    let mut result: Vec<EpochRootEntry> = Vec::new();
    result.try_reserve(capacity).map_err(|_| ExofsError::NoMemory)?;

    let hdr_size   = size_of::<EpochRootPageHeader>();
    let entry_size = size_of::<EpochRootEntry>();

    // RÈGLE RECUR-01 : boucle itérative.
    for page in pages {
        // RÈGLE CHAIN-01 : vérification avant lecture.
        verify_epoch_root_page(page)?;

        if page.len() < hdr_size.saturating_add(32) {
            return Err(ExofsError::CorruptedStructure);
        }

        // Lecture de l'en-tête.
        // SAFETY: page.as_ptr est aligné, hdr_size = 64, EpochRootPageHeader #[repr(C, packed)].
        let hdr: EpochRootPageHeader = unsafe {
            core::ptr::read_unaligned(page.as_ptr() as *const EpochRootPageHeader)
        };

        let entry_count = hdr.entry_count as usize;
        let max_entries = page.len()
            .saturating_sub(hdr_size)
            .saturating_sub(32)
            / entry_size;

        if entry_count > max_entries {
            return Err(ExofsError::CorruptedStructure);
        }

        result.try_reserve(entry_count).map_err(|_| ExofsError::NoMemory)?;

        let mut offset = hdr_size;
        for _ in 0..entry_count {
            if offset + entry_size > CHECKSUM_OFFSET {
                return Err(ExofsError::CorruptedStructure);
            }
            // SAFETY: EpochRootEntry est #[repr(C, packed)], taille 48, Copy.
            let entry: EpochRootEntry = unsafe {
                core::ptr::read_unaligned(page[offset..].as_ptr() as *const EpochRootEntry)
            };
            result.push(entry);
            offset = offset.saturating_add(entry_size);
        }
    }

    Ok(result)
}

// =============================================================================
// validate_chain_integrity — validation d'une chaîne complète
// =============================================================================

/// Valide l'intégrité d'une chaîne de pages EpochRoot.
///
/// Vérifie :
/// 1. Chaque page a un magic valide (CHAIN-01).
/// 2. Chaque page a un checksum valide (CHAIN-01).
/// 3. Les page_index sont consécutifs.
/// 4. La dernière page a next_page == 0.
/// 5. Toutes les pages partagent le même epoch_id.
///
/// RÈGLE RECUR-01 : boucle itérative.
pub fn validate_chain_integrity(pages: &[Vec<u8>]) -> ExofsResult<ChainStats> {
    if pages.is_empty() {
        return Err(ExofsError::CorruptedStructure);
    }
    let hdr_size = size_of::<EpochRootPageHeader>();
    let mut first_epoch_id: Option<u64> = None;
    let mut total_entries: u32 = 0;
    let mut total_bytes: usize = 0;
    let mut empty_pages: u32 = 0;
    let mut full_pages: u32 = 0;
    let mut partial_pages: u32 = 0;

    for (idx, page) in pages.iter().enumerate() {
        // RÈGLE CHAIN-01.
        verify_epoch_root_page(page)?;
        if page.len() < hdr_size {
            return Err(ExofsError::CorruptedStructure);
        }
        // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
        let hdr: EpochRootPageHeader = unsafe {
            core::ptr::read_unaligned(page.as_ptr() as *const EpochRootPageHeader)
        };
        // Vérifie l'epoch_id cohérent.
        match first_epoch_id {
            None       => { first_epoch_id = Some(hdr.epoch_id); }
            Some(eid)  => {
                if hdr.epoch_id != eid {
                    return Err(ExofsError::CorruptedStructure);
                }
            }
        }
        // Vérifie l'index consécutif.
        if hdr.page_index as usize != idx {
            return Err(ExofsError::CorruptedStructure);
        }
        // Vérifie que la dernière page a next_page == 0.
        if idx == pages.len() - 1 && hdr.next_page != 0 {
            return Err(ExofsError::CorruptedStructure);
        }
        let cnt = hdr.entry_count as usize;
        total_entries = total_entries.saturating_add(cnt as u32);
        total_bytes   = total_bytes.saturating_add(hdr_size + cnt * size_of::<EpochRootEntry>() + 32);
        match cnt {
            0               => empty_pages   += 1,
            c if c >= ENTRIES_PER_PAGE => full_pages    += 1,
            _               => partial_pages += 1,
        }
    }

    Ok(ChainStats {
        epoch_id:      EpochId(first_epoch_id.unwrap_or(0)),
        page_count:    pages.len() as u32,
        total_entries,
        empty_pages,
        full_pages,
        partial_pages,
        total_bytes,
    })
}

