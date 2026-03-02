//! heap_coalesce.rs — Fusion de blocs libres contigus (compaction heap ExoFS, no_std).

use crate::fs::exofs::storage::heap_free_map::HeapFreeMap;

/// Résultat d'une passe de coalescence.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoalesceResult {
    pub runs_merged:   u32,
    pub blocks_freed:  u64,
}

/// Fusionne les blocs libres contigus dans la carte de bits.
///
/// Cette opération est O(n) et doit être appelée hors du chemin critique
/// (GC, maintenance). Elle ne modifie pas la carte : elle retourne juste
/// un rapport.
pub struct HeapCoalescer;

impl HeapCoalescer {
    /// Compacte les runs libres (détecte la fragmentation mais ne déplace pas
    /// les données — la défragmentation physique est hors scope ici).
    pub fn analyze(map: &HeapFreeMap) -> CoalesceResult {
        let total  = map.total_blocks();
        let mut res = CoalesceResult::default();
        let mut in_run = false;

        for block in 0..total {
            if map.is_free(block) {
                if !in_run {
                    in_run   = true;
                    res.runs_merged    += 1;
                    res.blocks_freed   += 1;
                } else {
                    res.blocks_freed   += 1;
                }
            } else {
                in_run = false;
            }
        }
        // On ne compte le "merge" que quand il y a >1 runs.
        if res.runs_merged > 0 { res.runs_merged -= 1; }
        res
    }

    /// Fragmentation ratio en pourcents (0=compact, 100=très fragmenté).
    pub fn fragmentation_pct(map: &HeapFreeMap) -> u8 {
        let total = map.total_blocks();
        if total == 0 { return 0; }
        let free = map.free_blocks();
        if free == 0 { return 0; }

        // Compter le nombre de runs libres.
        let mut runs = 0u64;
        let mut prev_free = false;
        for b in 0..total {
            let f = map.is_free(b);
            if f && !prev_free { runs += 1; }
            prev_free = f;
        }

        // Utilisation optimale = 1 run. Fragmentation = (runs-1) / free * 100.
        if runs <= 1 { 0 } else { (((runs - 1) * 100) / free).min(100) as u8 }
    }
}
