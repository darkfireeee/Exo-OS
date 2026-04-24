//! buffered_io.rs — Couche tampon (ring buffer + lecteur/écrivain bufférisé) (no_std).
//!
//! Ce module fournit :
//!  - `RingBuffer`       : anneau d'octets FIFO avec indices wrapping.
//!  - `BufferedReader`   : lecture bufférisée depuis une source byte-slice.
//!  - `BufferedWriter`   : écriture bufférisée avec flush vers un sink.
//!  - `IoBuffer`         : buffer positionnel (read_at / write_at).
//!  - `ByteSource` / `ByteSink` : traits d'abstraction source/sink.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add/mul.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── Traits source / sink ─────────────────────────────────────────────────────

/// Source d'octets (lecture séquentielle).
pub trait ByteSource {
    /// Lit au plus `buf.len()` octets. Retourne le nombre d'octets lus.
    fn read(&mut self, buf: &mut [u8]) -> ExofsResult<usize>;
    fn is_eof(&self) -> bool;
}

/// Sink d'octets (écriture séquentielle).
pub trait ByteSink {
    fn write(&mut self, buf: &[u8]) -> ExofsResult<usize>;
    fn flush(&mut self) -> ExofsResult<()>;
}

// ─── SliceSource — implémentation de ByteSource sur slice ────────────────────

/// Source d'octets sur un slice (pour tests).
pub struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

impl<'a> ByteSource for SliceSource<'a> {
    fn read(&mut self, buf: &mut [u8]) -> ExofsResult<usize> {
        let avail = self.data.len().saturating_sub(self.pos);
        if avail == 0 {
            return Ok(0);
        }
        let n = buf.len().min(avail);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos.wrapping_add(n)]);
        self.pos = self.pos.wrapping_add(n);
        Ok(n)
    }
    fn is_eof(&self) -> bool {
        self.pos >= self.data.len()
    }
}

// ─── VecSink — implémentation de ByteSink sur Vec ─────────────────────────────

/// Sink d'octets sur un Vec (pour tests).
pub struct VecSink {
    data: Vec<u8>,
    flush_count: u32,
}

impl VecSink {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            flush_count: 0,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
    pub fn flush_count(&self) -> u32 {
        self.flush_count
    }
}

impl ByteSink for VecSink {
    fn write(&mut self, buf: &[u8]) -> ExofsResult<usize> {
        self.data
            .try_reserve(buf.len())
            .map_err(|_| ExofsError::NoMemory)?;
        self.data.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> ExofsResult<()> {
        self.flush_count = self.flush_count.saturating_add(1);
        Ok(())
    }
}

// ─── RingBuffer ───────────────────────────────────────────────────────────────

/// Anneau FIFO d'octets de capacité fixe.
///
/// Indices `head` (lecture) et `tail` (écriture) en mode wrapping.
pub struct RingBuffer {
    buf: Vec<u8>,
    cap: usize,
    head: usize,
    tail: usize,
    len: usize,
}

impl RingBuffer {
    /// Crée un anneau de `capacity` octets (OOM-02).
    pub fn new(capacity: usize) -> ExofsResult<Self> {
        if capacity == 0 {
            return Err(ExofsError::InvalidArgument);
        }
        let mut buf = Vec::new();
        buf.try_reserve(capacity)
            .map_err(|_| ExofsError::NoMemory)?;
        buf.resize(capacity, 0u8);
        Ok(Self {
            buf,
            cap: capacity,
            head: 0,
            tail: 0,
            len: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn free(&self) -> usize {
        self.cap.saturating_sub(self.len)
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn is_full(&self) -> bool {
        self.len >= self.cap
    }

    /// Écrit des octets dans l'anneau (ARITH-02).
    pub fn push_slice(&mut self, data: &[u8]) -> ExofsResult<usize> {
        let n = data.len().min(self.free());
        let mut i = 0usize;
        while i < n {
            self.buf[self.tail] = data[i];
            self.tail = self.tail.wrapping_add(1);
            if self.tail >= self.cap {
                self.tail = 0;
            }
            self.len = self.len.saturating_add(1);
            i = i.wrapping_add(1);
        }
        Ok(n)
    }

    /// Lit des octets depuis l'anneau (ARITH-02).
    pub fn pop_slice(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        let mut i = 0usize;
        while i < n {
            out[i] = self.buf[self.head];
            self.head = self.head.wrapping_add(1);
            if self.head >= self.cap {
                self.head = 0;
            }
            self.len = self.len.saturating_sub(1);
            i = i.wrapping_add(1);
        }
        n
    }

    /// Vide l'anneau sans libérer la mémoire.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

// ─── BufferedReader ───────────────────────────────────────────────────────────

/// Lecteur bufférisé depuis une source `ByteSource`.
pub struct BufferedReader {
    ring: RingBuffer,
    bytes_produced: u64,
    fill_calls: u32,
}

impl BufferedReader {
    pub fn new(ring_capacity: usize) -> ExofsResult<Self> {
        Ok(Self {
            ring: RingBuffer::new(ring_capacity)?,
            bytes_produced: 0,
            fill_calls: 0,
        })
    }

    /// Remplit le ring depuis la source (RECUR-01 : while).
    pub fn fill<S: ByteSource>(&mut self, source: &mut S) -> ExofsResult<usize> {
        if self.ring.is_full() {
            return Ok(0);
        }
        let mut temp = [0u8; 512];
        let mut total = 0usize;
        while !self.ring.is_full() && !source.is_eof() {
            let to_read = temp.len().min(self.ring.free());
            let n = source.read(&mut temp[..to_read])?;
            if n == 0 {
                break;
            }
            let pushed = self.ring.push_slice(&temp[..n])?;
            total = total.saturating_add(pushed);
        }
        self.fill_calls = self.fill_calls.saturating_add(1);
        Ok(total)
    }

    /// Lit depuis le ring (RECUR-01 : while).
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let n = self.ring.pop_slice(out);
        self.bytes_produced = self.bytes_produced.saturating_add(n as u64);
        n
    }

    /// Peek : lit sans consommer (copie dans buf, RECUR-01 : while).
    pub fn peek(&self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.ring.len());
        let mut i = 0usize;
        let mut h = self.ring.head;
        while i < n {
            out[i] = self.ring.buf[h];
            h = h.wrapping_add(1);
            if h >= self.ring.cap {
                h = 0;
            }
            i = i.wrapping_add(1);
        }
        n
    }

    pub fn available(&self) -> usize {
        self.ring.len()
    }
    pub fn bytes_produced(&self) -> u64 {
        self.bytes_produced
    }
    pub fn fill_calls(&self) -> u32 {
        self.fill_calls
    }
}

// ─── BufferedWriter ───────────────────────────────────────────────────────────

/// Écrivain bufférisé vers un sink `ByteSink`.
pub struct BufferedWriter {
    ring: RingBuffer,
    bytes_flushed: u64,
    flush_calls: u32,
}

impl BufferedWriter {
    pub fn new(ring_capacity: usize) -> ExofsResult<Self> {
        Ok(Self {
            ring: RingBuffer::new(ring_capacity)?,
            bytes_flushed: 0,
            flush_calls: 0,
        })
    }

    /// Écrit dans le ring (RECUR-01 : while).
    pub fn write(&mut self, data: &[u8]) -> ExofsResult<usize> {
        self.ring.push_slice(data)
    }

    /// Flush le ring vers le sink (RECUR-01 : while).
    pub fn flush<S: ByteSink>(&mut self, sink: &mut S) -> ExofsResult<u64> {
        let mut temp = [0u8; 512];
        let mut total = 0u64;
        while !self.ring.is_empty() {
            let n = self.ring.pop_slice(&mut temp);
            if n == 0 {
                break;
            }
            sink.write(&temp[..n])?;
            total = total.saturating_add(n as u64);
        }
        sink.flush()?;
        self.bytes_flushed = self.bytes_flushed.saturating_add(total);
        self.flush_calls = self.flush_calls.saturating_add(1);
        Ok(total)
    }

    pub fn pending_bytes(&self) -> usize {
        self.ring.len()
    }
    pub fn bytes_flushed(&self) -> u64 {
        self.bytes_flushed
    }
    pub fn flush_calls(&self) -> u32 {
        self.flush_calls
    }
}

// ─── IoBuffer : buffer positionnel ───────────────────────────────────────────

/// Buffer IO positionnel (accès read_at / write_at).
pub struct IoBuffer {
    data: Vec<u8>,
    capacity: usize,
}

impl IoBuffer {
    /// Crée un IoBuffer de capacité `cap` octets (OOM-02).
    pub fn new(cap: usize) -> ExofsResult<Self> {
        let mut data = Vec::new();
        data.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        data.resize(cap, 0u8);
        Ok(Self {
            data,
            capacity: cap,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Écrit `src` à l'offset `off` (ARITH-02).
    pub fn write_at(&mut self, off: usize, src: &[u8]) -> ExofsResult<usize> {
        let end = off
            .checked_add(src.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if end > self.capacity {
            return Err(ExofsError::OffsetOverflow);
        }
        self.data[off..end].copy_from_slice(src);
        Ok(src.len())
    }

    /// Lit `buf.len()` octets à l'offset `off` (ARITH-02).
    pub fn read_at(&self, off: usize, buf: &mut [u8]) -> ExofsResult<usize> {
        let end = off
            .checked_add(buf.len())
            .ok_or(ExofsError::OffsetOverflow)?;
        if end > self.capacity {
            return Err(ExofsError::OffsetOverflow);
        }
        buf.copy_from_slice(&self.data[off..end]);
        Ok(buf.len())
    }

    /// Remplit tout le buffer avec `byte`.
    pub fn fill(&mut self, byte: u8) {
        let mut i = 0usize;
        while i < self.capacity {
            self.data[i] = byte;
            i = i.wrapping_add(1);
        }
    }

    /// Compare deux IoBuffer (RECUR-01 : while).
    pub fn equals(&self, other: &Self) -> bool {
        if self.capacity != other.capacity {
            return false;
        }
        let mut i = 0usize;
        while i < self.capacity {
            if self.data[i] != other.data[i] {
                return false;
            }
            i = i.wrapping_add(1);
        }
        true
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_push_pop() {
        let mut ring = RingBuffer::new(16).expect("ok");
        assert_eq!(ring.push_slice(b"hello").expect("ok"), 5);
        let mut out = [0u8; 5];
        assert_eq!(ring.pop_slice(&mut out), 5);
        assert_eq!(&out, b"hello");
    }

    #[test]
    fn test_ring_overflow() {
        let mut ring = RingBuffer::new(4).expect("ok");
        assert_eq!(ring.push_slice(b"abcde").expect("ok"), 4); // tronqué
        assert!(ring.is_full());
    }

    #[test]
    fn test_ring_wrap_around() {
        let mut ring = RingBuffer::new(4).expect("ok");
        ring.push_slice(b"ab").expect("ok");
        let mut out = [0u8; 2];
        ring.pop_slice(&mut out);
        ring.push_slice(b"cd").expect("ok");
        ring.pop_slice(&mut out);
        assert_eq!(&out, b"cd");
    }

    #[test]
    fn test_ring_clear() {
        let mut ring = RingBuffer::new(8).expect("ok");
        ring.push_slice(b"data").expect("ok");
        ring.clear();
        assert!(ring.is_empty());
    }

    #[test]
    fn test_buffered_reader_fill_read() {
        let mut src = SliceSource::new(b"buffered read test");
        let mut reader = BufferedReader::new(64).expect("ok");
        reader.fill(&mut src).expect("ok");
        assert_eq!(reader.available(), 18);
        let mut buf = [0u8; 18];
        assert_eq!(reader.read(&mut buf), 18);
        assert_eq!(&buf, b"buffered read test");
    }

    #[test]
    fn test_buffered_reader_partial() {
        let mut src = SliceSource::new(b"partial");
        let mut reader = BufferedReader::new(4).expect("ok");
        reader.fill(&mut src).expect("ok");
        let mut out = [0u8; 4];
        reader.read(&mut out);
        assert_eq!(&out, b"part");
    }

    #[test]
    fn test_buffered_writer_flush() {
        let mut sink = VecSink::new();
        let mut writer = BufferedWriter::new(32).expect("ok");
        writer.write(b"write test").expect("ok");
        let flushed = writer.flush(&mut sink).expect("ok");
        assert_eq!(flushed, 10);
        assert_eq!(sink.as_slice(), b"write test");
    }

    #[test]
    fn test_buffered_writer_multi_flush() {
        let mut sink = VecSink::new();
        let mut writer = BufferedWriter::new(32).expect("ok");
        writer.write(b"abc").expect("ok");
        writer.flush(&mut sink).expect("ok");
        writer.write(b"def").expect("ok");
        writer.flush(&mut sink).expect("ok");
        assert_eq!(sink.as_slice(), b"abcdef");
        assert_eq!(writer.flush_calls(), 2);
    }

    #[test]
    fn test_io_buffer_write_read() {
        let mut buf = IoBuffer::new(32).expect("ok");
        buf.write_at(0, b"test data").expect("ok");
        let mut out = [0u8; 9];
        buf.read_at(0, &mut out).expect("ok");
        assert_eq!(&out, b"test data");
    }

    #[test]
    fn test_io_buffer_offset_overflow() {
        let buf = IoBuffer::new(4).expect("ok");
        let mut out = [0u8; 8];
        assert!(buf.read_at(0, &mut out).is_err());
    }

    #[test]
    fn test_io_buffer_fill() {
        let mut buf = IoBuffer::new(8).expect("ok");
        buf.fill(0xAB);
        let mut out = [0u8; 8];
        buf.read_at(0, &mut out).expect("ok");
        let mut i = 0;
        while i < 8 {
            assert_eq!(out[i], 0xAB);
            i += 1;
        }
    }

    #[test]
    fn test_io_buffer_equals() {
        let mut a = IoBuffer::new(4).expect("ok");
        let mut b = IoBuffer::new(4).expect("ok");
        a.fill(1);
        b.fill(1);
        assert!(a.equals(&b));
        b.fill(2);
        assert!(!a.equals(&b));
    }

    #[test]
    fn test_slice_source_eof() {
        let mut src = SliceSource::new(b"hi");
        let mut buf = [0u8; 2];
        src.read(&mut buf).expect("ok");
        assert!(src.is_eof());
    }

    #[test]
    fn test_peek_does_not_consume() {
        let mut src = SliceSource::new(b"peek test");
        let mut reader = BufferedReader::new(32).expect("ok");
        reader.fill(&mut src).expect("ok");
        let mut out1 = [0u8; 4];
        let mut out2 = [0u8; 4];
        reader.peek(&mut out1);
        reader.peek(&mut out2);
        assert_eq!(out1, out2);
        assert_eq!(reader.available(), 9); // pas consumé
    }
}
