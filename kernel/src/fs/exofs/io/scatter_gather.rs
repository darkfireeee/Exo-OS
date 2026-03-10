//! scatter_gather.rs — Listes Scatter-Gather pour DMA (no_std).
//!
//! Ce module fournit :
//!  - `SgFragment`      : segment virtuel (ptr + len) pour Scatter-Gather.
//!  - `SgList`          : liste de fragments, gather_read / scatter_write.
//!  - `PhysSegment`     : segment physique (DMA bus address).
//!  - `PhysSgList`      : liste de segments physiques.
//!  - `SgStats`         : statistiques des opérations SG.
//!  - `SgBuffer`        : pool de fragments alloués (fallback).
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.
//! SAFETY   : chaque bloc `unsafe` est documenté avec sa justification.


extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─── SgFragment ───────────────────────────────────────────────────────────────

/// Un segment virtuel pour Scatter-Gather.
///
/// SAFETY: `ptr` doit pointer vers une mémoire valide pendant toute
/// la durée de vie de `SgFragment`.
#[derive(Debug, Clone, Copy)]
pub struct SgFragment {
    pub ptr: *mut u8,
    pub len: usize,
}

// SAFETY: les pointeurs SgFragment sont transmis entre threads sous
// contrôle exclusif du moteur DMA — aucun accès concurrent.
unsafe impl Send for SgFragment {}
unsafe impl Sync for SgFragment {}

impl SgFragment {
    /// Crée un fragment à partir d'un slice mutable.
    ///
    /// SAFETY: le slice doit rester valide aussi longtemps que ce fragment.
    pub fn from_slice(s: &mut [u8]) -> Self {
        Self { ptr: s.as_mut_ptr(), len: s.len() }
    }

    /// Crée un fragment en lecture seule (ptr const casté en mut).
    ///
    /// SAFETY: ce fragment ne doit pas être utilisé pour scatter_write.
    pub fn from_const_slice(s: &[u8]) -> Self {
        // SAFETY: utilisé uniquement en lecture — jamais écrit via ce ptr.
        Self { ptr: s.as_ptr() as *mut u8, len: s.len() }
    }

    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Lit `buf.len()` octets depuis ce fragment.
    ///
    /// SAFETY: `ptr` doit pointer vers au moins `self.len` octets valides.
    pub fn read_into(&self, buf: &mut [u8]) -> ExofsResult<usize> {
        let n = buf.len().min(self.len);
        if n == 0 { return Ok(0); }
        // SAFETY: n ≤ self.len, la mémoire est valide par invariant de struct.
        unsafe {
            core::ptr::copy_nonoverlapping(self.ptr, buf.as_mut_ptr(), n);
        }
        Ok(n)
    }

    /// Écrit `src` dans ce fragment.
    ///
    /// SAFETY: même invariant que `read_into`.
    pub fn write_from(&self, src: &[u8]) -> ExofsResult<usize> {
        let n = src.len().min(self.len);
        if n == 0 { return Ok(0); }
        // SAFETY: n ≤ self.len.
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr, n);
        }
        Ok(n)
    }
}

// ─── SgList ───────────────────────────────────────────────────────────────────

/// Liste de fragments Scatter-Gather.
pub struct SgList {
    fragments: Vec<SgFragment>,
    total_bytes: u64,
}

impl SgList {
    pub fn new() -> Self { Self { fragments: Vec::new(), total_bytes: 0 } }

    /// Ajoute un fragment (OOM-02).
    pub fn add(&mut self, frag: SgFragment) -> ExofsResult<()> {
        self.fragments.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.total_bytes = self.total_bytes.saturating_add(frag.len as u64);
        self.fragments.push(frag);
        Ok(())
    }

    pub fn fragment_count(&self) -> usize { self.fragments.len() }
    pub fn total_bytes(&self) -> u64 { self.total_bytes }
    pub fn is_empty(&self) -> bool { self.fragments.is_empty() }

    /// Gather : lit toutes les données des fragments vers `sink` (RECUR-01 : while).
    ///
    /// SAFETY: chaque fragment doit pointer vers une mémoire valide.
    pub fn gather_read(&self, sink: &mut Vec<u8>) -> ExofsResult<u64> {
        sink.try_reserve(self.total_bytes as usize).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        let mut copied = 0u64;
        while i < self.fragments.len() {
            let frag = &self.fragments[i];
            let start = sink.len();
            // OOM-02: réserve faite au-dessus, len suffisante
            let old_len = sink.len();
            sink.resize(old_len.saturating_add(frag.len), 0u8);
            let n = frag.read_into(&mut sink[old_len..])?;
            sink.truncate(old_len.wrapping_add(n));
            copied = copied.saturating_add(n as u64);
            let _ = start;
            i = i.wrapping_add(1);
        }
        Ok(copied)
    }

    /// Scatter : écrit `src` dans les fragments (RECUR-01 : while).
    ///
    /// SAFETY: chaque fragment doit pointer vers une mémoire valide.
    pub fn scatter_write(&self, src: &[u8]) -> ExofsResult<u64> {
        if src.len() as u64 > self.total_bytes {
            return Err(ExofsError::InvalidArgument);
        }
        let mut i = 0usize;
        let mut offset = 0usize;
        while i < self.fragments.len() && offset < src.len() {
            let frag = &self.fragments[i];
            let left = src.len().saturating_sub(offset);
            let chunk = left.min(frag.len);
            frag.write_from(&src[offset..offset.wrapping_add(chunk)])?;
            offset = offset.wrapping_add(chunk);
            i = i.wrapping_add(1);
        }
        Ok(offset as u64)
    }

    pub fn clear(&mut self) { self.fragments.clear(); self.total_bytes = 0; }
}

// ─── PhysSegment ─────────────────────────────────────────────────────────────

/// Segment DMA physique (bus address).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct PhysSegment {
    pub phys_addr: u64,
    pub len: u32,
    pub flags: u32,
}

impl PhysSegment {
    pub const FLAG_READ:  u32 = 0x01;
    pub const FLAG_WRITE: u32 = 0x02;

    pub fn read_segment(phys_addr: u64, len: u32) -> Self {
        Self { phys_addr, len, flags: Self::FLAG_READ }
    }

    pub fn write_segment(phys_addr: u64, len: u32) -> Self {
        Self { phys_addr, len, flags: Self::FLAG_WRITE }
    }

    pub fn is_read(&self)  -> bool { self.flags & Self::FLAG_READ != 0 }
    pub fn is_write(&self) -> bool { self.flags & Self::FLAG_WRITE != 0 }

    /// Vérifie que ce segment ne déborde pas (ARITH-02).
    pub fn end_addr(&self) -> Option<u64> {
        self.phys_addr.checked_add(self.len as u64)
    }
}

// ─── PhysSgList ──────────────────────────────────────────────────────────────

/// Liste de segments physiques pour DMA.
pub struct PhysSgList {
    segs: Vec<PhysSegment>,
    total_bytes: u64,
}

impl PhysSgList {
    pub fn new() -> Self { Self { segs: Vec::new(), total_bytes: 0 } }

    /// Ajoute un segment (OOM-02).
    pub fn add(&mut self, seg: PhysSegment) -> ExofsResult<()> {
        // Vérifier absence d'overflow sur l'adresse de fin (ARITH-02)
        seg.end_addr().ok_or(ExofsError::OffsetOverflow)?;
        self.segs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.total_bytes = self.total_bytes.saturating_add(seg.len as u64);
        self.segs.push(seg);
        Ok(())
    }

    pub fn segment_count(&self) -> usize { self.segs.len() }
    pub fn total_bytes(&self) -> u64 { self.total_bytes }
    pub fn segment(&self, idx: usize) -> ExofsResult<&PhysSegment> {
        self.segs.get(idx).ok_or(ExofsError::InvalidArgument)
    }

    /// Vérifie l'absence d'overlap entre segments (RECUR-01 : while).
    pub fn validate_no_overlap(&self) -> ExofsResult<()> {
        let n = self.segs.len();
        if n < 2 { return Ok(()); }
        // On fait un check simplifié (O(n²) acceptable pour petites listes DMA)
        let mut i = 0usize;
        while i < n {
            let a = &self.segs[i];
            let a_end = a.end_addr().ok_or(ExofsError::OffsetOverflow)?;
            let mut j = i.wrapping_add(1);
            while j < n {
                let b = &self.segs[j];
                let b_end = b.end_addr().ok_or(ExofsError::OffsetOverflow)?;
                // overlap si a.start < b.end && b.start < a.end
                if a.phys_addr < b_end && b.phys_addr < a_end {
                    return Err(ExofsError::InvalidArgument);
                }
                j = j.wrapping_add(1);
            }
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    pub fn clear(&mut self) { self.segs.clear(); self.total_bytes = 0; }
}

// ─── SgStats ─────────────────────────────────────────────────────────────────

/// Statistiques des opérations Scatter-Gather.
#[derive(Clone, Copy, Debug, Default)]
pub struct SgStats {
    pub gather_ops: u64,
    pub scatter_ops: u64,
    pub bytes_gathered: u64,
    pub bytes_scattered: u64,
    pub errors: u64,
}

impl SgStats {
    pub fn new() -> Self { Self::default() }
    pub fn is_clean(&self) -> bool { self.errors == 0 }
    pub fn total_ops(&self) -> u64 { self.gather_ops.saturating_add(self.scatter_ops) }
}

// ─── SgEngine ─────────────────────────────────────────────────────────────────

/// Moteur Scatter-Gather avec statistiques.
pub struct SgEngine {
    stats: SgStats,
}

impl SgEngine {
    pub fn new() -> Self { Self { stats: SgStats::new() } }

    /// Gather : lit tous les fragments d'une SgList vers un Vec.
    pub fn gather(&mut self, list: &SgList, sink: &mut Vec<u8>) -> ExofsResult<u64> {
        match list.gather_read(sink) {
            Ok(n) => {
                self.stats.gather_ops = self.stats.gather_ops.saturating_add(1);
                self.stats.bytes_gathered = self.stats.bytes_gathered.saturating_add(n);
                Ok(n)
            }
            Err(e) => {
                self.stats.errors = self.stats.errors.saturating_add(1);
                Err(e)
            }
        }
    }

    /// Scatter : écrit `data` dans les fragments d'une SgList.
    pub fn scatter(&mut self, list: &SgList, data: &[u8]) -> ExofsResult<u64> {
        match list.scatter_write(data) {
            Ok(n) => {
                self.stats.scatter_ops = self.stats.scatter_ops.saturating_add(1);
                self.stats.bytes_scattered = self.stats.bytes_scattered.saturating_add(n);
                Ok(n)
            }
            Err(e) => {
                self.stats.errors = self.stats.errors.saturating_add(1);
                Err(e)
            }
        }
    }

    pub fn stats(&self) -> &SgStats { &self.stats }
    pub fn reset_stats(&mut self) { self.stats = SgStats::new(); }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sg_fragment_read() {
        let mut data = [1u8, 2, 3, 4, 5];
        let frag = SgFragment::from_slice(&mut data);
        let mut buf = [0u8; 5];
        let n = frag.read_into(&mut buf).expect("ok");
        assert_eq!(n, 5);
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_sg_fragment_write() {
        let mut data = [0u8; 5];
        let frag = SgFragment::from_slice(&mut data);
        frag.write_from(b"hello").expect("ok");
        assert_eq!(data, *b"hello");
    }

    #[test]
    fn test_sg_list_gather() {
        let mut a = [1u8, 2, 3];
        let mut b = [4u8, 5, 6];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        list.add(SgFragment::from_slice(&mut b)).expect("ok");
        let mut sink = Vec::new();
        let n = list.gather_read(&mut sink).expect("ok");
        assert_eq!(n, 6);
        assert_eq!(sink, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_sg_list_scatter() {
        let mut a = [0u8; 3];
        let mut b = [0u8; 3];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        list.add(SgFragment::from_slice(&mut b)).expect("ok");
        list.scatter_write(&[1, 2, 3, 4, 5, 6]).expect("ok");
        assert_eq!(a, [1, 2, 3]);
        assert_eq!(b, [4, 5, 6]);
    }

    #[test]
    fn test_sg_list_scatter_too_large() {
        let mut a = [0u8; 2];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        assert!(list.scatter_write(b"too long").is_err());
    }

    #[test]
    fn test_phys_segment_end_addr() {
        let seg = PhysSegment::read_segment(0x1000, 0x200);
        assert_eq!(seg.end_addr(), Some(0x1200));
    }

    #[test]
    fn test_phys_sg_list_add() {
        let mut plist = PhysSgList::new();
        plist.add(PhysSegment::read_segment(0x1000, 512)).expect("ok");
        plist.add(PhysSegment::read_segment(0x2000, 512)).expect("ok");
        assert_eq!(plist.segment_count(), 2);
        assert_eq!(plist.total_bytes(), 1024);
    }

    #[test]
    fn test_phys_sg_list_no_overlap() {
        let mut plist = PhysSgList::new();
        plist.add(PhysSegment::read_segment(0x1000, 0x100)).expect("ok");
        plist.add(PhysSegment::read_segment(0x2000, 0x100)).expect("ok");
        assert!(plist.validate_no_overlap().is_ok());
    }

    #[test]
    fn test_phys_sg_list_overlap_detected() {
        let mut plist = PhysSgList::new();
        plist.add(PhysSegment::read_segment(0x1000, 0x200)).expect("ok");
        plist.add(PhysSegment::read_segment(0x1100, 0x100)).expect("ok"); // overlap!
        assert!(plist.validate_no_overlap().is_err());
    }

    #[test]
    fn test_sg_engine_gather_stats() {
        let mut a = [1u8, 2, 3];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        let mut engine = SgEngine::new();
        let mut sink = Vec::new();
        engine.gather(&list, &mut sink).expect("ok");
        assert_eq!(engine.stats().gather_ops, 1);
        assert_eq!(engine.stats().bytes_gathered, 3);
    }

    #[test]
    fn test_sg_engine_scatter_stats() {
        let mut a = [0u8; 3];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        let mut engine = SgEngine::new();
        engine.scatter(&list, &[9, 8, 7]).expect("ok");
        assert_eq!(engine.stats().scatter_ops, 1);
    }

    #[test]
    fn test_sg_engine_reset_stats() {
        let mut a = [0u8; 3];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        let mut engine = SgEngine::new();
        engine.scatter(&list, &[1, 2, 3]).expect("ok");
        engine.reset_stats();
        assert_eq!(engine.stats().scatter_ops, 0);
    }

    #[test]
    fn test_sg_list_total_bytes() {
        let mut a = [0u8; 10];
        let mut b = [0u8; 20];
        let mut list = SgList::new();
        list.add(SgFragment::from_slice(&mut a)).expect("ok");
        list.add(SgFragment::from_slice(&mut b)).expect("ok");
        assert_eq!(list.total_bytes(), 30);
    }

    #[test]
    fn test_const_slice_fragment() {
        let data = b"const source";
        let frag = SgFragment::from_const_slice(data);
        let mut buf = [0u8; 12];
        frag.read_into(&mut buf).expect("ok");
        assert_eq!(&buf, b"const source");
    }
}
