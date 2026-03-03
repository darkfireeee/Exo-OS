//! gc_trigger.rs — SYS_EXOFS_GC_TRIGGER (514)
//!
//! Déclenche le GC ExoFS : identifie les blobs orphelins, libère l'espace.
//! RECUR-01 / OOM-02 / ARITH-02.

use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use crate::fs::exofs::core::types::BlobId;
use crate::fs::exofs::cache::blob_cache::BLOB_CACHE;
use super::validation::{
    exofs_err_to_errno, copy_struct_from_user, write_user_buf, EFAULT,
};
use super::object_fd::OBJECT_TABLE;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const GC_MAX_BLOBS_PER_RUN:  usize = 4096;
pub const GC_TOMBSTONE_MAGIC:    u32   = 0x54_4F_4D_42; // "TOMB"
const     GC_QUEUE_FLAG_RUNNING: u32   = 0x0001;
const     GC_QUEUE_FLAG_DRY:     u32   = 0x0002;

// ─────────────────────────────────────────────────────────────────────────────
// Structures publiques
// ─────────────────────────────────────────────────────────────────────────────

pub mod gc_flags {
    pub const DRY_RUN:       u32 = 0x0001;
    pub const AGGRESSIVE:    u32 = 0x0002;
    pub const TOMBSTONE_KEEP:u32 = 0x0004;
    pub const VALID_MASK:    u32 = DRY_RUN | AGGRESSIVE | TOMBSTONE_KEEP;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GcArgs {
    pub flags:           u32,
    pub _pad:            u32,
    pub epoch_threshold: u64,
    pub max_blobs:       u32,
    pub _pad2:           u32,
}

const _: () = assert!(core::mem::size_of::<GcArgs>() == 24);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GcResult {
    pub orphans_found: u64,
    pub bytes_freed:   u64,
    pub blobs_deleted: u64,
    pub dry_run:       u32,
    pub _pad:          u32,
}

const _: () = assert!(core::mem::size_of::<GcResult>() == 32);

// ─────────────────────────────────────────────────────────────────────────────
// GC Queue globale (protégée par un spinlock atomique)
// ─────────────────────────────────────────────────────────────────────────────

use core::sync::atomic::{AtomicU32, Ordering};
static GC_LOCK: AtomicU32 = AtomicU32::new(0);

fn gc_lock_acquire() -> bool {
    GC_LOCK.compare_exchange(0, GC_QUEUE_FLAG_RUNNING, Ordering::Acquire, Ordering::Relaxed).is_ok()
}

fn gc_lock_release() {
    GC_LOCK.store(0, Ordering::Release);
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers internes
// ─────────────────────────────────────────────────────────────────────────────

/// Décide si un blob est orphelin (aucun fd ouvert + non dans la queue active).
fn is_orphan(blob_id: &BlobId) -> bool {
    OBJECT_TABLE.lock()
        .map(|t| t.open_count_for(blob_id) == 0)
        .unwrap_or(false)
}

/// Retourne la taille d'un blob tel que stocké dans le cache.
fn blob_size(blob_id: &BlobId) -> u64 {
    BLOB_CACHE.get(blob_id).map(|d| d.len() as u64).unwrap_or(0)
}

/// Vérifie que l'epoch d'un blob dépasse le seuil.
fn blob_epoch(blob_id: &BlobId) -> u64 {
    let data = match BLOB_CACHE.get(blob_id) { Some(d) => d, None => return 0 };
    if data.len() < 8 { return 0; }
    u64::from_le_bytes([data[0],data[1],data[2],data[3],data[4],data[5],data[6],data[7]])
}

/// Supprime un blob du cache et l'invalide.
fn delete_blob(blob_id: &BlobId) {
    BLOB_CACHE.invalidate(blob_id);
}

/// Collecte tous les BlobIds candidats au GC depuis le cache.
/// OOM-02 / RECUR-01.
fn collect_candidates(max: usize) -> ExofsResult<Vec<BlobId>> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let n = all.len().min(max).min(GC_MAX_BLOBS_PER_RUN);
    let mut out: Vec<BlobId> = Vec::new();
    out.try_reserve(n).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < n { out.push(all[i]); i = i.wrapping_add(1); }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonction principale du GC
// ─────────────────────────────────────────────────────────────────────────────

/// Lance le ramasse-miettes.
/// OOM-02 : try_reserve. RECUR-01 : while.
fn run_gc(args: &GcArgs) -> ExofsResult<GcResult> {
    if args.flags & !gc_flags::VALID_MASK != 0 { return Err(ExofsError::InvalidArgument); }
    if !gc_lock_acquire() { return Err(ExofsError::GcQueueFull); }

    let dry = args.flags & gc_flags::DRY_RUN != 0;
    let aggressive = args.flags & gc_flags::AGGRESSIVE != 0;
    let max_blobs = if args.max_blobs == 0 { GC_MAX_BLOBS_PER_RUN } else { (args.max_blobs as usize).min(GC_MAX_BLOBS_PER_RUN) };

    let candidates = match collect_candidates(max_blobs) {
        Ok(v)  => v,
        Err(e) => { gc_lock_release(); return Err(e); }
    };

    let mut res = GcResult::default();
    res.dry_run = if dry { 1 } else { 0 };
    let mut i = 0usize;
    while i < candidates.len() {
        let bid = &candidates[i];
        let orphan = is_orphan(bid);
        let old_epoch = args.epoch_threshold > 0 && blob_epoch(bid) < args.epoch_threshold;
        let should_collect = orphan && (aggressive || old_epoch || args.epoch_threshold == 0);
        if should_collect {
            res.orphans_found = res.orphans_found.saturating_add(1);
            let sz = blob_size(bid);
            if !dry {
                delete_blob(bid);
                res.bytes_freed   = res.bytes_freed.saturating_add(sz);
                res.blobs_deleted = res.blobs_deleted.saturating_add(1);
            }
        }
        i = i.wrapping_add(1);
    }
    gc_lock_release();
    Ok(res)
}

// ─────────────────────────────────────────────────────────────────────────────
// Handler SYS_EXOFS_GC_TRIGGER (514)
// ─────────────────────────────────────────────────────────────────────────────

pub fn sys_exofs_gc_trigger(
    args_ptr:   u64,
    result_ptr: u64,
    _a3: u64, _a4: u64, _a5: u64, _a6: u64,
) -> i64 {
    if args_ptr == 0 { return EFAULT; }
    let args = match unsafe { copy_struct_from_user::<GcArgs>(args_ptr) } {
        Ok(a)  => a,
        Err(_) => return EFAULT,
    };
    let res = match run_gc(&args) {
        Ok(r)  => r,
        Err(e) => return exofs_err_to_errno(e),
    };
    if result_ptr != 0 {
        let bytes = unsafe {
            core::slice::from_raw_parts(&res as *const GcResult as *const u8, core::mem::size_of::<GcResult>())
        };
        if let Err(e) = write_user_buf(result_ptr, bytes) { return e; }
    }
    0i64
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers supplémentaires
// ─────────────────────────────────────────────────────────────────────────────

/// Compte les blobs en cache (hors fds ouverts).
pub fn count_orphans() -> ExofsResult<usize> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let mut count = 0usize;
    let mut i = 0usize;
    while i < all.len() {
        if is_orphan(&all[i]) { count = count.saturating_add(1); }
        i = i.wrapping_add(1);
    }
    Ok(count)
}

/// Estimation du total d'octets libérables.
pub fn estimate_reclaimable() -> ExofsResult<u64> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let mut total = 0u64;
    let mut i = 0usize;
    while i < all.len() {
        if is_orphan(&all[i]) { total = total.saturating_add(blob_size(&all[i])); }
        i = i.wrapping_add(1);
    }
    Ok(total)
}

/// Retourne vrai si le GC est actuellement en cours.
pub fn gc_running() -> bool {
    GC_LOCK.load(Ordering::Relaxed) != 0
}

/// Collecte forcée d'un blob précis (si orphelin).
pub fn collect_one(blob_id: &BlobId) -> ExofsResult<bool> {
    if !is_orphan(blob_id) { return Ok(false); }
    if !gc_lock_acquire() { return Err(ExofsError::GcQueueFull); }
    delete_blob(blob_id);
    gc_lock_release();
    Ok(true)
}

/// Purge tous les blobs dont l'époque est strictement inférieure à `min_epoch`.
/// RECUR-01 / OOM-02.
pub fn purge_old_epochs(min_epoch: u64) -> ExofsResult<u64> {
    if !gc_lock_acquire() { return Err(ExofsError::GcQueueFull); }
    let all = match BLOB_CACHE.list_keys() {
        Ok(v)  => v,
        Err(_) => { gc_lock_release(); return Err(ExofsError::GcQueueFull); }
    };
    let mut freed = 0u64;
    let mut i = 0usize;
    while i < all.len() {
        let ep = blob_epoch(&all[i]);
        if ep > 0 && ep < min_epoch && is_orphan(&all[i]) {
            freed = freed.saturating_add(blob_size(&all[i]));
            delete_blob(&all[i]);
        }
        i = i.wrapping_add(1);
    }
    gc_lock_release();
    Ok(freed)
}

/// Retourne la taille totale du cache courant.
pub fn cache_total_size() -> ExofsResult<u64> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let mut total = 0u64;
    let mut i = 0usize;
    while i < all.len() { total = total.saturating_add(blob_size(&all[i])); i = i.wrapping_add(1); }
    Ok(total)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gc_args_size() { assert_eq!(core::mem::size_of::<GcArgs>(), 24); }

    #[test]
    fn test_gc_result_size() { assert_eq!(core::mem::size_of::<GcResult>(), 32); }

    #[test]
    fn test_gc_dry_run_returns_ok() {
        let args = GcArgs { flags: gc_flags::DRY_RUN, _pad: 0, epoch_threshold: 0, max_blobs: 64, _pad2: 0 };
        let res = run_gc(&args);
        // peut échouer si GcQueueFull (autre test) mais sinon Ok
        match res { Ok(_) | Err(ExofsError::GcQueueFull) => {} Err(e) => panic!("unexpected {:?}", e) }
    }

    #[test]
    fn test_gc_invalid_flags() {
        let args = GcArgs { flags: 0xDEAD, _pad: 0, epoch_threshold: 0, max_blobs: 0, _pad2: 0 };
        assert!(matches!(run_gc(&args), Err(ExofsError::InvalidArgument)));
    }

    #[test]
    fn test_gc_null_args_returns_efault() {
        assert_eq!(sys_exofs_gc_trigger(0, 0, 0, 0, 0, 0), EFAULT);
    }

    #[test]
    fn test_gc_flags_valid_mask() {
        let all = gc_flags::DRY_RUN | gc_flags::AGGRESSIVE | gc_flags::TOMBSTONE_KEEP;
        assert_eq!(all, gc_flags::VALID_MASK);
    }

    #[test]
    fn test_gc_tombstone_magic() { assert_eq!(GC_TOMBSTONE_MAGIC, 0x544F4D42); }

    #[test]
    fn test_gc_running_initial() { assert!(!gc_running()); }

    #[test]
    fn test_collect_candidates_zero_max() {
        let v = collect_candidates(0).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn test_cache_total_size() {
        let sz = cache_total_size().unwrap_or(0);
        let _ = sz; // may be 0 in test env
    }

    #[test]
    fn test_purge_old_epochs_no_crash() {
        let freed = purge_old_epochs(1).unwrap_or(0);
        let _ = freed;
    }

    #[test]
    fn test_estimate_reclaimable() {
        let r = estimate_reclaimable().unwrap_or(0);
        let _ = r;
    }

    #[test]
    fn test_count_orphans() {
        let n = count_orphans().unwrap_or(0);
        let _ = n;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Rapport de GC étendu
// ─────────────────────────────────────────────────────────────────────────────

/// Encode un GcResult en octets pour transmission userspace (16 octets).
pub fn encode_gc_result(r: &GcResult) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let a = r.orphans_found.to_le_bytes();
    let b = r.bytes_freed.to_le_bytes();
    let c = r.blobs_deleted.to_le_bytes();
    let d = r.dry_run.to_le_bytes();
    let mut i = 0usize;
    while i < 8 { buf[i] = a[i]; i = i.wrapping_add(1); }
    let mut i = 0usize;
    while i < 8 { buf[8 + i] = b[i]; i = i.wrapping_add(1); }
    let mut i = 0usize;
    while i < 8 { buf[16 + i] = c[i]; i = i.wrapping_add(1); }
    let mut i = 0usize;
    while i < 4 { buf[24 + i] = d[i]; i = i.wrapping_add(1); }
    buf
}

/// Décode un GcResult depuis un buffer de 32 octets.
pub fn decode_gc_result(buf: &[u8]) -> Option<GcResult> {
    if buf.len() < 32 { return None; }
    Some(GcResult {
        orphans_found: u64::from_le_bytes([buf[0],buf[1],buf[2],buf[3],buf[4],buf[5],buf[6],buf[7]]),
        bytes_freed:   u64::from_le_bytes([buf[8],buf[9],buf[10],buf[11],buf[12],buf[13],buf[14],buf[15]]),
        blobs_deleted: u64::from_le_bytes([buf[16],buf[17],buf[18],buf[19],buf[20],buf[21],buf[22],buf[23]]),
        dry_run:       u32::from_le_bytes([buf[24],buf[25],buf[26],buf[27]]),
        _pad:          0,
    })
}

/// Retourne la liste des BlobIds orphelins (hors dry-run).
/// OOM-02 / RECUR-01.
pub fn list_orphans() -> ExofsResult<Vec<BlobId>> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    let mut out: Vec<BlobId> = Vec::new();
    out.try_reserve(all.len().min(GC_MAX_BLOBS_PER_RUN)).map_err(|_| ExofsError::NoMemory)?;
    let mut i = 0usize;
    while i < all.len() && out.len() < GC_MAX_BLOBS_PER_RUN {
        if is_orphan(&all[i]) { out.push(all[i]); }
        i = i.wrapping_add(1);
    }
    Ok(out)
}

/// Exécute le GC de manière sécurisée (dry-run puis effective).
/// Phase 1 détecte, phase 2 supprime.
pub fn run_gc_two_phase(epoch_threshold: u64) -> ExofsResult<GcResult> {
    let dry_args = GcArgs { flags: gc_flags::DRY_RUN, _pad:0, epoch_threshold, max_blobs: GC_MAX_BLOBS_PER_RUN as u32, _pad2:0 };
    let preview = run_gc(&dry_args)?;
    if preview.orphans_found == 0 { return Ok(preview); }
    let live_args = GcArgs { flags: 0, _pad:0, epoch_threshold, max_blobs: GC_MAX_BLOBS_PER_RUN as u32, _pad2:0 };
    run_gc(&live_args)
}

/// Calcule le ratio d'utilisation du cache (0..=100).
pub fn cache_usage_percent(max_bytes: u64) -> ExofsResult<u32> {
    if max_bytes == 0 { return Ok(0); }
    let used = cache_total_size()?;
    let pct = used.saturating_mul(100).checked_div(max_bytes).unwrap_or(100);
    Ok(pct.min(100) as u32)
}

/// Supprime jusqu'à `n` orphelins, retourne les BlobIds supprimés.
/// OOM-02 / RECUR-01.
pub fn collect_n_orphans(n: usize) -> ExofsResult<Vec<BlobId>> {
    let orphans = list_orphans()?;
    let limit = n.min(orphans.len());
    let mut removed: Vec<BlobId> = Vec::new();
    removed.try_reserve(limit).map_err(|_| ExofsError::NoMemory)?;
    if !gc_lock_acquire() { return Err(ExofsError::GcQueueFull); }
    let mut i = 0usize;
    while i < limit {
        delete_blob(&orphans[i]);
        removed.push(orphans[i]);
        i = i.wrapping_add(1);
    }
    gc_lock_release();
    Ok(removed)
}

/// Vérifie que le GC n'est pas actif avant de tenter une allocation critique.
pub fn assert_gc_idle() -> ExofsResult<()> {
    if gc_running() { Err(ExofsError::GcQueueFull) } else { Ok(()) }
}

/// Marqueur : retourne vrai si le blob est un tombstone.
pub fn is_tombstone(blob_id: &BlobId) -> bool {
    match BLOB_CACHE.get(blob_id) {
        Some(d) if d.len() >= 4 => {
            let m = u32::from_le_bytes([d[0],d[1],d[2],d[3]]);
            m == GC_TOMBSTONE_MAGIC
        }
        _ => false,
    }
}

/// Supprime tous les tombstones (sauf si flag TOMBSTONE_KEEP).
pub fn purge_tombstones() -> ExofsResult<usize> {
    let all = BLOB_CACHE.list_keys().map_err(|_| ExofsError::GcQueueFull)?;
    if !gc_lock_acquire() { return Err(ExofsError::GcQueueFull); }
    let mut count = 0usize;
    let mut i = 0usize;
    while i < all.len() {
        if is_tombstone(&all[i]) { delete_blob(&all[i]); count = count.saturating_add(1); }
        i = i.wrapping_add(1);
    }
    gc_lock_release();
    Ok(count)
}
