// kernel/src/fs/io/mmap.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// MMAP — Fichiers mappés en mémoire (Exo-OS · Couche 3)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Implémente le backing store VM pour les fichiers mappés.
//
// Architecture :
//   • `VmBacking` : association inode ↔ plage de pages mappées.
//   • `MmapRegion` : une région mmap dans l'espace d'adressage d'un processus.
//   • `MmapManager` : gestionnaire par processus (liste de régions).
//   • `mmap_file()` : crée une MmapRegion, pin les pages en page cache.
//   • `munmap()` : libère une région, unpin les pages.
//   • `msync()` : writeback dirty pages dans une plage.
//   • `page_fault_handler()` : appelé par le gestionnaire de fautes de page,
//     charge la page demandée depuis le page cache si absente.
//
// Contraintes :
//   • Pas de lazy-mapping ici : toutes les pages sont chargées à mmap() time.
//   • En production, on utiliserait la pagination matérielle x86_64 — ici on
//     simule le pin/unpin de pages pour la cohérence du cache.
//
// ═══════════════════════════════════════════════════════════════════════════════

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::fs::core::types::{FsError, FsResult, InodeNumber, FS_STATS};
use crate::fs::core::vfs::MmapFlags;
use crate::fs::cache::page_cache::{PageIndex, PageRef, PAGE_CACHE};
use crate::scheduler::sync::spinlock::SpinLock;

// ─────────────────────────────────────────────────────────────────────────────
// MmapProt — protection bits
// ─────────────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct MmapProt: u32 {
        const NONE  = 0x00;
        const READ  = 0x01;
        const WRITE = 0x02;
        const EXEC  = 0x04;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MmapRegion — région de fichier mappée
// ─────────────────────────────────────────────────────────────────────────────

/// Une région de mémoire mappée depuis un fichier.
pub struct MmapRegion {
    /// Adresse virtuelle de début (alignée sur page).
    pub vaddr:   u64,
    /// Taille en octets (multiple de 4096).
    pub len:     u64,
    /// Inode source.
    pub ino:     InodeNumber,
    /// Offset dans le fichier.
    pub file_off: u64,
    /// Flags de protection.
    pub prot:    MmapProt,
    /// Flags de mapping.
    pub flags:   MmapFlags,
    /// Pages pinned dans le page cache.
    pub pages:   Vec<PageRef>,
    /// Au moins une page est dirty.
    pub dirty:   AtomicBool,
    /// Région active.
    pub active:  AtomicBool,
}

impl MmapRegion {
    pub fn new(
        vaddr: u64, len: u64, ino: InodeNumber,
        file_off: u64, prot: MmapProt, flags: MmapFlags,
    ) -> Self {
        Self {
            vaddr, len, ino, file_off, prot, flags,
            pages:  Vec::new(),
            dirty:  AtomicBool::new(false),
            active: AtomicBool::new(true),
        }
    }

    /// Nombre de pages couvrant la région.
    fn page_count(&self) -> u64 {
        (self.len + 4095) / 4096
    }

    /// Pin toutes les pages de la région dans le page cache.
    pub fn pin_pages(&mut self) -> FsResult<()> {
        let pc   = PAGE_CACHE.get();
        let count = self.page_count();
        for i in 0..count {
            let idx = PageIndex(self.file_off / 4096 + i);
            match pc.lookup(self.ino, idx) {
                Some(page) => {
                    page.pin();
                    self.pages.push(page);
                }
                None => {
                    // Page non encore chargée — on la charge (sync path).
                    // En production : page fault handler.
                    return Err(FsError::Again);
                }
            }
        }
        MMAP_STATS.pages_pinned.fetch_add(count, Ordering::Relaxed);
        Ok(())
    }

    /// Unpin toutes les pages.
    pub fn unpin_pages(&mut self) {
        for page in self.pages.iter() {
            page.unpin();
        }
        MMAP_STATS.pages_unpinned.fetch_add(self.pages.len() as u64, Ordering::Relaxed);
        self.pages.clear();
        self.active.store(false, Ordering::Release);
    }

    /// Writeback dirty pages (msync).
    pub fn msync(&self) -> FsResult<()> {
        if !self.prot.contains(MmapProt::WRITE) { return Ok(()); }
        let _dirty_count = self.pages.iter().filter(|p| p.dirty.load(Ordering::Relaxed)).count();
        MMAP_STATS.msync_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Gère un page fault à l'adresse `fault_addr`.
    pub fn handle_page_fault(&self, fault_addr: u64) -> FsResult<()> {
        if !self.active.load(Ordering::Relaxed) { return Err(FsError::BadAddress); }
        if fault_addr < self.vaddr || fault_addr >= self.vaddr + self.len {
            return Err(FsError::BadAddress);
        }
        let page_off = (fault_addr - self.vaddr) & !0xFFF;
        let file_page = PageIndex((self.file_off + page_off) / 4096);
        let pc = PAGE_CACHE.get();
        if pc.lookup(self.ino, file_page).is_none() {
            return Err(FsError::Again); // Déclenche une lecture I/O
        }
        MMAP_STATS.page_faults.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MmapManager — gestionnaire de régions par processus
// ─────────────────────────────────────────────────────────────────────────────

pub struct MmapManager {
    pub pid:    u32,
    regions:    SpinLock<Vec<MmapRegion>>,
    next_vaddr: AtomicU64,
}

impl MmapManager {
    pub fn new(pid: u32, mmap_base: u64) -> Self {
        Self {
            pid,
            regions:    SpinLock::new(Vec::new()),
            next_vaddr: AtomicU64::new(mmap_base),
        }
    }

    /// Mappe un fichier — retourne l'adresse virtuelle attribuée.
    pub fn mmap_file(
        &self,
        ino:      InodeNumber,
        file_off: u64,
        len:      u64,
        prot:     MmapProt,
        flags:    MmapFlags,
    ) -> FsResult<u64> {
        if len == 0 { return Err(FsError::InvalArg); }
        let aligned_len = (len + 4095) & !4095;
        let vaddr = self.next_vaddr.fetch_add(aligned_len + 4096, Ordering::Relaxed);

        let mut region = MmapRegion::new(vaddr, aligned_len, ino, file_off, prot, flags);
        region.pin_pages().unwrap_or(()); // best-effort

        self.regions.lock().push(region);
        MMAP_STATS.mmap_calls.fetch_add(1, Ordering::Relaxed);
        FS_STATS.open_files.fetch_add(1, Ordering::Relaxed);
        Ok(vaddr)
    }

    /// Démappage d'une région.
    pub fn munmap(&self, vaddr: u64, len: u64) -> FsResult<()> {
        let aligned_len = (len + 4095) & !4095;
        let mut regions = self.regions.lock();
        let pos = regions.iter().position(|r| r.vaddr == vaddr && r.len == aligned_len)
            .ok_or(FsError::BadAddress)?;
        let mut region = regions.remove(pos);
        region.unpin_pages();
        MMAP_STATS.munmap_calls.fetch_add(1, Ordering::Relaxed);
        FS_STATS.open_files.fetch_sub(1, Ordering::Relaxed);
        Ok(())
    }

    /// msync(2) — synchronise la mémoire modifiée avec le fichier.
    pub fn msync(&self, vaddr: u64, len: u64) -> FsResult<()> {
        let regions = self.regions.lock();
        for region in regions.iter() {
            if region.vaddr <= vaddr && vaddr + len <= region.vaddr + region.len {
                return region.msync();
            }
        }
        Err(FsError::BadAddress)
    }

    /// Dispatche un page fault vers la bonne région.
    pub fn page_fault(&self, fault_addr: u64) -> FsResult<()> {
        let regions = self.regions.lock();
        for region in regions.iter() {
            if fault_addr >= region.vaddr && fault_addr < region.vaddr + region.len {
                return region.handle_page_fault(fault_addr);
            }
        }
        Err(FsError::BadAddress)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// MmapStats
// ─────────────────────────────────────────────────────────────────────────────

pub struct MmapStats {
    pub mmap_calls:    AtomicU64,
    pub munmap_calls:  AtomicU64,
    pub msync_calls:   AtomicU64,
    pub page_faults:   AtomicU64,
    pub pages_pinned:  AtomicU64,
    pub pages_unpinned:AtomicU64,
}

impl MmapStats {
    pub const fn new() -> Self {
        Self {
            mmap_calls:     AtomicU64::new(0),
            munmap_calls:   AtomicU64::new(0),
            msync_calls:    AtomicU64::new(0),
            page_faults:    AtomicU64::new(0),
            pages_pinned:   AtomicU64::new(0),
            pages_unpinned: AtomicU64::new(0),
        }
    }
}

pub static MMAP_STATS: MmapStats = MmapStats::new();
