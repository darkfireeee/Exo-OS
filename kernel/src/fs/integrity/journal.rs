// kernel/src/fs/integrity/journal.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// JOURNAL — Write-Ahead Log (WAL) (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// WAL pour la cohérence des métadonnées FS sur crash.
//
// Architecture :
//   • Anneau circulaire de `LogEntry` sur le device de journal.
//   • `JournalHandle` = transaction active (start → commit/abort).
//   • `journal_start()` → ouvre une transaction.
//   • `journal_write()` → loggue un bloc modifié dans la transaction.
//   • `journal_commit()` → marque la transaction committée (TxCommit record).
//   • `journal_abort()` → annule la transaction (blocs non replay).
//   • `journal_checkpoint()` → libère les entrées déjà appliqées (GC).
//
// Format d'entrée :
//   [LogHeader: type | txid | seq | checksum] [données...]
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{DevId, FsError, FsResult};
use crate::fs::integrity::checksum::{crc32c, ChecksumType};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// JournalMode — mode d'opération du WAL (RÈGLE FS-EXT4P-02)
// ─────────────────────────────────────────────────────────────────────────────

/// Mode WAL contrôlant ce qui est journalé.
///
/// RÈGLE FS-EXT4P-02 : ext4plus DOIT utiliser `DataOrdered`.
///   Séquence obligatoire :
///     1. Écrire les DONNÉES à l'emplacement final sur disque (bio submit)
///     2. Attendre ACK disque (émettre une barrière)
///     3. Seulement ALORS : écrire les MÉTADONNÉES dans le journal
///     4. Commiter le journal
///   INTERDIT : commiter les métadonnées avant que `data_barrier_passed` soit true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalMode {
    /// Mode Data=Ordered (ext4plus) : WAL sur métadonnées SEULES.
    /// Données → disque final AVANT commit journal (barrière obligatoire).
    DataOrdered,
    /// Mode Data=Journal : données ET métadonnées dans le WAL (double écriture).
    /// Plus sûr en cas de crash mais deux fois plus d'écritures.
    DataJournal,
    /// Mode Data=Writeback : métadonnées seules, données asynchrones sans ordre.
    /// Performances maximales, risque de corruption des données.
    DataWriteback,
}

// ─────────────────────────────────────────────────────────────────────────────
// Types de records
// ─────────────────────────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalEntryType {
    TxStart  = 1,
    DataBlock= 2,
    TxCommit = 3,
    TxAbort  = 4,
    Checkpoint= 5,
    Superblock= 6,
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures on-disk
// ─────────────────────────────────────────────────────────────────────────────

/// Header d'un enregistrement de journal (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct LogHeader {
    /// Magic de validation.
    pub magic:    u32,
    /// Type d'entrée.
    pub entry_type: u32,
    /// ID de transaction.
    pub txid:     u64,
    /// Numéro de séquence dans la transaction.
    pub seq:      u32,
    /// Numéro de bloc cible (0 si N/A).
    pub block:    u64,
    /// CRC32c du header (les 28 premiers bytes).
    pub checksum: u32,
}

pub const JOURNAL_MAGIC: u32 = 0xC03B3998;

impl LogHeader {
    pub fn new(entry_type: JournalEntryType, txid: u64, seq: u32, block: u64) -> Self {
        let mut h = Self {
            magic: JOURNAL_MAGIC,
            entry_type: entry_type as u32,
            txid,
            seq,
            block,
            checksum: 0,
        };
        // Calcule le checksum sur les 28 premiers bytes.
        let bytes = &[
            h.magic.to_le_bytes().as_ref(),
            h.entry_type.to_le_bytes().as_ref(),
            h.txid.to_le_bytes().as_ref(),
            h.seq.to_le_bytes().as_ref(),
        ].concat();
        h.checksum = crc32c(bytes);
        h
    }

    pub fn is_valid(&self) -> bool {
        if self.magic != JOURNAL_MAGIC { return false; }
        let bytes = &[
            self.magic.to_le_bytes().as_ref(),
            self.entry_type.to_le_bytes().as_ref(),
            self.txid.to_le_bytes().as_ref(),
            self.seq.to_le_bytes().as_ref(),
        ].concat();
        crc32c(bytes) == self.checksum
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JournalEntry — entrée en mémoire
// ─────────────────────────────────────────────────────────────────────────────

pub struct JournalEntry {
    pub header: LogHeader,
    /// Données du bloc (jusqu'à 4 KiB).
    pub data:   Vec<u8>,
}

// ─────────────────────────────────────────────────────────────────────────────
// JournalHandle — handle de transaction
// ─────────────────────────────────────────────────────────────────────────────

/// Handle d'une transaction de journal active.
pub struct JournalHandle {
    pub txid:      u64,
    /// Mode WAL de cette transaction (déterminé au moment de start()).
    pub mode:      JournalMode,
    entries:       SpinLock<Vec<JournalEntry>>,
    seq:           AtomicU32,
    committed:     AtomicBool,
    aborted:       AtomicBool,
    /// RÈGLE FS-EXT4P-02 : vrai ssi la barrière disque a été émise et acquittée.
    /// En mode DataOrdered, `journal_commit()` REFUSE de procéder si false.
    pub data_barrier_passed: AtomicBool,
}

impl JournalHandle {
    fn new(txid: u64, mode: JournalMode) -> Self {
        Self {
            txid,
            mode,
            entries: SpinLock::new(Vec::new()),
            seq:     AtomicU32::new(0),
            committed: AtomicBool::new(false),
            aborted:   AtomicBool::new(false),
            data_barrier_passed: AtomicBool::new(false),
        }
    }

    /// Siégnale que la barrière disque a été émise et l'ACK reçu.
    /// DOIT être appelé entre l'écriture des données et le commit journal.
    /// RÈGLE FS-EXT4P-02.
    pub fn set_data_barrier_passed(&self) {
        self.data_barrier_passed.store(true, Ordering::Release);
        JOURNAL_STATS.barriers_passed.fetch_add(1, Ordering::Relaxed);
    }

    /// Loggue un bloc modifié.
    pub fn write_block(&self, block: u64, data: &[u8]) -> FsResult<()> {
        if self.committed.load(Ordering::Relaxed) || self.aborted.load(Ordering::Relaxed) {
            return Err(FsError::InvalArg);
        }
        let seq  = self.seq.fetch_add(1, Ordering::Relaxed);
        let hdr  = LogHeader::new(JournalEntryType::DataBlock, self.txid, seq, block);
        let data = data.to_vec();
        self.entries.lock().push(JournalEntry { header: hdr, data });
        JOURNAL_STATS.blocks_logged.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn is_committed(&self) -> bool { self.committed.load(Ordering::Relaxed) }
    pub fn is_aborted(&self)  -> bool  { self.aborted.load(Ordering::Relaxed) }
    pub fn entry_count(&self) -> usize { self.entries.lock().len() }
}

pub type JournalHandleRef = Arc<JournalHandle>;

// ─────────────────────────────────────────────────────────────────────────────
// Journal — gestionnaire principal
// ─────────────────────────────────────────────────────────────────────────────

pub struct Journal {
    dev:        DevId,
    /// Entrées committées en attente de writeback.
    committed:  SpinLock<Vec<JournalEntry>>,
    /// Dernier txid assigné.
    next_txid:  AtomicU64,
    /// Generation pour invalidation.
    generation: AtomicU64,
    pub active: AtomicBool,
}

impl Journal {
    pub fn new(dev: DevId) -> Self {
        Self {
            dev,
            committed:  SpinLock::new(Vec::new()),
            next_txid:  AtomicU64::new(1),
            generation: AtomicU64::new(0),
            active:     AtomicBool::new(true),
        }
    }

    /// Ouvre une nouvelle transaction.
    pub fn start(&self) -> FsResult<JournalHandleRef> {
        self.start_with_mode(JournalMode::DataOrdered)
    }

    /// Ouvre une transaction avec un mode WAL explicite.
    pub fn start_with_mode(&self, mode: JournalMode) -> FsResult<JournalHandleRef> {
        if !self.active.load(Ordering::Relaxed) {
            return Err(FsError::ReadOnly);
        }
        let txid = self.next_txid.fetch_add(1, Ordering::Relaxed);
        JOURNAL_STATS.tx_started.fetch_add(1, Ordering::Relaxed);
        Ok(Arc::new(JournalHandle::new(txid, mode)))
    }

    /// Committe une transaction (écrit les entrées dans `committed`).
    ///
    /// RÈGLE FS-EXT4P-02 : En mode DataOrdered, le commit est refusé
    /// si `data_barrier_passed` n'est pas vrai — les données DOIVENT
    /// être sur le disque physique avant que les métadonnées soient commitées.
    pub fn commit(&self, handle: &JournalHandle) -> FsResult<()> {
        if handle.aborted.load(Ordering::Relaxed) {
            return Err(FsError::Io);
        }
        // Vérification barrière Data=Ordered
        if handle.mode == JournalMode::DataOrdered
            && !handle.data_barrier_passed.load(Ordering::Acquire)
        {
            JOURNAL_STATS.barrier_violations.fetch_add(1, Ordering::Relaxed);
            return Err(FsError::InvalArg); // Barrière non émise — refus de commit
        }
        handle.committed.store(true, Ordering::Release);
        let entries: Vec<JournalEntry> = {
            let mut lock = handle.entries.lock();
            core::mem::take(&mut *lock)
        };
        // Ajoute le record TxCommit.
        let commit_hdr = LogHeader::new(
            JournalEntryType::TxCommit,
            handle.txid,
            handle.seq.load(Ordering::Relaxed),
            0,
        );
        let mut committed = self.committed.lock();
        committed.extend(entries);
        committed.push(JournalEntry { header: commit_hdr, data: Vec::new() });
        JOURNAL_STATS.tx_committed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Abandonne une transaction.
    pub fn abort(&self, handle: &JournalHandle) {
        handle.aborted.store(true, Ordering::Release);
        handle.entries.lock().clear();
        JOURNAL_STATS.tx_aborted.fetch_add(1, Ordering::Relaxed);
    }

    /// Checkpoint : libère les entrées déjà appliquées.
    pub fn checkpoint(&self, up_to_txid: u64) -> usize {
        let mut committed = self.committed.lock();
        let before = committed.len();
        committed.retain(|e| e.header.txid > up_to_txid);
        let freed = before - committed.len();
        JOURNAL_STATS.checkpoint_entries.fetch_add(freed as u64, Ordering::Relaxed);
        freed
    }

    /// Collecte les entrées committées pour replay.
    pub fn collect_committed(&self) -> Vec<JournalEntry> {
        let mut c = self.committed.lock();
        core::mem::take(&mut *c)
    }

    pub fn pending_count(&self) -> usize {
        self.committed.lock().len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JournalStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct JournalStats {
    pub tx_started:         AtomicU64,
    pub tx_committed:       AtomicU64,
    pub tx_aborted:         AtomicU64,
    pub blocks_logged:      AtomicU64,
    pub checkpoint_entries: AtomicU64,
    pub replays:            AtomicU64,
    /// Compteur de barrières disque émises (Data=Ordered).
    pub barriers_passed:     AtomicU64,
    /// Violations de la règle barrière (métadonnées commitées avant données).
    pub barrier_violations:  AtomicU64,
}

impl JournalStats {
    pub const fn new() -> Self {
        Self {
            tx_started:         AtomicU64::new(0),
            tx_committed:       AtomicU64::new(0),
            tx_aborted:         AtomicU64::new(0),
            blocks_logged:      AtomicU64::new(0),
            checkpoint_entries: AtomicU64::new(0),
            replays:            AtomicU64::new(0),
            barriers_passed:    AtomicU64::new(0),
            barrier_violations: AtomicU64::new(0),
        }
    }
}

pub static JOURNAL_STATS: JournalStats = JournalStats::new();
