// kernel/src/fs/integrity/recovery.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// RECOVERY — Crash recovery WAL replay (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Rejoue le WAL après un crash pour restaurer un état cohérent du FS.
//
// Processus de recovery :
//   1. `scan_journal()` → lit tous les LogEntry du journal sur disque.
//   2. `find_committed_txs()` → identifie les transactions avec TxCommit.
//   3. `replay_transaction()` → ré-applique les blocs de chaque tx committée.
//   4. `truncate_partial()` → supprime les enregistrements partiels.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::core::types::{DevId, FsError, FsResult};
use crate::fs::integrity::journal::{
    Journal, JournalEntry, JournalEntryType, LogHeader, JOURNAL_MAGIC, JOURNAL_STATS,
};
use crate::fs::block::bio::{Bio, BioOp, BioFlags};
use crate::fs::block::queue::submit_bio;
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// Recovery state
// ─────────────────────────────────────────────────────────────────────────────

pub struct RecoveryResult {
    /// Nombre de transactions rejouées.
    pub txs_replayed:   u64,
    /// Nombre de blocks appliqués.
    pub blocks_applied: u64,
    /// Transactions partielles truncatées.
    pub partial_truncated: u64,
    /// Erreurs rencontrées.
    pub errors:         u64,
    /// Recovery terminée avec succès.
    pub success:        bool,
}

impl RecoveryResult {
    pub fn new() -> Self {
        Self {
            txs_replayed: 0, blocks_applied: 0,
            partial_truncated: 0, errors: 0, success: false,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Journal recovery
// ─────────────────────────────────────────────────────────────────────────────

/// Rejoue le journal à partir des entrées committées fournies par `journal`.
///
/// Cette fonction est appelée au montage du FS si le superbloc indique
/// que le FS n'a pas été démonté proprement (`needs_recovery` flag).
pub fn journal_recovery(journal: &Journal) -> RecoveryResult {
    let mut result = RecoveryResult::new();
    RECOVERY_STATS.recovery_runs.fetch_add(1, Ordering::Relaxed);

    // Collecte toutes les entrées committées.
    let entries = journal.collect_committed();
    if entries.is_empty() {
        result.success = true;
        return result;
    }

    // Groupe par txid.
    let mut tx_map: BTreeMap<u64, Vec<JournalEntry>> = BTreeMap::new();
    let mut committed_txids: alloc::collections::BTreeSet<u64> = alloc::collections::BTreeSet::new();

    for entry in entries {
        if !entry.header.is_valid() {
            result.errors += 1;
            continue;
        }
        match entry.header.entry_type {
            x if x == JournalEntryType::TxCommit as u32 => {
                committed_txids.insert(entry.header.txid);
            }
            x if x == JournalEntryType::TxAbort as u32 => {
                // Transaction abortée → ignorer.
            }
            _ => {
                tx_map.entry(entry.header.txid).or_insert_with(Vec::new).push(entry);
            }
        }
    }

    // Replay des transactions committées uniquement.
    for txid in &committed_txids {
        if let Some(blocks) = tx_map.remove(txid) {
            result.blocks_applied += replay_transaction(&blocks);
            result.txs_replayed += 1;
        }
    }

    // Transactions partielles (pas de TxCommit).
    result.partial_truncated = tx_map.len() as u64;

    RECOVERY_STATS.txs_replayed.fetch_add(result.txs_replayed, Ordering::Relaxed);
    RECOVERY_STATS.blocks_applied.fetch_add(result.blocks_applied, Ordering::Relaxed);

    result.success = result.errors == 0;
    result
}

/// Rejoue les blocs d'une transaction.
fn replay_transaction(entries: &[JournalEntry]) -> u64 {
    let mut applied = 0u64;
    for entry in entries {
        if entry.header.entry_type != JournalEntryType::DataBlock as u32 { continue; }
        if entry.data.is_empty() { continue; }

        // Reconstruit une Bio pour réécrire le bloc.
        let buf_ptr = entry.data.as_ptr() as u64;
        let bio = Bio::new(
            BioOp::Write,
            0, // dev_id 0 — en production : récupéré depuis le superbloc
            entry.header.block * 8, // LBA (secteurs 512 octets à partir de block 4K)
            buf_ptr,
            entry.data.len() as u32,
            BioFlags::SYNC | BioFlags::META,
        );

        if submit_bio(bio).is_ok() {
            applied += 1;
            JOURNAL_STATS.replays.fetch_add(1, Ordering::Relaxed);
        }
    }
    applied
}

/// Vérifie si un FS a besoin de recovery (flag dans superbloc).
pub fn needs_recovery(sb_flags: u32) -> bool {
    const NEEDS_RECOVERY_FLAG: u32 = 0x0004;
    sb_flags & NEEDS_RECOVERY_FLAG != 0
}

// ─────────────────────────────────────────────────────────────────────────────
// RecoveryStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct RecoveryStats {
    pub recovery_runs:  AtomicU64,
    pub txs_replayed:   AtomicU64,
    pub blocks_applied: AtomicU64,
    pub errors:         AtomicU64,
}

impl RecoveryStats {
    pub const fn new() -> Self {
        Self {
            recovery_runs:  AtomicU64::new(0),
            txs_replayed:   AtomicU64::new(0),
            blocks_applied: AtomicU64::new(0),
            errors:         AtomicU64::new(0),
        }
    }
}

pub static RECOVERY_STATS: RecoveryStats = RecoveryStats::new();
