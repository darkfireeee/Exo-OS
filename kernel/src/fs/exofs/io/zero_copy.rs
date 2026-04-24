//! zero_copy.rs — Lecture/écriture zéro-copie sur tranches mémoire (no_std).
//!
//! Fournit :
//!  - `ZeroCopySlice<'a>`  : vue empruntée et positionnée sur une région mémoire.
//!  - `ZeroCopyWindow`     : sous-vue sans copie.
//!  - `ZeroCopyReader`     : lecteur séquentiel.
//!  - `ZeroCopyWriter`     : écrivain séquentiel sur tranche mutable.
//!  - `ZeroCopyPipe`       : chaîne logique de deux tranches de lecture.
//!  - `ZeroCopyStats`      : compteurs d'opérations.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── ZeroCopySlice ────────────────────────────────────────────────────────────

/// Vue immuable et positionnée sur une tranche `&[u8]`.
pub struct ZeroCopySlice<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ZeroCopySlice<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn at_offset(data: &'a [u8], offset: usize) -> ExofsResult<Self> {
        if offset > data.len() {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(Self { data, offset })
    }

    pub fn position(&self) -> usize {
        self.offset
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }
    pub fn total_len(&self) -> usize {
        self.data.len()
    }
    pub fn is_exhausted(&self) -> bool {
        self.offset >= self.data.len()
    }

    pub fn peek_slice(&self, n: usize) -> ExofsResult<&'a [u8]> {
        let end = self.offset.saturating_add(n);
        if end > self.data.len() {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(&self.data[self.offset..end])
    }

    /// Copie jusqu'à `dst.len()` bytes, avance le curseur.
    pub fn read_into(&mut self, dst: &mut [u8]) -> ExofsResult<usize> {
        let n = dst.len();
        let to_copy = n.min(self.remaining());
        if to_copy == 0 {
            return Ok(0);
        }
        let src = &self.data[self.offset..self.offset.saturating_add(to_copy)];
        // RECUR-01 : while
        let mut i = 0usize;
        while i < to_copy {
            dst[i] = src[i];
            i = i.wrapping_add(1);
        }
        self.offset = self.offset.saturating_add(to_copy);
        Ok(to_copy)
    }

    pub fn advance(&mut self, n: usize) -> ExofsResult<()> {
        let new_off = self.offset.saturating_add(n);
        if new_off > self.data.len() {
            return Err(ExofsError::OffsetOverflow);
        }
        self.offset = new_off;
        Ok(())
    }

    pub fn as_remaining_slice(&self) -> &'a [u8] {
        if self.offset >= self.data.len() {
            return &[];
        }
        &self.data[self.offset..]
    }

    /// Copie l'intégralité du contenu restant dans un `Vec` (OOM-02).
    pub fn drain_to_vec(&mut self) -> ExofsResult<Vec<u8>> {
        let rem = self.remaining();
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve(rem).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < rem {
            v.push(self.data[self.offset.wrapping_add(i)]);
            i = i.wrapping_add(1);
        }
        self.offset = self.offset.saturating_add(rem);
        Ok(v)
    }
}

// ─── ZeroCopyWindow ───────────────────────────────────────────────────────────

/// Sous-vue (fenêtre) sur une `ZeroCopySlice` — aucune copie.
pub struct ZeroCopyWindow<'a> {
    data: &'a [u8],
    start: usize,
    end: usize,
}

impl<'a> ZeroCopyWindow<'a> {
    pub fn new(slice: &ZeroCopySlice<'a>, start: usize, len: usize) -> ExofsResult<Self> {
        let end = start.saturating_add(len);
        if end > slice.data.len() {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(Self {
            data: slice.data,
            start,
            end,
        })
    }

    pub fn as_slice(&self) -> &'a [u8] {
        &self.data[self.start..self.end]
    }
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read_at(&self, off: usize, n: usize) -> ExofsResult<&'a [u8]> {
        let abs_start = self.start.saturating_add(off);
        let abs_end = abs_start.saturating_add(n);
        if abs_end > self.end {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(&self.data[abs_start..abs_end])
    }

    pub fn sub_window(&self, off: usize, len: usize) -> ExofsResult<ZeroCopyWindow<'a>> {
        let start = self.start.saturating_add(off);
        let end = start.saturating_add(len);
        if end > self.end {
            return Err(ExofsError::OffsetOverflow);
        }
        Ok(ZeroCopyWindow {
            data: self.data,
            start,
            end,
        })
    }

    /// Copie la fenêtre dans un Vec (OOM-02).
    pub fn to_vec(&self) -> ExofsResult<Vec<u8>> {
        let len = self.len();
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve(len).map_err(|_| ExofsError::NoMemory)?;
        let mut i = 0usize;
        while i < len {
            v.push(self.data[self.start.wrapping_add(i)]);
            i = i.wrapping_add(1);
        }
        Ok(v)
    }
}

// ─── ZeroCopyReader ───────────────────────────────────────────────────────────

/// Lecteur séquentiel sur une `ZeroCopySlice`.
pub struct ZeroCopyReader<'a> {
    slice: ZeroCopySlice<'a>,
    stats: ZeroCopyStats,
}

impl<'a> ZeroCopyReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            slice: ZeroCopySlice::new(data),
            stats: ZeroCopyStats::new(),
        }
    }

    /// Lit exactement `dst.len()` bytes.
    pub fn read_exact(&mut self, dst: &mut [u8]) -> ExofsResult<()> {
        if self.slice.remaining() < dst.len() {
            return Err(ExofsError::IoError);
        }
        let n = self.slice.read_into(dst)?;
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(n as u64);
        self.stats.read_ops = self.stats.read_ops.saturating_add(1);
        Ok(())
    }

    /// Lit au plus `dst.len()` bytes.
    pub fn read_partial(&mut self, dst: &mut [u8]) -> ExofsResult<usize> {
        let n = self.slice.read_into(dst)?;
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(n as u64);
        self.stats.read_ops = self.stats.read_ops.saturating_add(1);
        Ok(n)
    }

    pub fn skip(&mut self, n: usize) -> ExofsResult<()> {
        self.slice.advance(n)
    }
    pub fn position(&self) -> usize {
        self.slice.position()
    }
    pub fn remaining(&self) -> usize {
        self.slice.remaining()
    }
    pub fn stats(&self) -> &ZeroCopyStats {
        &self.stats
    }
    pub fn reset_stats(&mut self) {
        self.stats = ZeroCopyStats::new();
    }
    pub fn peek(&self, n: usize) -> ExofsResult<&'a [u8]> {
        self.slice.peek_slice(n)
    }

    /// Lit une valeur u8.
    pub fn read_u8(&mut self) -> ExofsResult<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf)?;
        Ok(buf[0])
    }

    /// Lit une valeur u16 big-endian.
    pub fn read_u16_be(&mut self) -> ExofsResult<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf))
    }

    /// Lit une valeur u32 big-endian.
    pub fn read_u32_be(&mut self) -> ExofsResult<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }

    /// Lit une valeur u64 big-endian.
    pub fn read_u64_be(&mut self) -> ExofsResult<u64> {
        let mut buf = [0u8; 8];
        self.read_exact(&mut buf)?;
        Ok(u64::from_be_bytes(buf))
    }
}

// ─── ZeroCopyWriter ───────────────────────────────────────────────────────────

/// Écrivain séquentiel sur une tranche mutable (aucune allocation).
pub struct ZeroCopyWriter<'a> {
    buf: &'a mut [u8],
    offset: usize,
    stats: ZeroCopyStats,
}

impl<'a> ZeroCopyWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            offset: 0,
            stats: ZeroCopyStats::new(),
        }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.offset)
    }
    pub fn position(&self) -> usize {
        self.offset
    }

    pub fn write_exact(&mut self, src: &[u8]) -> ExofsResult<()> {
        let n = src.len();
        if self.remaining() < n {
            return Err(ExofsError::NoSpace);
        }
        let start = self.offset;
        // RECUR-01 : while
        let mut i = 0usize;
        while i < n {
            self.buf[start.wrapping_add(i)] = src[i];
            i = i.wrapping_add(1);
        }
        self.offset = self.offset.saturating_add(n);
        self.stats.bytes_written = self.stats.bytes_written.saturating_add(n as u64);
        self.stats.write_ops = self.stats.write_ops.saturating_add(1);
        Ok(())
    }

    pub fn write_zeros(&mut self, n: usize) -> ExofsResult<()> {
        if self.remaining() < n {
            return Err(ExofsError::NoSpace);
        }
        let start = self.offset;
        let mut i = 0usize;
        while i < n {
            self.buf[start.wrapping_add(i)] = 0u8;
            i = i.wrapping_add(1);
        }
        self.offset = self.offset.saturating_add(n);
        self.stats.bytes_written = self.stats.bytes_written.saturating_add(n as u64);
        Ok(())
    }

    pub fn write_u8(&mut self, v: u8) -> ExofsResult<()> {
        self.write_exact(&[v])
    }

    pub fn write_u16_be(&mut self, v: u16) -> ExofsResult<()> {
        self.write_exact(&v.to_be_bytes())
    }

    pub fn write_u32_be(&mut self, v: u32) -> ExofsResult<()> {
        self.write_exact(&v.to_be_bytes())
    }

    pub fn write_u64_be(&mut self, v: u64) -> ExofsResult<()> {
        self.write_exact(&v.to_be_bytes())
    }

    pub fn written_slice(&self) -> &[u8] {
        &self.buf[..self.offset]
    }
    pub fn stats(&self) -> &ZeroCopyStats {
        &self.stats
    }
}

// ─── ZeroCopyPipe ─────────────────────────────────────────────────────────────

/// Chaîne logique de deux tranches de lecture (sans allocation).
pub struct ZeroCopyPipe<'a> {
    first: ZeroCopySlice<'a>,
    second: ZeroCopySlice<'a>,
    stats: ZeroCopyStats,
}

impl<'a> ZeroCopyPipe<'a> {
    pub fn new(first: &'a [u8], second: &'a [u8]) -> Self {
        Self {
            first: ZeroCopySlice::new(first),
            second: ZeroCopySlice::new(second),
            stats: ZeroCopyStats::new(),
        }
    }

    pub fn remaining(&self) -> usize {
        self.first
            .remaining()
            .saturating_add(self.second.remaining())
    }

    /// Lit jusqu'à `dst.len()` bytes en consommant first puis second.
    pub fn read(&mut self, dst: &mut [u8]) -> ExofsResult<usize> {
        let mut total = 0usize;
        if !self.first.is_exhausted() {
            let n = self.first.read_into(&mut dst[total..])?;
            total = total.saturating_add(n);
        }
        if total < dst.len() && !self.second.is_exhausted() {
            let n = self.second.read_into(&mut dst[total..])?;
            total = total.saturating_add(n);
        }
        self.stats.bytes_read = self.stats.bytes_read.saturating_add(total as u64);
        self.stats.read_ops = self.stats.read_ops.saturating_add(1);
        Ok(total)
    }

    pub fn stats(&self) -> &ZeroCopyStats {
        &self.stats
    }
}

// ─── ZeroCopyStats ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct ZeroCopyStats {
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

impl ZeroCopyStats {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Ratio read/(read+write) * 100 (ARITH-02: checked_div).
    pub fn read_ratio_pct(&self) -> u64 {
        let total = self.read_ops.saturating_add(self.write_ops);
        self.read_ops
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_basic() {
        let data = [1u8, 2, 3, 4, 5];
        let s = ZeroCopySlice::new(&data);
        assert_eq!(s.total_len(), 5);
        assert_eq!(s.remaining(), 5);
        assert!(!s.is_exhausted());
    }

    #[test]
    fn test_slice_read_into() {
        let data = [10u8, 20, 30];
        let mut s = ZeroCopySlice::new(&data);
        let mut buf = [0u8; 2];
        let n = s.read_into(&mut buf).expect("ok");
        assert_eq!(n, 2);
        assert_eq!(&buf, &[10u8, 20]);
        assert_eq!(s.remaining(), 1);
    }

    #[test]
    fn test_slice_advance_overflow() {
        let data = [0u8; 4];
        let mut s = ZeroCopySlice::new(&data);
        assert!(s.advance(10).is_err());
    }

    #[test]
    fn test_slice_drain_to_vec() {
        let data = [1u8, 2, 3];
        let mut s = ZeroCopySlice::new(&data);
        let v = s.drain_to_vec().expect("ok");
        assert_eq!(&v[..], &[1u8, 2, 3]);
        assert!(s.is_exhausted());
    }

    #[test]
    fn test_window_as_slice() {
        let data = [0u8, 1, 2, 3, 4];
        let s = ZeroCopySlice::new(&data);
        let w = ZeroCopyWindow::new(&s, 1, 3).expect("ok");
        assert_eq!(w.as_slice(), &[1u8, 2, 3]);
    }

    #[test]
    fn test_window_sub_window() {
        let data: Vec<u8> = (0u8..20).collect();
        let s = ZeroCopySlice::new(&data);
        let w = ZeroCopyWindow::new(&s, 5, 10).expect("ok");
        let sw = w.sub_window(2, 3).expect("ok");
        assert_eq!(sw.as_slice(), &[7u8, 8, 9]);
    }

    #[test]
    fn test_window_to_vec() {
        let data = [100u8, 101, 102, 103];
        let s = ZeroCopySlice::new(&data);
        let w = ZeroCopyWindow::new(&s, 1, 2).expect("ok");
        let v = w.to_vec().expect("ok");
        assert_eq!(&v[..], &[101u8, 102]);
    }

    #[test]
    fn test_reader_read_u32_be() {
        let data = 0xDEADBEEFu32.to_be_bytes();
        let mut r = ZeroCopyReader::new(&data);
        let v = r.read_u32_be().expect("ok");
        assert_eq!(v, 0xDEADBEEF);
    }

    #[test]
    fn test_reader_read_u64_be() {
        let data = 0x0102030405060708u64.to_be_bytes();
        let mut r = ZeroCopyReader::new(&data);
        let v = r.read_u64_be().expect("ok");
        assert_eq!(v, 0x0102030405060708);
    }

    #[test]
    fn test_writer_write_u32_read_back() {
        let mut buf = [0u8; 8];
        let mut w = ZeroCopyWriter::new(&mut buf);
        w.write_u32_be(0xCAFEBABE).expect("ok");
        assert_eq!(
            u32::from_be_bytes(buf[..4].try_into().expect("4")),
            0xCAFEBABE
        );
    }

    #[test]
    fn test_writer_zeros() {
        let mut buf = [0xffu8; 8];
        let mut w = ZeroCopyWriter::new(&mut buf);
        w.write_zeros(4).expect("ok");
        assert_eq!(&buf[..4], &[0u8; 4]);
    }

    #[test]
    fn test_writer_overflow() {
        let mut buf = [0u8; 2];
        let mut w = ZeroCopyWriter::new(&mut buf);
        assert!(w.write_exact(&[0u8; 8]).is_err());
    }

    #[test]
    fn test_pipe_full_read() {
        let a = [1u8, 2, 3];
        let b = [4u8, 5, 6];
        let mut pipe = ZeroCopyPipe::new(&a, &b);
        let mut out = [0u8; 6];
        let n = pipe.read(&mut out).expect("ok");
        assert_eq!(n, 6);
        assert_eq!(&out, &[1u8, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_stats_ratio() {
        let mut s = ZeroCopyStats::new();
        s.read_ops = 3;
        s.write_ops = 1;
        assert_eq!(s.read_ratio_pct(), 75);
    }
}
