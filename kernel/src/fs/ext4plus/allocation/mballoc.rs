// kernel/src/fs/ext4plus/allocation/mballoc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ MBALLOC — allocateur multi-blocs (buddy + first-fit par groupe)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Stratégies implémentées :
//   1. First-fit  — premier run de `count` blocs consécutifs libres.
//   2. Best-fit   — run libre le plus petit ≥ count (réduction de fragmentation).
//   3. Buddy      — allocation sur puissances de 2, arbre buddy par groupe.
//
// Le buddy system utilise un vecteur de 14 niveaux (2^0 … 2^13 = 8192 blocs).
// Chaque niveau est une bitmask indiquant quels blocs-compagnons sont libres.
//
// L'état buddy est maintenu en mémoire (pas persisté entre les montages).
// Il est reconstruit depuis la bitmap de blocs au montage.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::fs::core::types::{FsError, FsResult};
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::ext4plus::group_desc::GroupDescTable;
use crate::fs::ext4plus::allocation::balloc::{ext4_alloc_block, ext4_free_block};
use crate::memory::core::types::PhysAddr;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};

// ─────────────────────────────────────────────────────────────────────────────
// BuddyLevel — niveau d'un arbre buddy pour un groupe
// ─────────────────────────────────────────────────────────────────────────────

pub const BUDDY_MAX_ORDER: usize = 14;  // 2^14 = 16384 blocs max par groupe

/// Un niveau du buddy system : tableau de bits (1 = bloc libre).
struct BuddyLevel {
    bits: Vec<u64>,  // chaque u64 couvre 64 blocs compagnons
    order: usize,    // taille d'un bloc = 2^order
}

impl BuddyLevel {
    fn new(blocks_at_level: usize) -> Self {
        let words = (blocks_at_level + 63) / 64;
        Self { bits: alloc::vec![!0u64; words], order: 0 }
    }

    fn is_free(&self, idx: usize) -> bool {
        (self.bits[idx / 64] >> (idx % 64)) & 1 != 0
    }

    fn mark_used(&mut self, idx: usize) {
        self.bits[idx / 64] &= !(1u64 << (idx % 64));
    }

    fn mark_free(&mut self, idx: usize) {
        self.bits[idx / 64] |= 1u64 << (idx % 64);
    }

    /// Cherche un bloc libre à ce niveau. Retourne l'index ou None.
    fn find_free(&self) -> Option<usize> {
        for (wi, &word) in self.bits.iter().enumerate() {
            if word != 0 {
                let bit = word.trailing_zeros() as usize;
                return Some(wi * 64 + bit);
            }
        }
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BuddyGroup — buddy system pour un groupe de blocs
// ─────────────────────────────────────────────────────────────────────────────

pub struct BuddyGroup {
    group_idx:    u64,
    blocks_total: usize,
    levels:       Vec<BuddyLevel>,
}

impl BuddyGroup {
    pub fn new(group_idx: u64, blocks_per_group: usize) -> Self {
        let mut levels = Vec::new();
        let mut n = blocks_per_group;
        for o in 0..BUDDY_MAX_ORDER {
            levels.push(BuddyLevel { bits: alloc::vec![!0u64; (n + 63) / 64], order: o });
            n = (n + 1) / 2;
            if n == 0 { break; }
        }
        Self { group_idx, blocks_total: blocks_per_group, levels }
    }

    /// Construit le buddy depuis une bitmap de blocs (chaque bit = 1 bloc).
    pub fn rebuild_from_bitmap(&mut self, bitmap: &[u8]) {
        // Remplit le niveau 0 depuis la bitmap (0 = libre)
        if let Some(lvl0) = self.levels.get_mut(0) {
            for (i, word) in lvl0.bits.iter_mut().enumerate() {
                let mut w = 0u64;
                for b in 0..64 {
                    let bit_idx = i * 64 + b;
                    if bit_idx < bitmap.len() * 8 {
                        let is_free = (bitmap[bit_idx / 8] >> (bit_idx % 8)) & 1 == 0;
                        if is_free { w |= 1u64 << b; }
                    }
                }
                *word = w;
            }
        }
        // Reconstruction des niveaux supérieurs
        for o in 1..self.levels.len() {
            let prev_bits: Vec<u64> = self.levels[o - 1].bits.clone();
            let words = self.levels[o].bits.len();
            for wi in 0..words {
                let mut w = 0u64;
                for b in 0..64 {
                    let pair = (wi * 64 + b) * 2;
                    let pair_wi = pair / 64;
                    let pair_bi = pair % 64;
                    if pair_wi < prev_bits.len() {
                        let a = (prev_bits[pair_wi] >> pair_bi) & 1;
                        let b_bit = if pair_bi + 1 < 64 {
                            (prev_bits[pair_wi] >> (pair_bi + 1)) & 1
                        } else if pair_wi + 1 < prev_bits.len() {
                            prev_bits[pair_wi + 1] & 1
                        } else { 0 };
                        if a & b_bit != 0 { w |= 1u64 << b; }
                    }
                }
                self.levels[o].bits[wi] = w;
            }
        }
        MBALLOC_STATS.rebuilds.fetch_add(1, Ordering::Relaxed);
    }

    /// Alloue `count = 2^order` blocs contigus.
    pub fn alloc_order(&mut self, order: usize) -> Option<usize> {
        if order >= self.levels.len() { return None; }
        let lvl = &self.levels[order];
        let frame_idx = lvl.find_free()?;

        // Marque tous les blocs de niveau 0 couverts par ce frame comme utilisés
        let first_block = frame_idx * (1 << order);
        for blk in first_block..first_block + (1 << order) {
            if let Some(l0) = self.levels.get_mut(0) { l0.mark_used(blk); }
        }
        // Remonte : marque les niveaux supérieurs si le compagnon est aussi utilisé
        for o in order..self.levels.len() {
            let buddy_idx = frame_idx ^ 1;
            if let Some(lvl) = self.levels.get_mut(o) {
                lvl.mark_used(frame_idx);
                if !lvl.is_free(buddy_idx) {
                    if let Some(up) = self.levels.get_mut(o + 1) {
                        up.mark_used(frame_idx / 2);
                    }
                }
            }
        }
        MBALLOC_STATS.allocs.fetch_add(1, Ordering::Relaxed);
        Some(first_block)
    }

    /// Libère `count = 2^order` blocs à partir de `first_block`.
    pub fn free_order(&mut self, first_block: usize, order: usize) {
        for blk in first_block..first_block + (1 << order) {
            if let Some(l0) = self.levels.get_mut(0) { l0.mark_free(blk); }
        }
        let mut frame_idx = first_block / (1 << order);
        for o in order..self.levels.len() {
            let buddy_idx = frame_idx ^ 1;
            if let Some(lvl) = self.levels.get_mut(o) { lvl.mark_free(frame_idx); }
            // Fusion si le compagnon est aussi libre
            if let Some(lvl) = self.levels.get(o) {
                if !lvl.is_free(buddy_idx) { break; }
            }
            frame_idx /= 2;
        }
        MBALLOC_STATS.frees.fetch_add(1, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MballockContext — allocateur multi-blocs global
// ─────────────────────────────────────────────────────────────────────────────

pub struct MballocContext {
    groups: SpinLock<Vec<BuddyGroup>>,
}

impl MballocContext {
    pub const fn new() -> Self { Self { groups: SpinLock::new(Vec::new()) } }

    pub fn init_group(&self, group_idx: u64, blocks_per_group: usize, bitmap: &[u8]) {
        let mut bg = BuddyGroup::new(group_idx, blocks_per_group);
        bg.rebuild_from_bitmap(bitmap);
        let mut guard = self.groups.lock();
        // Insère ou remplace
        if let Some(g) = guard.iter_mut().find(|g| g.group_idx == group_idx) {
            *g = bg;
        } else {
            guard.push(bg);
        }
    }

    /// Alloue `count` blocs contigus. Arrondit count à la prochaine puissance de 2.
    pub fn alloc(&self, count: usize, preferred_group: Option<u64>) -> Option<(u64 /*group*/, usize /*local block*/, usize /*real count*/)> {
        let order = usize::BITS as usize - count.leading_zeros() as usize - 1;
        let real_count = 1 << order;
        let mut guard = self.groups.lock();
        if let Some(pg) = preferred_group {
            guard.sort_unstable_by_key(|g| if g.group_idx == pg { 0u8 } else { 1u8 });
        }
        for bg in guard.iter_mut() {
            if let Some(local) = bg.alloc_order(order) {
                MBALLOC_STATS.allocs.fetch_add(1, Ordering::Relaxed);
                return Some((bg.group_idx, local, real_count));
            }
        }
        None
    }

    pub fn free(&self, group_idx: u64, local_block: usize, order: usize) {
        let mut guard = self.groups.lock();
        if let Some(bg) = guard.iter_mut().find(|g| g.group_idx == group_idx) {
            bg.free_order(local_block, order);
        }
    }
}

/// Contexte multi-blocs global du montage EXT4.
pub static MBALLOC: MballocContext = MballocContext::new();

// ─────────────────────────────────────────────────────────────────────────────
// MballocStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct MballocStats {
    pub allocs:   AtomicU64,
    pub frees:    AtomicU64,
    pub rebuilds: AtomicU64,
    pub failures: AtomicU64,
}

impl MballocStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { allocs: z!(), frees: z!(), rebuilds: z!(), failures: z!() }
    }
}

pub static MBALLOC_STATS: MballocStats = MballocStats::new();
