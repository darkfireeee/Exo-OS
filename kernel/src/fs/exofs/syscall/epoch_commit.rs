//! epoch_commit.rs — SYS_EXOFS_EPOCH_COMMIT (518)
//!
//! Valide une epoch ExoFS : scelle le journal, avance le compteur,
//! invalide les entrées obsolètes du cache.
//! RECUR-01 / OOM-02 / ARITH-02.

use super::validation::{
    copy_kernel_bytes_to_struct, copy_struct_from_user, exofs_err_to_errno, kernel_struct_to_bytes,
    verify_cap, write_user_struct, CapabilityType, EFAULT,
};
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use crate::fs::exofs::core::types::{object_id_from_blob_id, BlobId};
use crate::fs::exofs::core::{DiskOffset, EpochFlags, EpochId, ExofsError, ExofsResult};
use crate::fs::exofs::crypto::key_storage;
use crate::fs::exofs::epoch::epoch_commit as durable_epoch_commit;
use crate::fs::exofs::epoch::epoch_root::{EpochRootEntry, EpochRootInMemory};
use crate::fs::exofs::epoch::epoch_root_chain::{
    rebuild_chain_offsets, serialize_epoch_root_chain, EPOCH_ROOT_PAGE_SIZE,
};
use crate::fs::exofs::storage::{layout, superblock, superblock_backup, virtio_adapter};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const EPOCH_MAGIC: u32 = 0x45_50_4F_43; // "EPOC"
pub const EPOCH_VERSION: u8 = 1;
pub const EPOCH_HDR_SIZE: usize = 40;
pub const EPOCH_MAX_ENTRIES: usize = 1024;
pub const EPOCH_JOURNAL_KEY: &[u8] = b"EPOCH_JOURNAL";

// ─────────────────────────────────────────────────────────────────────────────
// État global de l'epoch courante
// ─────────────────────────────────────────────────────────────────────────────

static CURRENT_EPOCH: AtomicU64 = AtomicU64::new(1);
static COMMIT_STATE: AtomicU32 = AtomicU32::new(0); // 0=idle 1=in_progress
static LAST_COMMITTED_SLOT: AtomicU64 = AtomicU64::new(0);

const STATE_IDLE: u32 = 0;
const STATE_IN_PROGRESS: u32 = 1;

/// Retourne l'epoch courante.
pub fn current_epoch() -> u64 {
    CURRENT_EPOCH.load(Ordering::Acquire)
}

/// Restaure l'epoch courante depuis le recovery ou un superblock validé.
pub fn set_current_epoch(epoch: u64) {
    let normalized = if epoch == 0 { 1 } else { epoch };
    CURRENT_EPOCH.store(normalized, Ordering::Release);
}

/// Retourne vrai si un commit est en cours.
pub fn commit_in_progress() -> bool {
    COMMIT_STATE.load(Ordering::Relaxed) == STATE_IN_PROGRESS
}

// ─────────────────────────────────────────────────────────────────────────────
// Flags
// ─────────────────────────────────────────────────────────────────────────────

pub mod epoch_flags {
    pub const FORCE: u32 = 0x0001;
    pub const VERIFY_CHECKSUM: u32 = 0x0002;
    pub const COMPACT: u32 = 0x0004;
    pub const NO_ADVANCE: u32 = 0x0008;
    pub const VALID_MASK: u32 = FORCE | VERIFY_CHECKSUM | COMPACT | NO_ADVANCE;
}

// ─────────────────────────────────────────────────────────────────────────────
// Structures
// ─────────────────────────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochCommitArgs {
    pub flags: u32,
    pub _pad: u32,
    pub epoch_id: u64,
    pub checksum: u64,
    pub hints: u64,
}

const _: () = assert!(core::mem::size_of::<EpochCommitArgs>() == 32);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochCommitResult {
    pub new_epoch: u64,
    pub sealed_epoch: u64,
    pub blobs_sealed: u64,
    pub bytes_sealed: u64,
    pub flags: u32,
    pub _pad: u32,
}

const _: () = assert!(core::mem::size_of::<EpochCommitResult>() == 40);

/// En-tête du journal d'epoch (40 octets).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochJournalHeader {
    pub magic: u32,
    pub version: u8,
    pub flags: u8,
    pub _pad: u16,
    pub epoch_id: u64,
    pub entry_count: u32,
    pub checksum: u64,
    pub _pad2: u32,
}

const _: () = assert!(core::mem::size_of::<EpochJournalHeader>() == EPOCH_HDR_SIZE);

/// Entrée de journal (40 octets) : blob scellé.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpochJournalEntry {
    pub blob_id: [u8; 32],
    pub size: u64,
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
    while i < 8 {
        buf[i] = ep[i];
        i = i.wrapping_add(1);
    }
    buf[8] = b'E';
    buf[9] = b'P';
    buf[10] = b'O';
    buf[11] = b'C';
    buf[12] = b'H';
    buf[13] = b'_';
    buf[14] = b'J';
    buf[15] = b'O';
    buf[16] = b'U';
    buf[17] = b'R';
    buf[18] = b'N';
    buf[19] = b'A';
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
    let data = match BLOB_CACHE.get(&jid) {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    if data.len() < EPOCH_HDR_SIZE {
        return Err(ExofsError::CorruptedStructure);
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != EPOCH_MAGIC {
        return Err(ExofsError::InvalidMagic);
    }
    let count = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let avail = (data.len().saturating_sub(EPOCH_HDR_SIZE)) / EPOCH_ENTRY_SIZE;
    let n = count.min(avail).min(EPOCH_MAX_ENTRIES);
    let mut out: Vec<EpochJournalEntry> = Vec::new();
    out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n {
        let off = EPOCH_HDR_SIZE.saturating_add(i.saturating_mul(EPOCH_ENTRY_SIZE));
        let mut e = EpochJournalEntry::default();
        copy_kernel_bytes_to_struct(&mut e, &data[off..off + EPOCH_ENTRY_SIZE])?;
        out.push(e);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Sérialise et sauvegarde le journal.
/// OOM-02 / RECUR-01.
fn save_journal(epoch_id: u64, entries: &[EpochJournalEntry], flags: u8) -> ExofsResult<BlobId> {
    let n = entries.len().min(EPOCH_MAX_ENTRIES);
    let total = EPOCH_HDR_SIZE.saturating_add(n.saturating_mul(EPOCH_ENTRY_SIZE));
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let magic = EPOCH_MAGIC.to_le_bytes();
    let mut i = 0usize;
    while i < 4 {
        buf.push(magic[i]);
        i = i.wrapping_add(1);
    }
    buf.push(EPOCH_VERSION);
    buf.push(flags);
    buf.push(0);
    buf.push(0); // _pad
    let ep = epoch_id.to_le_bytes();
    let mut i = 0usize;
    while i < 8 {
        buf.push(ep[i]);
        i = i.wrapping_add(1);
    }
    let cnt = (n as u32).to_le_bytes();
    let mut i = 0usize;
    while i < 4 {
        buf.push(cnt[i]);
        i = i.wrapping_add(1);
    }
    let cs = compute_checksum(entries, epoch_id).to_le_bytes();
    let mut i = 0usize;
    while i < 8 {
        buf.push(cs[i]);
        i = i.wrapping_add(1);
    }
    buf.push(0);
    buf.push(0);
    buf.push(0);
    buf.push(0); // _pad2
    let mut i = 0usize;
    while i < n {
        let src = kernel_struct_to_bytes::<_, EPOCH_ENTRY_SIZE>(&entries[i]);
        let mut j = 0usize;
        while j < EPOCH_ENTRY_SIZE {
            buf.push(src[j]);
            j = j.wrapping_add(1);
        }
        i = i.wrapping_add(1);
    }
    let jid = journal_blob_id(epoch_id);
    BLOB_CACHE
        .insert(jid, buf.to_vec())
        .map_err(|_| ExofsError::NoSpace)?;
    if crate::fs::exofs::storage::virtio_adapter::has_global_disk() {
        if super::object_store::persist_blob_data_if_disk(jid, &buf, true)? {
            let _ = BLOB_CACHE.mark_clean(&jid);
        }
    }
    Ok(jid)
}

// ─────────────────────────────────────────────────────────────────────────────
// Logique de commit
// ─────────────────────────────────────────────────────────────────────────────

/// Collecte tous les blobs appartenant à l'epoch `epoch_id`.
/// OOM-02 / RECUR-01.
fn collect_epoch_blobs(epoch_id: u64) -> ExofsResult<Vec<EpochJournalEntry>> {
    let all = BLOB_CACHE.dirty_ids();
    let mut out: Vec<EpochJournalEntry> = Vec::new();
    out.try_reserve(all.len().min(EPOCH_MAX_ENTRIES))
        .map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < all.len() && out.len() < EPOCH_MAX_ENTRIES {
        let fd_epoch = super::object_fd::OBJECT_TABLE
            .entry_for_blob(&all[i])
            .map(|entry| entry.epoch_id)
            .unwrap_or(epoch_id);
        if fd_epoch == 0 || fd_epoch == epoch_id {
            let sz = BLOB_CACHE.get(&all[i]).map(|d| d.len() as u64).unwrap_or(0);
            let mut entry = EpochJournalEntry::default();
            let bid = all[i].as_bytes();
            let mut j = 0usize;
            while j < 32 {
                entry.blob_id[j] = bid[j];
                j = j.wrapping_add(1);
            }
            entry.size = sz;
            out.push(entry);
        }
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Invalide les blobs marqués dirty après commit.
fn flush_dirty_blobs(entries: &[EpochJournalEntry]) -> ExofsResult<()> {
    let mut i = 0usize;
    while i < entries.len() {
        let bid = BlobId(entries[i].blob_id);
        if let Some(data) = BLOB_CACHE.get(&bid) {
            if super::object_store::persist_blob_data_if_disk(bid, data.as_ref(), true)? {
                let _ = BLOB_CACHE.mark_clean(&bid);
            }
        }
        i = i.wrapping_add(1);
    }
    Ok(())
}

fn flatten_root_pages(pages: &[Vec<u8>]) -> ExofsResult<Vec<u8>> {
    let total = pages.len().saturating_mul(EPOCH_ROOT_PAGE_SIZE);
    let mut out = Vec::new();
    out.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < pages.len() {
        out.extend_from_slice(&pages[i]);
        i = i.wrapping_add(1);
    }
    Ok(out)
}

fn epoch_root_blob_id(epoch_id: u64) -> BlobId {
    let mut buf = [0u8; 18];
    let ep = epoch_id.to_le_bytes();
    let mut i = 0usize;
    while i < 8 {
        buf[i] = ep[i];
        i = i.wrapping_add(1);
    }
    buf[8..18].copy_from_slice(b"EPOCH_ROOT");
    BlobId::from_bytes_blake3(&buf)
}

fn build_epoch_root(
    epoch_id: u64,
    entries: &[EpochJournalEntry],
) -> ExofsResult<EpochRootInMemory> {
    let mut root = EpochRootInMemory::new(EpochId(epoch_id));
    let mut i = 0usize;
    while i < entries.len() {
        let blob_id = BlobId(entries[i].blob_id);
        let object_id = object_id_from_blob_id(&blob_id);
        let disk_offset =
            super::object_store::mapping_disk_offset(&blob_id).unwrap_or(DiskOffset::ZERO);
        root.add_modified(object_id, disk_offset, EpochRootEntry::FLAG_MODIFIED)?;
        i = i.wrapping_add(1);
    }
    Ok(root)
}

fn save_epoch_root_blob(epoch_id: u64, root: &EpochRootInMemory) -> ExofsResult<BlobId> {
    let root_blob = epoch_root_blob_id(epoch_id);
    let mut pages = serialize_epoch_root_chain(root)?;
    let first_flat = flatten_root_pages(&pages)?;
    BLOB_CACHE
        .insert(root_blob, first_flat.to_vec())
        .map_err(|_| ExofsError::NoSpace)?;
    let _ = super::object_store::persist_blob_data_if_disk(root_blob, &first_flat, true)?;

    if let Some(base) = super::object_store::mapping_disk_offset(&root_blob) {
        let mut offsets = Vec::new();
        offsets
            .try_reserve(pages.len())
            .map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < pages.len() {
            offsets.push(DiskOffset(
                base.0
                    .checked_add((i as u64).saturating_mul(EPOCH_ROOT_PAGE_SIZE as u64))
                    .ok_or(ExofsError::OffsetOverflow)?,
            ));
            i = i.wrapping_add(1);
        }
        rebuild_chain_offsets(&mut pages, &offsets)?;
        let flat = flatten_root_pages(&pages)?;
        BLOB_CACHE
            .insert(root_blob, flat.to_vec())
            .map_err(|_| ExofsError::NoSpace)?;
        let _ = super::object_store::persist_blob_data_if_disk(root_blob, &flat, true)?;
        let _ = BLOB_CACHE.mark_clean(&root_blob);
    }
    Ok(root_blob)
}

fn next_epoch_slot() -> ExofsResult<(DiskOffset, DiskOffset)> {
    let prev = DiskOffset(LAST_COMMITTED_SLOT.load(Ordering::Acquire));
    let disk_size = virtio_adapter::default_global_disk_size_bytes();
    let slot_a = layout::epoch_slot_a();
    let slot_b = layout::epoch_slot_b();
    let slot_c = layout::epoch_slot_c(disk_size).unwrap_or(slot_a);
    let next = if prev == slot_a {
        slot_b
    } else if prev == slot_b {
        slot_c
    } else {
        slot_a
    };
    Ok((prev, next))
}

fn update_superblock_epoch_if_present(next_epoch: u64) -> ExofsResult<()> {
    if !virtio_adapter::has_global_disk() {
        return Ok(());
    }
    let disk_size = virtio_adapter::default_global_disk_size_bytes();
    if disk_size < superblock::MIN_DISK_SIZE {
        return Ok(());
    }
    let read_fn = |offset: DiskOffset, buf: &mut [u8]| -> ExofsResult<usize> {
        let data = virtio_adapter::read_at_offset(offset, buf.len())?;
        buf.copy_from_slice(&data);
        Ok(data.len())
    };
    let recovered = match superblock_backup::recover_superblock(disk_size, &read_fn) {
        Ok(result) => result,
        Err(ExofsError::BadMagic)
        | Err(ExofsError::InvalidMagic)
        | Err(ExofsError::CorruptFilesystem)
        | Err(ExofsError::InvalidState)
        | Err(ExofsError::IoError) => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut sb = recovered.superblock;
    if sb.epoch_current >= next_epoch {
        return Ok(());
    }
    sb.epoch_current = next_epoch;
    sb.last_commit_time = crate::arch::time::read_ticks();
    sb.finalize();
    let write_fn = |data: &[u8], offset: DiskOffset| -> ExofsResult<usize> {
        virtio_adapter::write_at_offset(offset, data)
    };
    let _ = superblock_backup::write_superblock_mirrors(&sb, disk_size, &write_fn)?;
    virtio_adapter::flush_global_disk()
}

fn commit_durable_epoch_if_disk(
    epoch_to_commit: u64,
    entries: &[EpochJournalEntry],
    flags: u32,
) -> ExofsResult<Option<u64>> {
    if !virtio_adapter::has_global_disk() || flags & epoch_flags::NO_ADVANCE != 0 {
        return Ok(None);
    }
    // FIX-EXOFS-ROB-1 (AUDIT-EXOFS §3) : EPOCH-02 — un commit durable sans hook
    // de flush NVMe enregistré exécuterait ses 3 barrières en no-op (fausse
    // durabilité, corruption certaine au prochain crash). Un disque est présent
    // (has_global_disk) mais le block layer n'a pas enregistré son flush : on
    // refuse le commit au lieu de prétendre l'avoir durabilisé. Le chemin
    // dev-sans-disque est déjà court-circuité ci-dessus (has_global_disk()==false).
    if !crate::fs::exofs::epoch::epoch_barriers::is_nvme_flush_registered() {
        return Err(ExofsError::NvmeFlushFailed);
    }
    let root = build_epoch_root(epoch_to_commit, entries)?;
    let root_blob = save_epoch_root_blob(epoch_to_commit, &root)?;
    let root_disk_offset =
        super::object_store::mapping_disk_offset(&root_blob).unwrap_or(DiskOffset::ZERO);
    let (prev_slot, slot_offset) = next_epoch_slot()?;
    let get_current_epoch = || EpochId(epoch_to_commit);
    let advance_epoch = |next: EpochId| -> ExofsResult<()> {
        set_current_epoch(next.0);
        update_superblock_epoch_if_present(next.0)
    };
    let get_tsc = || crate::arch::time::read_ticks();
    let write_fn = |data: &[u8], offset: DiskOffset| -> ExofsResult<usize> {
        virtio_adapter::write_at_offset(offset, data)
    };
    let result = durable_epoch_commit::commit_epoch(durable_epoch_commit::CommitInput {
        root: &root,
        callbacks: durable_epoch_commit::CommitCallbacks {
            get_current_epoch: &get_current_epoch,
            advance_epoch: &advance_epoch,
            get_tsc: &get_tsc,
            write_fn: &write_fn,
        },
        root_disk_offset,
        slot_offset,
        prev_slot_offset: prev_slot,
        extra_flags: EpochFlags::default(),
    })?;
    LAST_COMMITTED_SLOT.store(result.slot_offset.0, Ordering::Release);
    Ok(Some(result.epoch_id.0))
}

/// Exécute le commit d'une epoch.
fn do_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    if args.flags & !epoch_flags::VALID_MASK != 0 {
        return Err(ExofsError::InvalidArgument);
    }
    let cur = current_epoch();
    if args.epoch_id != 0 && args.epoch_id != cur {
        if args.flags & epoch_flags::FORCE == 0 {
            return Err(ExofsError::NoValidEpoch);
        }
    }
    if COMMIT_STATE
        .compare_exchange(
            STATE_IDLE,
            STATE_IN_PROGRESS,
            Ordering::Acquire,
            Ordering::Relaxed,
        )
        .is_err()
    {
        return Err(ExofsError::CommitInProgress);
    }
    let epoch_to_commit = if args.epoch_id != 0 {
        args.epoch_id
    } else {
        cur
    };
    let entries = match collect_epoch_blobs(epoch_to_commit) {
        Ok(v) => v,
        Err(e) => {
            COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
            return Err(e);
        }
    };
    let actual_cs = compute_checksum(&entries, epoch_to_commit);
    if args.flags & epoch_flags::VERIFY_CHECKSUM != 0 && args.checksum != 0 {
        if actual_cs != args.checksum {
            COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
            return Err(ExofsError::ChecksumMismatch);
        }
    }
    if let Err(e) = flush_dirty_blobs(&entries) {
        COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
        return Err(e);
    }
    if let Err(e) = key_storage::persist_global_if_master_present() {
        COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
        return Err(e);
    }
    if let Err(e) = save_journal(epoch_to_commit, &entries, args.flags as u8) {
        COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
        return Err(e);
    }
    let durable_new_epoch =
        match commit_durable_epoch_if_disk(epoch_to_commit, &entries, args.flags) {
            Ok(epoch) => epoch,
            Err(e) => {
                COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
                return Err(e);
            }
        };
    let mut bytes_sealed = 0u64;
    let mut i = 0usize;
    while i < entries.len() {
        bytes_sealed = bytes_sealed.saturating_add(entries[i].size);
        i = i.wrapping_add(1);
    }
    let new_epoch = if let Some(epoch) = durable_new_epoch {
        epoch
    } else if args.flags & epoch_flags::NO_ADVANCE != 0 {
        epoch_to_commit
    } else {
        epoch_to_commit.wrapping_add(1)
    };
    if args.flags & epoch_flags::NO_ADVANCE == 0 && durable_new_epoch.is_none() {
        set_current_epoch(new_epoch);
    }
    COMMIT_STATE.store(STATE_IDLE, Ordering::Release);
    Ok(EpochCommitResult {
        new_epoch,
        sealed_epoch: epoch_to_commit,
        blobs_sealed: entries.len() as u64,
        bytes_sealed,
        flags: args.flags,
        _pad: 0,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// API interne — utilisée par exofs_shutdown() (pas de validation userspace)
// ─────────────────────────────────────────────────────────────────────────────

/// Commit synchrone de l'epoch courante venant du kernel (shutdown path).
///
/// Évite le copy_from_user / write_user_buf — appelé directement depuis
/// `exofs_shutdown()` sans frame syscall.
pub fn do_shutdown_commit(args: &EpochCommitArgs) -> ExofsResult<EpochCommitResult> {
    do_commit(args)
}

/// Commit kernel-internal de l'epoch courante (chemin chaud writeback/fsync/sync).
///
/// FIX-EXOFS-CORE-1 (AUDIT-EXOFS §2) : `commit_epoch` n'était déclenché qu'au
/// démontage ⇒ les écritures atteignaient le disque en blobs bruts isolés, sans
/// EpochRecord pour les valider/annuler (pas d'atomicité, recovery sans objet).
/// Ce point d'entrée déclenche le commit transactionnel complet (flush des blobs
/// dirty → journal → EpochRoot → EpochRecord + 3 barrières NVMe) via le même
/// `do_commit` que le shutdown, sur le chemin chaud.
///
/// `CommitInProgress` est renvoyé si un commit concourant est déjà en cours :
/// l'appelant (writeback périodique / sync) peut l'ignorer, le travail sera fait.
pub fn commit_current_epoch() -> ExofsResult<EpochCommitResult> {
    let args = EpochCommitArgs {
        flags: 0, // commit normal de l'epoch courante (pas de FORCE)
        _pad: 0,
        epoch_id: 0, // 0 = epoch courante
        checksum: 0,
        hints: 0,
    };
    do_commit(&args)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_EPOCH_COMMIT (518)
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_exofs_epoch_commit(
    args_ptr: u64,
    result_ptr: u64,
    _a3: u64,
    _a4: u64,
    _a5: u64,
    cap_rights: u64,
) -> i64 {
    if args_ptr == 0 {
        return EFAULT;
    }
    // SAFETY: invariant de sécurité vérifié par les préconditions de la fonction appelante.
    let args = match unsafe { copy_struct_from_user::<EpochCommitArgs>(args_ptr) } {
        Ok(a) => a,
        Err(_) => return EFAULT,
    };
    // Phase 2 (TOCTOU-safe): verify_cap après copie immuable des args.
    if let Err(e) = verify_cap(cap_rights, CapabilityType::ExoFsEpochCommit) {
        return e;
    }
    let res = match do_commit(&args) {
        Ok(r) => r,
        Err(e) => return exofs_err_to_errno(e),
    };
    if result_ptr != 0 {
        if let Err(e) = write_user_struct(result_ptr, &res) {
            return e;
        }
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
    while i < entries.len() {
        total = total.saturating_add(entries[i].size);
        i = i.wrapping_add(1);
    }
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
    LAST_COMMITTED_SLOT.store(0, Ordering::Release);
}

/// Retourne vrai si le journal d'une epoch est présent dans le cache.
pub fn epoch_journal_exists(epoch_id: u64) -> bool {
    BLOB_CACHE.get(&journal_blob_id(epoch_id)).is_some()
}

/// Retourne le checksum enregistré dans le journal d'une epoch.
pub fn sealed_checksum(epoch_id: u64) -> ExofsResult<u64> {
    let jid = journal_blob_id(epoch_id);
    let data = BLOB_CACHE.get(&jid).ok_or(ExofsError::BlobNotFound)?;
    if data.len() < EPOCH_HDR_SIZE {
        return Err(ExofsError::CorruptedStructure);
    }
    Ok(u64::from_le_bytes([
        data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
    ]))
}

/// Vérifie la cohérence du journal d'une epoch (magic + checksum recalculé).
pub fn verify_epoch_journal(epoch_id: u64) -> ExofsResult<bool> {
    let jid = journal_blob_id(epoch_id);
    let data = BLOB_CACHE.get(&jid).ok_or(ExofsError::BlobNotFound)?;
    if data.len() < EPOCH_HDR_SIZE {
        return Err(ExofsError::CorruptedStructure);
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != EPOCH_MAGIC {
        return Ok(false);
    }
    let stored_cs = u64::from_le_bytes([
        data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
    ]);
    let entries = load_journal(epoch_id)?;
    let computed = compute_checksum(&entries, epoch_id);
    Ok(stored_cs == computed)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    fn clean() {
        reset_epoch_counter();
    }

    #[test]
    fn test_epoch_commit_args_size() {
        assert_eq!(core::mem::size_of::<EpochCommitArgs>(), 32);
    }

    #[test]
    fn test_epoch_commit_result_size() {
        assert_eq!(core::mem::size_of::<EpochCommitResult>(), 40);
    }

    #[test]
    fn test_epoch_journal_header_size() {
        assert_eq!(core::mem::size_of::<EpochJournalHeader>(), EPOCH_HDR_SIZE);
    }

    #[test]
    fn test_initial_epoch_is_one() {
        clean();
        assert_eq!(current_epoch(), 1);
    }

    #[test]
    fn test_commit_idle_initially() {
        assert!(!commit_in_progress());
    }

    #[test]
    fn test_commit_advances_epoch() {
        clean();
        let args = EpochCommitArgs {
            flags: 0,
            _pad: 0,
            epoch_id: 1,
            checksum: 0,
            hints: 0,
        };
        let res = do_commit(&args).test_unwrap();
        assert_eq!(res.sealed_epoch, 1);
        assert_eq!(res.new_epoch, 2);
    }

    #[test]
    fn test_commit_no_advance() {
        clean();
        let args = EpochCommitArgs {
            flags: epoch_flags::NO_ADVANCE,
            _pad: 0,
            epoch_id: 1,
            checksum: 0,
            hints: 0,
        };
        let res = do_commit(&args).test_unwrap();
        assert_eq!(res.new_epoch, res.sealed_epoch);
    }

    #[test]
    fn test_commit_wrong_epoch_no_force() {
        clean();
        let args = EpochCommitArgs {
            flags: 0,
            _pad: 0,
            epoch_id: 99,
            checksum: 0,
            hints: 0,
        };
        assert!(matches!(do_commit(&args), Err(ExofsError::NoValidEpoch)));
    }

    #[test]
    fn test_commit_wrong_epoch_with_force() {
        clean();
        let args = EpochCommitArgs {
            flags: epoch_flags::FORCE,
            _pad: 0,
            epoch_id: 99,
            checksum: 0,
            hints: 0,
        };
        let r = do_commit(&args);
        match r {
            Ok(_) | Err(ExofsError::GcQueueFull) => {}
            Err(e) => panic!("unexpected {:?}", e),
        }
    }

    #[test]
    fn test_commit_invalid_flags() {
        let args = EpochCommitArgs {
            flags: 0xDEAD,
            _pad: 0,
            epoch_id: 0,
            checksum: 0,
            hints: 0,
        };
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
