//! mmap.rs — Gestion des mappings mémoire POSIX pour ExoFS
//!
//! Implémente mmap/munmap/msync/mprotect en s'appuyant sur les blobs ExoFS.
//! Les mappings partagés (MAP_SHARED | PROT_WRITE) déclenchent une promotion
//! de l'objet vers la classe 2 (writable). Les mappings privés (MAP_PRIVATE)
//! produisent un instantané CoW en mémoire.
//!
//! RECUR-01 / OOM-02 / ARITH-02 — ExofsError exclusivement.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use core::cell::UnsafeCell;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

pub const MMAP_MAX_MAPPINGS:  usize = 2048;
pub const MMAP_PAGE_SIZE:     u64   = 4096;
pub const MMAP_BASE_VIRT:     u64   = 0x0001_0000_0000_0000;
pub const MMAP_VIRT_STRIDE:   u64   = 0x0000_0000_0010_0000; // 1 MiB entre chaque mapping
pub const MMAP_MAGIC:         u32   = 0x4D4D_4150;           // "MMAP"
pub const MMAP_VERSION:       u8    = 1;

// ─────────────────────────────────────────────────────────────────────────────
// Flags et protections
// ─────────────────────────────────────────────────────────────────────────────

/// Flags de mapping mémoire (MAP_*).
pub mod map_flags {
    pub const MAP_SHARED:    u32 = 0x01;
    pub const MAP_PRIVATE:   u32 = 0x02;
    pub const MAP_ANONYMOUS: u32 = 0x04;
    pub const MAP_FIXED:     u32 = 0x08;
    pub const MAP_LOCKED:    u32 = 0x10;
    pub const MAP_POPULATE:  u32 = 0x20;
    pub const MAP_HUGETLB:   u32 = 0x40;
}

/// Protections de mapping mémoire (PROT_*).
pub mod map_prot {
    pub const PROT_NONE:  u32 = 0x00;
    pub const PROT_READ:  u32 = 0x01;
    pub const PROT_WRITE: u32 = 0x02;
    pub const PROT_EXEC:  u32 = 0x04;
}

/// État interne d'un mapping.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingState {
    Active  = 0,
    Dirty   = 1,
    Syncing = 2,
    Removed = 3,
}

// ─────────────────────────────────────────────────────────────────────────────
// Entrée de mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Entrée de la table des mappings mémoire.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MmapEntry {
    pub virt_addr:  u64,
    pub length:     u64,
    pub object_id:  u64,
    pub offset:     u64,
    pub prot:       u32,
    pub flags:      u32,
    pub state:      MappingState,
    pub promoted:   u8,
    pub cow_active: u8,
    pub pid:        u32,
    pub access_ctr: u64,
}

const _: () = assert!(core::mem::size_of::<MmapEntry>() == 48);

impl MmapEntry {
    pub fn is_shared(&self) -> bool   { self.flags & map_flags::MAP_SHARED != 0 }
    pub fn is_private(&self) -> bool  { self.flags & map_flags::MAP_PRIVATE != 0 }
    pub fn is_anon(&self)   -> bool   { self.flags & map_flags::MAP_ANONYMOUS != 0 }
    pub fn is_writable(&self) -> bool { self.prot & map_prot::PROT_WRITE != 0 }
    pub fn is_readable(&self) -> bool { self.prot & map_prot::PROT_READ != 0 }
    pub fn is_exec(&self) -> bool     { self.prot & map_prot::PROT_EXEC != 0 }
    pub fn end_addr(&self) -> u64     { self.virt_addr.saturating_add(self.length) }

    pub fn overlaps_range(&self, other_start: u64, other_len: u64) -> bool {
        let self_end  = self.virt_addr.saturating_add(self.length);
        let other_end = other_start.saturating_add(other_len);
        self.virt_addr < other_end && other_start < self_end
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Table des mappings
// ─────────────────────────────────────────────────────────────────────────────

pub struct MmapTable {
    entries:      UnsafeCell<Vec<MmapEntry>>,
    spinlock:     AtomicU64,
    virt_cursor:  AtomicU64,
    total_mapped: AtomicU64,
    map_count:    AtomicU64,
}

unsafe impl Sync for MmapTable {}
unsafe impl Send for MmapTable {}

pub static MMAP_TABLE: MmapTable = MmapTable::new_const();

impl MmapTable {
    pub const fn new_const() -> Self {
        Self {
            entries:      UnsafeCell::new(Vec::new()),
            spinlock:     AtomicU64::new(0),
            virt_cursor:  AtomicU64::new(MMAP_BASE_VIRT),
            total_mapped: AtomicU64::new(0),
            map_count:    AtomicU64::new(0),
        }
    }

    fn lock_acquire(&self) {
        while self.spinlock.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
    }

    fn lock_release(&self) {
        self.spinlock.store(0, Ordering::Release);
    }

    // ─── Allocation d'adresse virtuelle ───

    fn alloc_virt(&self, length: u64) -> u64 {
        let aligned = align_up(length, MMAP_PAGE_SIZE);
        let base = self.virt_cursor.fetch_add(aligned.saturating_add(MMAP_VIRT_STRIDE), Ordering::Relaxed);
        base
    }

    // ─── Recherche interne (RECUR-01 : while) ───

    fn find_by_vaddr(entries: &[MmapEntry], vaddr: u64) -> Option<usize> {
        let mut i = 0usize;
        while i < entries.len() {
            let e = &entries[i];
            if e.state != MappingState::Removed && e.virt_addr <= vaddr && vaddr < e.end_addr() {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    fn find_exact(entries: &[MmapEntry], vaddr: u64) -> Option<usize> {
        let mut i = 0usize;
        while i < entries.len() {
            if entries[i].state != MappingState::Removed && entries[i].virt_addr == vaddr {
                return Some(i);
            }
            i = i.wrapping_add(1);
        }
        None
    }

    fn count_active(entries: &[MmapEntry]) -> usize {
        let mut n = 0usize;
        let mut i = 0usize;
        while i < entries.len() {
            if entries[i].state != MappingState::Removed { n = n.wrapping_add(1); }
            i = i.wrapping_add(1);
        }
        n
    }

    // ─── Validation ───

    fn validate_args(length: u64, prot: u32, flags: u32) -> ExofsResult<()> {
        if length == 0 { return Err(ExofsError::InvalidArgument); }
        if length > 0x0001_0000_0000_0000 { return Err(ExofsError::InvalidArgument); }
        let known_prot  = map_prot::PROT_READ | map_prot::PROT_WRITE | map_prot::PROT_EXEC | map_prot::PROT_NONE;
        let known_flags = map_flags::MAP_SHARED | map_flags::MAP_PRIVATE | map_flags::MAP_ANONYMOUS
            | map_flags::MAP_FIXED | map_flags::MAP_LOCKED | map_flags::MAP_POPULATE | map_flags::MAP_HUGETLB;
        if prot  & !known_prot  != 0 { return Err(ExofsError::InvalidArgument); }
        if flags & !known_flags != 0 { return Err(ExofsError::InvalidArgument); }
        if flags & map_flags::MAP_SHARED != 0 && flags & map_flags::MAP_PRIVATE != 0 {
            return Err(ExofsError::InvalidArgument);
        }
        Ok(())
    }

    // ─── API publique ───

    /// Crée un nouveau mapping. Retourne l'adresse virtuelle.
    /// OOM-02 : try_reserve avant push.
    pub fn mmap(&self, object_id: u64, offset: u64, length: u64, prot: u32, flags: u32, pid: u32) -> ExofsResult<u64> {
        validate_page_aligned(offset)?;
        Self::validate_args(length, prot, flags)?;
        if object_id == 0 && flags & map_flags::MAP_ANONYMOUS == 0 { return Err(ExofsError::InvalidArgument); }
        let aligned_len = align_up(length, MMAP_PAGE_SIZE);
        self.lock_acquire();
        let result = self.mmap_inner(object_id, offset, aligned_len, prot, flags, pid);
        self.lock_release();
        result
    }

    fn mmap_inner(&self, object_id: u64, offset: u64, length: u64, prot: u32, flags: u32, pid: u32) -> ExofsResult<u64> {
        let entries = unsafe { &mut *self.entries.get() };
        let active = Self::count_active(entries);
        if active >= MMAP_MAX_MAPPINGS { return Err(ExofsError::QuotaExceeded); }
        let promoted   = if flags & map_flags::MAP_SHARED != 0 && prot & map_prot::PROT_WRITE != 0 { 1u8 } else { 0u8 };
        let cow_active = if flags & map_flags::MAP_PRIVATE != 0 { 1u8 } else { 0u8 };
        let virt_addr  = self.alloc_virt(length);
        let entry = MmapEntry {
            virt_addr, length, object_id, offset, prot, flags,
            state: MappingState::Active, promoted, cow_active, pid, access_ctr: 0,
        };
        entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        entries.push(entry);
        self.map_count.fetch_add(1, Ordering::Relaxed);
        self.total_mapped.fetch_add(length, Ordering::Relaxed);
        Ok(virt_addr)
    }

    /// Supprime un mapping à partir de virt_addr.
    pub fn munmap(&self, virt_addr: u64, length: u64) -> ExofsResult<()> {
        if length == 0 { return Err(ExofsError::InvalidArgument); }
        self.lock_acquire();
        let result = self.munmap_inner(virt_addr, length);
        self.lock_release();
        result
    }

    fn munmap_inner(&self, virt_addr: u64, length: u64) -> ExofsResult<()> {
        let entries = unsafe { &mut *self.entries.get() };
        let mut found = false;
        let mut i = 0usize;
        while i < entries.len() {
            let e = &mut entries[i];
            if e.state != MappingState::Removed && e.overlaps_range(virt_addr, length) {
                let freed = e.length;
                e.state = MappingState::Removed;
                let cur = self.total_mapped.load(Ordering::Relaxed);
                self.total_mapped.store(cur.saturating_sub(freed), Ordering::Relaxed);
                found = true;
            }
            i = i.wrapping_add(1);
        }
        if found { Ok(()) } else { Err(ExofsError::ObjectNotFound) }
    }

    /// Synchronise un mapping (marque Dirty → flush).
    pub fn msync(&self, virt_addr: u64, length: u64) -> ExofsResult<()> {
        if length == 0 { return Err(ExofsError::InvalidArgument); }
        self.lock_acquire();
        let result = self.msync_inner(virt_addr, length);
        self.lock_release();
        result
    }

    fn msync_inner(&self, virt_addr: u64, length: u64) -> ExofsResult<()> {
        let entries = unsafe { &mut *self.entries.get() };
        let mut found = false;
        let mut i = 0usize;
        while i < entries.len() {
            let e = &mut entries[i];
            if e.state != MappingState::Removed && e.overlaps_range(virt_addr, length) {
                if e.is_writable() && e.is_shared() { e.state = MappingState::Dirty; }
                e.access_ctr = e.access_ctr.wrapping_add(1);
                found = true;
            }
            i = i.wrapping_add(1);
        }
        if found { Ok(()) } else { Err(ExofsError::ObjectNotFound) }
    }

    /// Confirme la fin du flush (Dirty → Active).
    pub fn msync_complete(&self, virt_addr: u64) -> ExofsResult<()> {
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = Self::find_by_vaddr(entries, virt_addr) {
            entries[idx].state = MappingState::Active; Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.lock_release();
        result
    }

    /// Change les protections d'un mapping.
    pub fn mprotect(&self, virt_addr: u64, prot: u32) -> ExofsResult<()> {
        let known_prot = map_prot::PROT_READ | map_prot::PROT_WRITE | map_prot::PROT_EXEC | map_prot::PROT_NONE;
        if prot & !known_prot != 0 { return Err(ExofsError::InvalidArgument); }
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = Self::find_exact(entries, virt_addr) {
            entries[idx].prot = prot;
            if prot & map_prot::PROT_WRITE == 0 { entries[idx].promoted = 0; }
            Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.lock_release();
        result
    }

    /// Retourne une copie de l'entrée à une adresse virtuelle.
    pub fn find_mapping(&self, virt_addr: u64) -> Option<MmapEntry> {
        self.lock_acquire();
        let entries = unsafe { &*self.entries.get() };
        let r = Self::find_by_vaddr(entries, virt_addr).map(|i| entries[i]);
        self.lock_release();
        r
    }

    /// Retourne toutes les entrées actives pour un pid.
    /// OOM-02 : try_reserve. RECUR-01 : while.
    pub fn mappings_for_pid(&self, pid: u32) -> ExofsResult<Vec<MmapEntry>> {
        self.lock_acquire();
        let entries = unsafe { &*self.entries.get() };
        let mut out: Vec<MmapEntry> = Vec::new();
        out.try_reserve(entries.len()).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < entries.len() {
            if entries[i].pid == pid && entries[i].state != MappingState::Removed {
                out.push(entries[i]);
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
        Ok(out)
    }

    /// Supprime tous les mappings d'un pid (appelé à la mort du processus).
    pub fn munmap_all_pid(&self, pid: u32) {
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let mut i = 0usize;
        while i < entries.len() {
            if entries[i].pid == pid && entries[i].state != MappingState::Removed {
                let freed = entries[i].length;
                entries[i].state = MappingState::Removed;
                let cur = self.total_mapped.load(Ordering::Relaxed);
                self.total_mapped.store(cur.saturating_sub(freed), Ordering::Relaxed);
            }
            i = i.wrapping_add(1);
        }
        self.lock_release();
    }

    /// Nombre de mappings actifs.
    pub fn mapping_count(&self) -> usize {
        self.lock_acquire();
        let entries = unsafe { &*self.entries.get() };
        let n = Self::count_active(entries);
        self.lock_release();
        n
    }

    /// Taille totale mappée.
    pub fn total_mapped_bytes(&self) -> u64 {
        self.total_mapped.load(Ordering::Relaxed)
    }

    /// Marque un mapping comme Dirty (page fault write).
    pub fn mark_dirty(&self, virt_addr: u64) -> ExofsResult<()> {
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let result = if let Some(idx) = Self::find_by_vaddr(entries, virt_addr) {
            if entries[idx].is_shared() && entries[idx].is_writable() {
                entries[idx].state = MappingState::Dirty;
            }
            Ok(())
        } else { Err(ExofsError::ObjectNotFound) };
        self.lock_release();
        result
    }

    /// Purge les entrées Removed.
    pub fn compact(&self) {
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        let mut i = 0usize;
        while i < entries.len() {
            if entries[i].state == MappingState::Removed {
                entries.remove(i);
            } else {
                i = i.wrapping_add(1);
            }
        }
        self.lock_release();
    }

    /// Vide entièrement la table.
    pub fn clear(&self) {
        self.lock_acquire();
        let entries = unsafe { &mut *self.entries.get() };
        entries.clear();
        self.total_mapped.store(0, Ordering::Relaxed);
        self.lock_release();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Aligne `v` sur `align` (puissance de 2). ARITH-02.
pub fn align_up(v: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    let mask = align.wrapping_sub(1);
    v.wrapping_add(mask) & !mask
}

/// Vérifie qu'un offset est aligné sur une page.
pub fn validate_page_aligned(offset: u64) -> ExofsResult<()> {
    if offset & (MMAP_PAGE_SIZE.wrapping_sub(1)) != 0 { Err(ExofsError::InvalidArgument) } else { Ok(()) }
}

/// Calcule le nombre de pages nécessaires.
pub fn pages_for(length: u64) -> u64 {
    align_up(length, MMAP_PAGE_SIZE) / MMAP_PAGE_SIZE
}

/// Retourne vrai si deux plages se chevauchent. ARITH-02 saturating.
pub fn ranges_overlap(a: u64, la: u64, b: u64, lb: u64) -> bool {
    let ae = a.saturating_add(la);
    let be = b.saturating_add(lb);
    a < be && b < ae
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::map_flags::*;
    use super::map_prot::*;

    fn make_table() -> MmapTable { MmapTable::new_const() }

    #[test]
    fn test_mmap_entry_size() { assert_eq!(core::mem::size_of::<MmapEntry>(), 48); }

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 4096), 0);
        assert_eq!(align_up(1, 4096), 4096);
        assert_eq!(align_up(4096, 4096), 4096);
        assert_eq!(align_up(4097, 4096), 8192);
    }

    #[test]
    fn test_mmap_basic() {
        let t = make_table();
        let va = t.mmap(1, 0, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, 42).unwrap();
        assert!(va >= MMAP_BASE_VIRT);
        assert_eq!(t.mapping_count(), 1);
    }

    #[test]
    fn test_mmap_shared_promotes() {
        let t = make_table();
        let va = t.mmap(2, 0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, 1).unwrap();
        let entry = t.find_mapping(va).unwrap();
        assert_eq!(entry.promoted, 1);
    }

    #[test]
    fn test_mmap_private_cow() {
        let t = make_table();
        let va = t.mmap(3, 0, 4096, PROT_READ, MAP_PRIVATE, 1).unwrap();
        let entry = t.find_mapping(va).unwrap();
        assert_eq!(entry.cow_active, 1);
    }

    #[test]
    fn test_munmap() {
        let t = make_table();
        let va = t.mmap(4, 0, 4096, PROT_READ, MAP_PRIVATE, 1).unwrap();
        t.munmap(va, 4096).unwrap();
        assert!(t.find_mapping(va).is_none());
    }

    #[test]
    fn test_munmap_not_found() {
        let t = make_table();
        assert!(matches!(t.munmap(0xDEAD_0000, 4096), Err(ExofsError::ObjectNotFound)));
    }

    #[test]
    fn test_msync_dirty() {
        let t = make_table();
        let va = t.mmap(5, 0, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, 1).unwrap();
        t.msync(va, 4096).unwrap();
        let entry = t.find_mapping(va).unwrap();
        assert_eq!(entry.state as u8, MappingState::Dirty as u8);
    }

    #[test]
    fn test_mprotect() {
        let t = make_table();
        let va = t.mmap(6, 0, 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE, 1).unwrap();
        t.mprotect(va, PROT_READ).unwrap();
        let e = t.find_mapping(va).unwrap();
        assert!(!e.is_writable());
    }

    #[test]
    fn test_mprotect_invalid_prot() {
        let t = make_table();
        let va = t.mmap(7, 0, 4096, PROT_READ, MAP_PRIVATE, 1).unwrap();
        assert!(matches!(t.mprotect(va, 0xFF), Err(ExofsError::InvalidArgument)));
    }

    #[test]
    fn test_munmap_all_pid() {
        let t = make_table();
        t.mmap(8,  0, 4096, PROT_READ, MAP_PRIVATE, 10).unwrap();
        t.mmap(9,  0, 4096, PROT_READ, MAP_PRIVATE, 10).unwrap();
        t.mmap(10, 0, 4096, PROT_READ, MAP_PRIVATE, 20).unwrap();
        t.munmap_all_pid(10);
        assert_eq!(t.mappings_for_pid(10).unwrap().len(), 0);
        assert_eq!(t.mappings_for_pid(20).unwrap().len(), 1);
    }

    #[test]
    fn test_ranges_overlap() {
        assert!( ranges_overlap(0, 100, 50, 100));
        assert!(!ranges_overlap(0, 100, 100, 100));
        assert!( ranges_overlap(50, 100, 0, 60));
    }

    #[test]
    fn test_validate_page_aligned() {
        assert!(validate_page_aligned(0).is_ok());
        assert!(validate_page_aligned(4096).is_ok());
        assert!(validate_page_aligned(1).is_err());
    }

    #[test]
    fn test_compact() {
        let t = make_table();
        let va = t.mmap(11, 0, 4096, PROT_READ, MAP_PRIVATE, 1).unwrap();
        t.munmap(va, 4096).unwrap();
        t.compact();
        assert_eq!(t.mapping_count(), 0);
    }

    #[test]
    fn test_pages_for() {
        assert_eq!(pages_for(1), 1);
        assert_eq!(pages_for(4096), 1);
        assert_eq!(pages_for(4097), 2);
    }
}
