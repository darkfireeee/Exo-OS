// kernel/src/security/capability/table.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CAP TABLE — Per-process capability table (Exo-OS Security · Layer 2b)
// ═══════════════════════════════════════════════════════════════════════════════
//
// ⚠️  OUT of formal verification scope (proptest + INVARIANTS.md only).
//     table.rs is the concrete implementation — not included in the proof.
//     The formal scope covers: model.rs, token.rs, rights.rs,
//     revocation.rs, delegation.rs (see DOC7, RULE CAP-02).
//
// STRUCTURE:
//   Each process owns a CapTable corresponding to an ObjectId→Entry map.
//   The implementation uses a static array of CAP_TABLE_CAPACITY entries
//   with a global spinlock — no heap allocation in the verification hot path.
//
// PROPERTIES:
//   • Insertion: amortized O(1) (linear scan of free slots)
//   • Verification: O(1) — direct access by hashed index
//   • Revocation: O(1) — atomic increment of generation
//   • Thread-safety: Spinlock on mutations, atomic read of generation
//
// RESERVED MEMORY SPACE:
//   CAP_TABLE_CAPACITY × size_of::<CapEntry>() = 512 × 24 = 12 288 bytes ≈ 12 KiB
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use super::rights::Rights;
use super::token::{CapObjectType, CapToken, ObjectId};
use super::verify::CapError;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Maximum entry capacity per CapTable (per process).
/// 512 concurrent capabilities — sufficient for a complex process.
pub const CAP_TABLE_CAPACITY: usize = 512;

/// Empty slot marker.
const SLOT_FREE: u64 = u64::MAX;

// ─────────────────────────────────────────────────────────────────────────────
// CapEntry — internal table entry
// ─────────────────────────────────────────────────────────────────────────────

/// Internal entry in CapTable.
/// Size: 8+4+4+4+4 = 24 bytes, aligned to 8.
#[repr(C)]
struct CapEntry {
    /// Object identifier — SLOT_FREE if free.
    object_id: AtomicU64,
    /// Rights granted for this object.
    rights: AtomicU32,
    /// Current generation — incremented upon revocation.
    generation: AtomicU32,
    /// Object type — stored for fast validation.
    type_tag: AtomicU32,
    /// Padding for cache-line-friendly alignment.
    _pad: AtomicU32,
}

impl CapEntry {
    const fn new_free() -> Self {
        Self {
            object_id: AtomicU64::new(SLOT_FREE),
            rights: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            type_tag: AtomicU32::new(0),
            _pad: AtomicU32::new(0),
        }
    }

    #[inline(always)]
    fn is_free(&self) -> bool {
        self.object_id.load(Ordering::Acquire) == SLOT_FREE
    }

    /// Returns (rights, generation, type_tag) if the ObjectId matches.
    #[inline(always)]
    #[allow(dead_code)]
    fn load_for_verify(&self, expected_oid: ObjectId) -> Option<(Rights, u32, CapObjectType)> {
        let oid = self.object_id.load(Ordering::Acquire);
        if oid != expected_oid.0 {
            return None;
        }
        let r = Rights::from_bits_truncate(self.rights.load(Ordering::Acquire));
        let gen = self.generation.load(Ordering::Acquire);
        let tt = CapObjectType::from_u16(self.type_tag.load(Ordering::Acquire) as u16);
        Some((r, gen, tt))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapTable — main structure
// ─────────────────────────────────────────────────────────────────────────────

/// Capability table of a process.
///
/// # Process security
/// Each process owns its own CapTable — no direct sharing.
/// Write access is protected by a Mutex; verification (atomic read)
/// does not take the lock under normal conditions.
pub struct CapTable {
    /// Fixed array of entries — no heap allocation.
    entries: [CapEntry; CAP_TABLE_CAPACITY],
    /// Mutex for mutations (grant, revoke, inherit).
    /// Not taken on verify() path except fallback.
    write_lock: Mutex<()>,
    /// Number of currently occupied entries.
    count: AtomicU32,
    /// Statistics.
    stats: CapTableStats,
}

/// Statistics per table.
struct CapTableStats {
    grants: AtomicU64,
    revocations: AtomicU64,
    lookups: AtomicU64,
    misses: AtomicU64,
}

impl CapTableStats {
    const fn new() -> Self {
        Self {
            grants: AtomicU64::new(0),
            revocations: AtomicU64::new(0),
            lookups: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }
}

/// CapTable statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct CapTableSnapshot {
    pub grants: u64,
    pub revocations: u64,
    pub lookups: u64,
    pub misses: u64,
    pub count: u32,
}

// ─────────────────────────────────────────────────────────────────────────────
// CapTable implementation
// ─────────────────────────────────────────────────────────────────────────────

// SAFETY: CapTable contains only atomic types + Mutex.
// Concurrent accesses are properly synchronized.
unsafe impl Send for CapTable {}
unsafe impl Sync for CapTable {}

// Macro to create a [CapEntry; N] array with free entries.
// Necessary because CapEntry is not Copy.
macro_rules! init_cap_entries {
    () => {{
        let mut arr: [core::mem::MaybeUninit<CapEntry>; CAP_TABLE_CAPACITY] =
            // SAFETY: MaybeUninit uninit array is valid; each slot is overwritten by new_free() afterwards.
            unsafe { core::mem::MaybeUninit::uninit().assume_init() };
        let mut i = 0usize;
        while i < CAP_TABLE_CAPACITY {
            arr[i] = core::mem::MaybeUninit::new(CapEntry::new_free());
            i += 1;
        }
        // SAFETY: All elements initialized.
        unsafe { core::mem::transmute::<_, [CapEntry; CAP_TABLE_CAPACITY]>(arr) }
    }};
}

impl CapTable {
    /// Creates a new empty CapTable.
    pub fn new() -> Self {
        Self {
            entries: init_cap_entries!(),
            write_lock: Mutex::new(()),
            count: AtomicU32::new(0),
            stats: CapTableStats::new(),
        }
    }

    /// FIX-P2-A03 (Security_Audit_Passe2 §A-03): creates a CapTable inherited
    /// from the parent for fork(). Copies all non-free entries.
    ///
    /// Uses real fields of CapEntry: object_id, rights, generation, type_tag.
    /// A slot is free if object_id == SLOT_FREE (u64::MAX).
    pub fn inherit_from(parent: &CapTable) -> Self {
        let child = Self::new();
        let _guard = parent.write_lock.lock();
        let mut inherited = 0u32;
        for i in 0..CAP_TABLE_CAPACITY {
            let oid = parent.entries[i].object_id.load(Ordering::Acquire);
            // SLOT_FREE = u64::MAX means empty slot — do not copy.
            if oid != u64::MAX {
                // Copy all fields atomically (under parent's write_lock).
                // SAFETY: write_lock is held, no concurrent writer on parent.
                child.entries[i].object_id.store(oid, Ordering::Relaxed);
                child.entries[i].rights.store(
                    parent.entries[i].rights.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                child.entries[i].generation.store(
                    parent.entries[i].generation.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                child.entries[i].type_tag.store(
                    parent.entries[i].type_tag.load(Ordering::Relaxed),
                    Ordering::Release,
                );
                inherited += 1;
            }
        }
        child.count.store(inherited, Ordering::Release);
        child
    }

    /// FIX-SEC-T1.0: fork inheritance with **least privilege**. Like `inherit_from`,
    /// but strips `strip_rights` bits from rights of any cap where
    /// `type_tag == strip_type`. Prevents automatic propagation of privileged FS
    /// rights (gc/import/snapshot/admin) to children: only an explicit `grant` or
    /// delegation re-establishes them. Operational rights (read/write/…)
    /// are preserved. Other types of caps are copied as is.
    pub fn inherit_from_masked(
        parent: &CapTable,
        strip_rights: Rights,
        strip_type: CapObjectType,
    ) -> Self {
        let child = Self::new();
        let _guard = parent.write_lock.lock();
        let mut inherited = 0u32;
        let strip = strip_rights.bits();
        let strip_tt = strip_type as u32;
        for i in 0..CAP_TABLE_CAPACITY {
            let oid = parent.entries[i].object_id.load(Ordering::Acquire);
            if oid != u64::MAX {
                let tt = parent.entries[i].type_tag.load(Ordering::Relaxed);
                let mut r = parent.entries[i].rights.load(Ordering::Relaxed);
                if tt == strip_tt {
                    r &= !strip;
                }
                child.entries[i].object_id.store(oid, Ordering::Relaxed);
                child.entries[i].rights.store(r, Ordering::Relaxed);
                child.entries[i].generation.store(
                    parent.entries[i].generation.load(Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                child.entries[i].type_tag.store(tt, Ordering::Release);
                inherited += 1;
            }
        }
        child.count.store(inherited, Ordering::Release);
        child
    }

    // ── Hashing ObjectId → index ─────────────────────────────────────────────

    /// Hashes an ObjectId into a starting index in the array.
    /// Uses 64-bit FNV-1a condensed to 9 bits.
    #[inline(always)]
    fn hash_index(oid: ObjectId) -> usize {
        let h = oid.0.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let h = h ^ (h >> 30);
        (h as usize) & (CAP_TABLE_CAPACITY - 1)
    }

    // ── Slot lookup ──────────────────────────────────────────────────────────

    /// Finds the entry index for a given ObjectId (linear probe).
    /// Returns None if not found.
    #[inline]
    fn find_slot(&self, oid: ObjectId) -> Option<usize> {
        let start = Self::hash_index(oid);
        for probe in 0..CAP_TABLE_CAPACITY {
            let idx = (start + probe) & (CAP_TABLE_CAPACITY - 1);
            let entry = &self.entries[idx];
            let stored = entry.object_id.load(Ordering::Acquire);
            if stored == oid.0 {
                return Some(idx);
            }
            if stored == SLOT_FREE {
                // No wrapped-around possible without more filling
                return None;
            }
        }
        None
    }

    /// Finds a free slot — used during grant().
    #[inline]
    fn find_free_slot(&self, oid: ObjectId) -> Option<usize> {
        let start = Self::hash_index(oid);
        for probe in 0..CAP_TABLE_CAPACITY {
            let idx = (start + probe) & (CAP_TABLE_CAPACITY - 1);
            if self.entries[idx].is_free() {
                return Some(idx);
            }
            // If same ObjectId already present, return this index (update)
            if self.entries[idx].object_id.load(Ordering::Acquire) == oid.0 {
                return Some(idx);
            }
        }
        None
    }

    // ── Public API ───────────────────────────────────────────────────────────

    /// Adds an entry to the table and returns the corresponding CapToken.
    ///
    /// # Errors
    /// - `CapError::TableFull` if the table is full.
    ///
    /// # Thread-safety
    /// Acquires the write mutex.
    pub fn grant(
        &self,
        object_id: ObjectId,
        rights: Rights,
        type_tag: CapObjectType,
    ) -> Result<CapToken, CapError> {
        let _guard = self.write_lock.lock();

        let idx = self.find_free_slot(object_id).ok_or(CapError::TableFull)?;
        let entry = &self.entries[idx];

        let gen = if entry.is_free() {
            // New slot — generation 0
            entry.rights.store(rights.bits(), Ordering::Release);
            entry.type_tag.store(type_tag as u32, Ordering::Release);
            entry.generation.store(0, Ordering::Release);
            // Atomic publication of ObjectId — must be LAST
            entry.object_id.store(object_id.0, Ordering::Release);
            self.count.fetch_add(1, Ordering::Relaxed);
            0
        } else {
            // Update of existing slot (re-grant after revocation)
            let current_gen = entry.generation.load(Ordering::Acquire);
            entry.rights.store(rights.bits(), Ordering::Release);
            entry.type_tag.store(type_tag as u32, Ordering::Release);
            current_gen
        };

        self.stats.grants.fetch_add(1, Ordering::Relaxed);
        Ok(CapToken::new(object_id, rights, gen, type_tag))
    }

    /// Revokes all tokens for a given ObjectId.
    ///
    /// Revocation is O(1): only the generation counter is incremented.
    /// Existing tokens with the old generation will return Err(Revoked).
    ///
    /// # VERIFIED PROPERTY (proptest + INVARIANTS.md — LAC-02)
    /// ∀ token t, revoke(obj) → verify(t) = Err(Denied)
    pub fn revoke(&self, object_id: ObjectId) -> Result<(), CapError> {
        if let Some(idx) = self.find_slot(object_id) {
            // Atomic increment of generation — invalidates ALL existing tokens
            self.entries[idx].generation.fetch_add(1, Ordering::Release);
            // Reset rights to zero — immediately invalidates check_object and check_right_any_type
            self.entries[idx].rights.store(0, Ordering::Release);
            self.stats.revocations.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(CapError::ObjectNotFound)
        }
    }

    /// Completely removes an entry (frees the slot).
    /// Used during object destruction.
    pub fn remove(&self, object_id: ObjectId) -> Result<(), CapError> {
        let _guard = self.write_lock.lock();
        if let Some(idx) = self.find_slot(object_id) {
            // First revoke (increment generation)
            self.entries[idx].generation.fetch_add(1, Ordering::Release);
            // Then free the slot by marking it free
            self.entries[idx]
                .object_id
                .store(SLOT_FREE, Ordering::Release);
            self.entries[idx].rights.store(0, Ordering::Release);
            self.count.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(CapError::ObjectNotFound)
        }
    }

    /// Directly increments the generation counter of an object.
    /// Used by internal `revocation::revoke()`.
    #[inline]
    pub(super) fn increment_generation(&self, object_id: ObjectId, ord: Ordering) {
        if let Some(idx) = self.find_slot(object_id) {
            self.entries[idx].generation.fetch_add(1, ord);
        }
    }

    /// Loads the entry for a given ObjectId — used by `revocation::verify()`.
    #[inline]
    pub(super) fn get(&self, object_id: ObjectId) -> Option<CapEntryView> {
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);
        if let Some(idx) = self.find_slot(object_id) {
            let entry = &self.entries[idx];
            let rights = Rights::from_bits_truncate(entry.rights.load(Ordering::Acquire));
            let gen = entry.generation.load(Ordering::Acquire);
            let tt = CapObjectType::from_u16(entry.type_tag.load(Ordering::Acquire) as u16);
            Some(CapEntryView {
                rights,
                generation: gen,
                type_tag: tt,
            })
        } else {
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Checks whether the object exists in the table.
    #[inline]
    pub fn contains(&self, object_id: ObjectId) -> bool {
        self.find_slot(object_id).is_some()
    }

    /// FIX-SEC-T0: PUBLIC enforcement entry point — verifies (atomic read)
    /// that a capability exists for `object_id`, of type `ty`, covering **at least**
    /// the `required` rights. Avoids exposing `get()` (reserved for the module). Used by
    /// ExoFS enforcement ([fs/exofs/syscall/captable.rs]).
    #[inline]
    pub fn check_object(&self, object_id: ObjectId, required: Rights, ty: CapObjectType) -> bool {
        match self.get(object_id) {
            Some(v) => v.type_tag == ty && v.rights.contains(required),
            None => false,
        }
    }

    /// Checks that a capability exists for `object_id` covering at least the `required` rights,
    /// regardless of the object type (used for universal rights such as REVOKE).
    #[inline]
    pub fn check_right_any_type(&self, object_id: ObjectId, required: Rights) -> bool {
        match self.get(object_id) {
            Some(v) => v.rights.contains(required),
            None => false,
        }
    }

    /// Returns a stats snapshot.
    pub fn stats(&self) -> CapTableSnapshot {
        CapTableSnapshot {
            grants: self.stats.grants.load(Ordering::Relaxed),
            revocations: self.stats.revocations.load(Ordering::Relaxed),
            lookups: self.stats.lookups.load(Ordering::Relaxed),
            misses: self.stats.misses.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }

    /// Number of active entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed) as usize
    }

    /// True if the table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for CapTable {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CapEntryView — read view
// ─────────────────────────────────────────────────────────────────────────────

/// Read view of an entry — returned by `get()`.
#[derive(Debug, Clone, Copy)]
pub struct CapEntryView {
    pub rights: Rights,
    pub generation: u32,
    pub type_tag: CapObjectType,
}

#[cfg(test)]
mod tier_sec_tests {
    use super::*;

    // Local ExoFS bits (avoids dependency on fs/exofs crate in test).
    const R_READ: u32 = 1 << 0;
    const R_WRITE: u32 = 1 << 1;
    const R_GC: u32 = 1 << 13; // RIGHT_GC_TRIGGER (privileged)
    const R_ADMIN: u32 = 1 << 16; // RIGHT_ADMIN (privileged)
    const PRIV: u32 = R_GC | R_ADMIN;

    fn oid(v: u64) -> ObjectId {
        ObjectId::from_raw(v)
    }
    fn r(bits: u32) -> Rights {
        Rights::from_bits_truncate(bits)
    }

    /// FIX-SEC-T0: `check_object` really enforces rights + type (not a self-declared
    /// bitmask). This proves that ExoFS enforcement is not theater.
    #[test]
    fn check_object_enforces_rights_and_type() {
        let t = CapTable::new();
        t.grant(oid(42), r(R_READ), CapObjectType::FileInode)
            .unwrap();
        assert!(t.check_object(oid(42), r(R_READ), CapObjectType::FileInode));
        assert!(!t.check_object(oid(42), r(R_WRITE), CapObjectType::FileInode)); // no WRITE
        assert!(!t.check_object(oid(42), r(R_READ), CapObjectType::IpcEndpoint)); // wrong type
        assert!(!t.check_object(oid(99), r(R_READ), CapObjectType::FileInode)); // unknown object
    }

    /// Revocation (remove) invalidates cap → check denies.
    #[test]
    fn revoke_then_check_denies() {
        let t = CapTable::new();
        t.grant(oid(7), r(R_READ | R_WRITE), CapObjectType::FileInode)
            .unwrap();
        assert!(t.check_object(oid(7), r(R_WRITE), CapObjectType::FileInode));
        t.remove(oid(7)).unwrap();
        assert!(!t.check_object(oid(7), r(R_READ), CapObjectType::FileInode));
    }

    /// FIX-SEC-T1.0: least-privilege fork inheritance strips privileged FS rights
    /// (gc/admin) from child but preserves read/write; parent (init)
    /// keeps everything.
    #[test]
    fn fork_inherit_masked_strips_privileged_fs_rights() {
        let parent = CapTable::new();
        parent
            .grant(
                oid(0),
                r(R_READ | R_WRITE | R_GC | R_ADMIN),
                CapObjectType::FileInode,
            )
            .unwrap();
        let child = CapTable::inherit_from_masked(&parent, r(PRIV), CapObjectType::FileInode);
        assert!(child.check_object(oid(0), r(R_READ | R_WRITE), CapObjectType::FileInode));
        assert!(!child.check_object(oid(0), r(R_GC), CapObjectType::FileInode));
        assert!(!child.check_object(oid(0), r(R_ADMIN), CapObjectType::FileInode));
        // init keeps all its rights.
        assert!(parent.check_object(oid(0), r(R_ADMIN), CapObjectType::FileInode));
    }
}
