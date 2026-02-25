// kernel/src/fs/ext4plus/allocation/prealloc.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// EXT4+ PREALLOC — pré-allocation de blocs pour les écritures séquentielles
// ═══════════════════════════════════════════════════════════════════════════════
//
// La pré-allocation réduit la fragmentation lors d'écritures séquentielles
// (fichiers créés ou agrandis progressivement).
//
// Principe :
//   • À chaque write(), si l'inode n'a pas de réserve, on alloue `PREALLOC_BLOCKS`
//     blocs d'un coup (via mballoc) et on les place dans un PreallocWindow.
//   • Les blocs suivants sont servis depuis la fenêtre sans accès disque.
//   • À la fermeture du fichier (release), les blocs non utilisés sont libérés.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::vec::Vec;
use alloc::sync::Arc;

use crate::fs::core::types::{FsError, FsResult, InodeNumber};
use crate::fs::ext4plus::allocation::mballoc::MBALLOC;
use crate::fs::ext4plus::superblock::Ext4Superblock;
use crate::fs::ext4plus::group_desc::GroupDescTable;
use crate::scheduler::sync::{spinlock::SpinLock, rwlock::RwLock};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Nombre de blocs pré-alloués par fenêtre (doit être une puissance de 2).
pub const PREALLOC_BLOCKS: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// PreallocWindow — fenêtre de pré-allocation pour un inode
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PreallocWindow {
    pub ino:          InodeNumber,
    pub group_idx:    u64,
    pub local_start:  usize,         // premier bloc local (dans le groupe)
    pub count:        usize,         // blocs réservés
    pub used:         usize,         // blocs effectivement utilisés
    pub logical_hint: u64,           // prochain bloc logique attendu
}

impl PreallocWindow {
    pub fn next_block(&mut self) -> Option<u64> {
        if self.used >= self.count { return None; }
        let blk = self.group_idx * 32768 + (self.local_start + self.used) as u64;
        self.used += 1;
        PREALLOC_STATS.served_from_window.fetch_add(1, Ordering::Relaxed);
        Some(blk)
    }

    pub fn remaining(&self) -> usize { self.count - self.used }

    pub fn release_unused(&self, sb: &Arc<RwLock<Ext4Superblock>>, gdt: &GroupDescTable) {
        if self.remaining() == 0 { return; }
        let order = PREALLOC_BLOCKS.trailing_zeros() as usize;
        MBALLOC.free(self.group_idx, self.local_start + self.used, order);
        PREALLOC_STATS.released_blocks.fetch_add(self.remaining() as u64, Ordering::Relaxed);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PreallocManager — gère les fenêtres par inode
// ─────────────────────────────────────────────────────────────────────────────

pub struct PreallocManager {
    windows: SpinLock<Vec<PreallocWindow>>,
}

impl PreallocManager {
    pub const fn new() -> Self { Self { windows: SpinLock::new(Vec::new()) } }

    /// Retourne le prochain bloc pré-alloué pour `ino`, ou en crée un nouvel fenêtre.
    pub fn get_or_alloc(
        &self,
        ino:           InodeNumber,
        logical_block: u64,
        preferred_grp: Option<u64>,
        sb:            &Arc<RwLock<Ext4Superblock>>,
        _gdt:          &GroupDescTable,
    ) -> FsResult<u64> {
        let mut guard = self.windows.lock();

        // Cherche une fenêtre active pour cet inode correspondant à la position
        if let Some(w) = guard.iter_mut().find(|w| w.ino == ino && w.remaining() > 0) {
            if let Some(blk) = w.next_block() { return Ok(blk); }
        }

        // Crée une nouvelle fenêtre via mballoc
        let (group_idx, local_start, real_count) =
            MBALLOC.alloc(PREALLOC_BLOCKS, preferred_grp).ok_or(FsError::NoSpace)?;

        let mut win = PreallocWindow {
            ino,
            group_idx,
            local_start,
            count:        real_count,
            used:         0,
            logical_hint: logical_block,
        };
        let blk = win.next_block().ok_or(FsError::NoSpace)?;
        guard.push(win);
        PREALLOC_STATS.windows_created.fetch_add(1, Ordering::Relaxed);
        Ok(blk)
    }

    /// Libère les blocs non utilisés d'un inode (appel lors de release()).
    pub fn release_inode(
        &self,
        ino: InodeNumber,
        sb:  &Arc<RwLock<Ext4Superblock>>,
        gdt: &GroupDescTable,
    ) {
        let mut guard = self.windows.lock();
        guard.retain(|w| {
            if w.ino == ino {
                w.release_unused(sb, gdt);
                false
            } else { true }
        });
    }
}

pub static PREALLOC_MGR: PreallocManager = PreallocManager::new();

// ─────────────────────────────────────────────────────────────────────────────
// PreallocStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct PreallocStats {
    pub windows_created:    AtomicU64,
    pub served_from_window: AtomicU64,
    pub released_blocks:    AtomicU64,
    pub alloc_failures:     AtomicU64,
}

impl PreallocStats {
    pub const fn new() -> Self {
        macro_rules! z { () => { AtomicU64::new(0) }; }
        Self { windows_created: z!(), served_from_window: z!(), released_blocks: z!(), alloc_failures: z!() }
    }
}

pub static PREALLOC_STATS: PreallocStats = PreallocStats::new();
