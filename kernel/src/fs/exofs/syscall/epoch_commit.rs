//! epoch_commit.rs — SYS_EXOFS_EPOCH_COMMIT (518)
//!
//! Valide une epoch ExoFS : scelle le journal, avance le compteur,
//! invalide les entrées obsolètes du cache.
//! RECUR-01 / OOM-02 / ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, copy_struct_from_user, write_user_buf, EFAULT,
};
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const EPOCH_MAGIC:       u32 = 0x45_50_4F_43; // "EPOC"
pub const EPOCH_VERSION:     u8  = 1;
pub const EPOCH_HDR_SIZE:    usize = 40;
pub const EPOCH_MAX_ENTRIES: usize = 1024;
pub const EPOCH_JOURNAL_KEY: &[u8] = b"EPOCH_JOURNAL";

// ─────────────────────────────────────────────────────────────────────────────
// État global de l'epoch courante
// ─────────────────────────────────────────────────────────────────────────────

static CURRENT_EPOCH: AtomicU64 = AtomicU64::new(1);
static COMMIT_STATE:  AtomicU32 = AtomicU32::new(0); // 0=idle 1=in_progress

const STATE_IDLE:        u32 = 0;
const STATE_IN_PROGRESS: u32 = 1;

/// Retourne l'epoch courante.
pub fn current_epoch() -> u64 {
    CURRENT_EPOCH.load(Ordering::Acquire)
}

/// Retourne vrai si un commit est en cours.
pub fn commit_in_progress() -> bool {
    COMMIT_STATE.load(Ordering::Relaxed) == STATE_IN_PROGRESS
}

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod epoch_flags {
    pub const FORCE:        u32 = 0x0001;
    pub const VERIFY_CHECKSUM: u32 = 0x0002;
    pub const COMPACT:      u32 = 0x0004;
    pub const NO_ADVANCE:   u32 = 0x0008;
    pub const VALID_MASK:   u32 = FORCE | VERIFY_CHECKSUM | COMPACT | NO_ADVANCE;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochCommitArgs {
    pub flags:    u32,
    pub _pad:     u32,
    pub epoch_id: u64,
    pub checksum: u64,
    pub hints:    u64,
}

const _: () = assert!(core::mem::size_of::<EpochCommitArgs>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochCommitResult {
    pub new_epoch:    u64,
    pub sealed_epoch: u64,
    pub blobs_sealed: u64,
    pub bytes_sealed: u64,
    pub flags:        u32,
    pub _pad:         u32,
}

const _: () = assert!(core::mem::size_of::<EpochCommitResult>() == 40);

/// En-tête du journal d'epoch (40 octets).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochJournalHeader {
    pub magic:       u32,
    pub version:     u8,
    pub flags:       u8,
    pub _pad:        u16,
    pub epoch_id:    u64,
    pub entry_count: u32,
    pub checksum:    u64,
    pub _pad2:       u32,
}

const _: () = assert!(core::mem::size_of::<EpochJournalHeader>() == EPOCH_HDR_SIZE);

/// Entrée de journal (40 octets) : blob scellé.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochJournalEntry {
    pub blob_id:   [u8; 32],
    pub size:      u64,
}

const EPOCH_ENTRY_SIZE: usize = core::mem::size_of::<EpochJournalEntry>();

// ─────────────────────────────────────────────────────────────────────────────
// Journal d'epoch
// ─────────────────────────────────────────────────────────────────────────────

/// Dérive la clé du blob journal pour une epoch donnée.
fn journal_blob_id(epoch_id: u64) -> BlobId {
    let mut buf = [0u8; 21];
    let ep = epoch_id.to_le_bytes();
    let mut i = 0usize;
    while i < 8 { buf[i] = ep[i]; i = i.wrapping_add(1); }
    buf[8]  = b'E'; buf[9]  = b'P'; buf[10] = b'O'; buf[11] = b'C';
    buf[12] = b'H'; buf[13] = b'_'; buf[14] = b'J'; buf[15] = b'O';
    buf[16] = b'U'; buf[17] = b'R'; buf[18] = b'N'; buf[19] = b'A';
    buf[20] = b'L';
    BlobId::from_bytes_blake3(&buf)
}

/// Calcule un checksum simple (XOR des tailles + epoch).
/// ARITH-02 : wrapping_add.
fn compute_checksum(entries: &[EpochJournalEntry], epoch_id: u64) -> u64 {
    let mut cs = epoch_id;
    let mut i = 0usize;
    while i < entries.len() {
        cs = cs.wrapping_add(entries[i].size);
        i = i.wrapping_add(1);
    }
    cs
}

/// Charge le journal d'une epoch depuis le cache.
/// OOM-02 / RECUR-01.
fn load_journal(epoch_id: u64) -> ExofsResult<Vec<EpochJournalEntry>> {
    let jid = journal_blob_id(epoch_id);
    let data = match BLOB_CACHE.get(&jid) { Some(d) => d, None => return Ok(Vec::new()) };
    if data.len() < EPOCH_HDR_SIZE { return Err(ExofsError::CorruptedStructure); }
    let magic = u32::from_le_bytes([data[0],data[1],data[2],data[3]]);
    if magic != EPOCH_MAGIC { return Err(ExofsError::InvalidMagic); }
    let count = u32::from_le_bytes([data[12],data[13],data[14],data[15]]) as usize;
    let avail = (data.len().saturating_sub(EPOCH_HDR_SIZE)) / EPOCH_ENTRY_SIZE;
    let n = count.min(avail).min(EPOCH_MAX_ENTRIES);
    let mut out: Vec<EpochJournalEntry> = Vec::new();
    out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n {
        let off = EPOCH_HDR_SIZE.saturating_add(i.saturating_mul(EPOCH_ENTRY_SIZE));
        let mut e = EpochJournalEntry::default();
        let dst = unsafe { core::slice::from_raw_parts_mut(&mut e as *mut EpochJournalEntry as *mut u8, EPOCH_ENTRY_SIZE) };
        let mut j = 0usize;
        while j < EPOCH_ENTRY_SIZE { dst[j] = data[off + j]; j = j.wrapping_add(1); }
        out.push(e);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Sérialise et sauvegarde le journal.
/// OOM-02 / RECUR-01.
fn save_journal(epoch_id: u64, entries: &[EpochJournalEntry], flags: u8) -> ExofsResult<()> {
    let n = entries.len().min(EPOCH_MAX_ENTRIES);
    let total = EPOCH_HDR_SIZE.saturating_add(n.saturating_mul(EPOCH_ENTRY_SIZE));
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let magic = EPOCH_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(magic[i]); i = i.wrapping_add(1); }
    buf.push(EPOCH_VERSION);
    buf.push(flags);
    buf.push(0); buf.push(0); // _pad
    let ep = epoch_id.to_le_bytes();
    let mut i = 0usize;
    while i < 8 { buf.push(ep[i]); i = i.wrapping_add(1); }
    let cnt = (n as u32).to_le_bytes();
    let mut i = 0usize;
    while i < 4 { buf.push(cnt[i]); i = i.wrapping_add(1); }
    let cs = compute_checksum(entries, epoch_id).to_le_bytes();
    let mut i = 0usize;
    while i < 8 { buf.push(cs[i]); i = i.wrapping_add(1); }
    buf.push(0); buf.push(0); buf.push(0); buf.push(0); // _pad2
    let mut i = 0usize;
    while i < n {
        let src = unsafe { core::slice::from_raw_parts(&entries[i] as *const EpochJournalEntry as *const u8, EPOCH_ENTRY_SIZE) };
        let mut j = 0usize;
        while j < EPOCH_ENTRY_SIZE { buf.push(src[j]); j = j.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    let jid = journal_blob_id(epoch_id);
    BLOB_CACHE.insert(jid, &buf).map_err(|_| ExofsError::NoSpace)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique de commit
// ─────────────────────────────────────────────────────────────────────────────

/// Détermine si un blob appartient à une epoch donnée.
fn blob_epoch_id(blob_id: &BlobId) -> u64 {
    match BLOB_CACHE.get(blob_id) {
        Some(d) if d.len() >= 8 => u64::from_le_bytes([d[0],d[1],d[2],d[3],d[4],d[5],d[6],d[7]]),
        _ => 0,
    }
}

/// Collecte tous les blobs appartenant à l'epoch `epoch_id`.
/// OOM-02 / RECUR-01.
fn collect_epoch_blobs(epoch_id: u64) -> ExofsResult<Vec<EpochJournalEntry>> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let mut out: Vec<EpochJournalEntry> = Vec::new();
    out.try_reserve(all.len().min(EPOCH_MAX_ENTRIES)).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < all.len() && out.len() < EPOCH_MAX_ENTRIES {
        let ep = blob_epoch_id(&all[i]);
        if ep == epoch_id {
            let sz = BLOB_CACHE.get(&all[i]).map(|d| d.len() as u64).unwrap_or(0);
            let mut entry = EpochJournalEntry::default();
            let bid = all[i].as_bytes();
            let mut j = 0usize;
            while j < 32 { entry.blob_id[j] = bid[j]; j = j.wrapping_add(1); }
            entry.size = sz;
            out.push(entry);
        }
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Invalide les blobs marqués dirty après commit.
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        BLOB_CACHE.mark_dirty(&bid);
        i = i.wrapping_add(1);
    }
}

/// Exécute le commit d'une epoch.
fn do_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    if args.flags & !epoch_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
    let cur = current_epoch();
    if args.epoch_id != 0 && args.epoch_id != cur {
        if args.flags & epoch_flags::FORCE == 0 { return Err(ExofsError::NoValidEpoch); }
    }
    if COMMIT_STATE.compare_exchange(STATE_IDLE, STATE_IN_PROGRESS, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return Err(ExofsError::CommitInProgress);
    }
    let epoch_to_commit = if args.epoch_id != 0 { args.epoch_id } else { cur };
    let entries = match collect_epoch_blobs(epoch_to_commit) {
        Ok(v)  => v,
        Err(e) => { COMMIT_STATE.store(STATE_IDLE, Ordering::Release); return Err(e); }
    };
    let actual_cs = compute_checksum(&entries, epoch_to_commit);
    if args.flags & epoch_flags::VERIFY_CHECKSUM != 0 && args.checksum != 0 {
        if actual_cs != args.checksum {
            COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
            return Err(ExofsError::ChecksumMismatch);
        }
    }
    if let Err(e) = save_journal(epoch_to_commit, &entries, args.flags as u8) {
        COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
        return Err(e);
    }
    flush_dirty_blobs(&entries);
    let mut bytes_sealed = 0u64;
    let mut i = 0usize;
    while i < entries.len() { bytes_sealed = bytes_sealed.saturating_add(entries[i].size); i = i.wrapping_add(1); }
    let new_epoch = if args.flags & epoch_flags::NO_ADVANCE != 0 { epoch_to_commit } else { epoch_to_commit.wrapping_add(1) };
    if args.flags & epoch_flags::NO_ADVANCE == 0 {
        CURRENT_EPOCH.store(new_epoch, Ordering::Release);
    }
    COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
    Ok(EpochCommitResult { new_epoch, sealed_epoch: epoch_to_commit, blobs_sealed: entries.len() as u64, bytes_sealed, flags: args.flags, _pad: 0 })
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_EPOCH_COMMIT (518)
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_exofs_epoch_commit(
    args_ptr:   u64,
    result_ptr: u64,
    _a3: u64, _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if args_ptr == 0 { return EFAULT; }
    let args = match unsafe { copy_struct_from_user::<EpochCommitArgs>(args_ptr) } {
        Ok(a)  => a,
        Err(_) => return EFAULT,
    };
    let res = match do_commit(&args) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };
    if result_ptr != 0 {
        let bytes = unsafe { core::slice::from_raw_parts(&res as *const EpochCommitResult as *const u8, core::mem::size_of::<EpochCommitResult>()) };
        if let Err(e) = write_user_buf(result_ptr, bytes) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Retourne le journal scellé d'une epoch antérieure (lecture seule).
pub fn read_sealed_journal(epoch_id: u64) -> ExofsResult<Vec<EpochJournalEntry>> {
    load_journal(epoch_id)
}

/// Compte les blobs scellés dans une epoch.
pub fn sealed_blob_count(epoch_id: u64) -> ExofsResult<usize> {
    let entries = load_journal(epoch_id)?;
    Ok(entries.len())
}

/// Retourne les octets totaux scellés dans une epoch.
pub fn sealed_bytes(epoch_id: u64) -> ExofsResult<u64> {
    let entries = load_journal(epoch_id)?;
    let mut total = 0u64;
    let mut i = 0usize;
    while i < entries.len() { total = total.saturating_add(entries[i].size); i = i.wrapping_add(1); }
    Ok(total)
}

/// Force l'avance de l'epoch sans commit complet (unsafe admin).
pub fn force_advance_epoch() -> u64 {
    let cur = CURRENT_EPOCH.fetch_add(1, Ordering::Release);
    cur.wrapping_add(1)
}

/// Réinitialise l'epoch à 1 (utile en test).
pub fn reset_epoch_counter() {
    CURRENT_EPOCH.store(1, Ordering::Release);
    COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
}

/// Retourne vrai si le journal d'une epoch est présent dans le cache.
pub fn epoch_journal_exists(epoch_id: u64) -> bool {
    BLOB_CACHE.get(&journal_blob_id(epoch_id)).is_some()
}

/// Retourne le checksum enregistré dans le journal d'une epoch.
pub fn sealed_checksum(epoch_id: u64) -> ExofsResult<u64> {
    let jid = journal_blob_id(epoch_id);
    let data = BLOB_CACHE.get(&jid).ok_or(ExofsError::BlobNotFound)?;
    if data.len() < EPOCH_HDR_SIZE { return Err(ExofsError::CorruptedStructure); }
    Ok(u64::from_le_bytes([data[16],data[17],data[18],data[19],data[20],data[21],data[22],data[23]]))
}

/// Vérifie la cohérence du journal d'une epoch (magic + checksum recalculé).
pub fn verify_epoch_journal(epoch_id: u64) -> ExofsResult<bool> {
    let jid = journal_blob_id(epoch_id);
    let data = BLOB_CACHE.get(&jid).ok_or(ExofsError::BlobNotFound)?;
    if data.len() < EPOCH_HDR_SIZE { return Err(ExofsError::CorruptedStructure); }
    let magic = u32::from_le_bytes([data[0],data[1],data[2],data[3]]);
    if magic != EPOCH_MAGIC { return Ok(false); }
    let stored_cs = u64::from_le_bytes([data[16],data[17],data[18],data[19],data[20],data[21],data[22],data[23]]);
    let entries = load_journal(epoch_id)?;
    let computed = compute_checksum(&entries, epoch_id);
    Ok(stored_cs == computed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() { reset_epoch_counter(); }

    #[test]
    fn test_epoch_commit_args_size() { assert_eq!(core::mem::size_of::<EpochCommitArgs>(), 32); }

    #[test]
    fn test_epoch_commit_result_size() { assert_eq!(core::mem::size_of::<EpochCommitResult>(), 40); }

    #[test]
    fn test_epoch_journal_header_size() { assert_eq!(core::mem::size_of::<EpochJournalHeader>(), EPOCH_HDR_SIZE); }

    #[test]
    fn test_initial_epoch_is_one() {
        clean();
        assert_eq!(current_epoch(), 1);
    }

    #[test]
    fn test_commit_idle_initially() { assert!(!commit_in_progress()); }

    #[test]
    fn test_commit_advances_epoch() {
        clean();
        let args = EpochCommitArgs { flags: 0, _pad:0, epoch_id: 1, checksum: 0, hints: 0 };
        let res = do_commit(&args).unwrap();
        assert_eq!(res.sealed_epoch, 1);
        assert_eq!(res.new_epoch, 2);
    }

    #[test]
    fn test_commit_no_advance() {
        clean();
        let args = EpochCommitArgs { flags: epoch_flags::NO_ADVANCE, _pad:0, epoch_id: 1, checksum: 0, hints: 0 };
        let res = do_commit(&args).unwrap();
        assert_eq!(res.new_epoch, res.sealed_epoch);
    }

    #[test]
    fn test_commit_wrong_epoch_no_force() {
        clean();
        let args = EpochCommitArgs { flags: 0, _pad:0, epoch_id: 99, checksum: 0, hints: 0 };
        assert!(matches!(do_commit(&args), Err(ExofsError::NoValidEpoch)));
    }

    #[test]
    fn test_commit_wrong_epoch_with_force() {
        clean();
        let args = EpochCommitArgs { flags: epoch_flags::FORCE, _pad:0, epoch_id: 99, checksum: 0, hints: 0 };
        let r = do_commit(&args);
        match r { Ok(_) | Err(ExofsError::GcQueueFull) => {} Err(e) => panic!("unexpected {:?}", e) }
    }

    #[test]
    fn test_commit_invalid_flags() {
        let args = EpochCommitArgs { flags: 0xDEAD, _pad:0, epoch_id: 0, checksum: 0, hints: 0 };
        assert!(matches!(do_commit(&args), Err(ExofsError::InvalidArgument)));
    }

    #[test]
    fn test_sys_null_args() {
        assert_eq!(sys_exofs_epoch_commit(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_force_advance() {
        clean();
        let e = force_advance_epoch();
        assert!(e >= 2);
    }

    #[test]
    fn test_epoch_journal_not_exists_initially() {
        assert!(!epoch_journal_exists(0xFFFF_FFFF));
    }
}
