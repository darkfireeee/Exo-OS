// drivers/fs/src/fat32/alloc.rs
//
// FAT32 — Logique d'allocation  (exo-os-driver-fs)
// RÈGLE FS-FAT32-05 : Les fonctions de cette couche indiquent QUELLE FAT écrire.
//   L'I/O réelle doit écrire sur FAT1 ET FAT2 (appeler deux fois write_entry_to_buf).

use core::sync::atomic::{AtomicU32, Ordering};
use super::bpb::ParsedBpb;
use super::fat_table::is_free;

static LAST_ALLOC_HINT: AtomicU32 = AtomicU32::new(2);

/// Réinitialise le hint d'allocation (appeler à l'unmount).
pub fn alloc_reset_hint() {
    LAST_ALLOC_HINT.store(2, Ordering::Relaxed);
}

/// Cherche le prochain cluster libre dans un tampon d'entrées FAT.
/// `fat_entries` : tranche d'u32 représentant les entrées FAT (déjà masquées).
/// `bpb` : BPB parsé (pour connaître cluster_count).
/// Retourne le numéro du cluster libre, ou None si plus d'espace.
pub fn find_free_cluster(fat_entries: &[u32], bpb: &ParsedBpb) -> Option<u32> {
    let total = bpb.cluster_count as usize;
    let max_cluster = (total + 2).min(fat_entries.len());
    let hint = (LAST_ALLOC_HINT.load(Ordering::Relaxed) as usize)
        .max(2)
        .min(max_cluster);

    for pass in 0..2 {
        let start = if pass == 0 { hint } else { 2 };
        let end   = if pass == 0 { max_cluster } else { hint };
        for i in start..end {
            if is_free(fat_entries[i]) {
                LAST_ALLOC_HINT.store(((i + 1).max(2).min(max_cluster)) as u32, Ordering::Relaxed);
                return Some(i as u32);
            }
        }
    }
    None
}
