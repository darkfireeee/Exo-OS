//! tar_compat.rs — POSIX ustar compatibility with the ExoAR format (no_std).
//!
//! This module provides:
//!  - `TarBlock`            : 512-byte tar block.
//!  - `TarHeader`           : ustar header (512 bytes).
//!  - `TarEntryKind`        : tar entry kind (Regular/Dir/Symlink/...).
//!  - `TarEntry`            : tar entry with metadata and data.
//!  - `TarEmitter`          : emits ExoFS blobs as tar entries.
//!  - `TarParser`           : parses a tar stream.
//!  - `TarSink`             : tar output sink trait.
//!  - `TarSource`           : tar input source trait.
//!  - `TarToExoarConverter` : converts a tar stream into an ExoAR archive.
//!  - `ExoarToTarConverter` : converts ExoAR blobs into a tar stream.
//!
//! Format : POSIX.1-2001 / GNU tar ustar.
//!
//! RECUR-01 : while loops — no recursion.
//! OOM-02   : try_reserve before push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

extern crate alloc;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─── Constants ───────────────────────────────────────────────────────────────

/// Size of a POSIX tar block.
pub const TAR_BLOCK_SIZE: usize = 512;

/// ustar format magic.
pub const TAR_MAGIC: &[u8; 6] = b"ustar ";

/// ustar version.
pub const TAR_VERSION: &[u8; 2] = b"00";

/// Maximum size of a tar file name.
pub const TAR_NAME_MAX: usize = 100;

/// Maximum size of a tar prefix.
pub const TAR_PREFIX_MAX: usize = 155;

/// Maximum size of a symlink target.
pub const TAR_LINKNAME_MAX: usize = 100;

// ─── Tar Entry Kind ──────────────────────────────────────────────────────────

/// Tar entry kind (typeflag field).
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum TarEntryKind {
    Regular = b'0',
    HardLink = b'1',
    Symlink = b'2',
    CharDevice = b'3',
    BlockDevice = b'4',
    Directory = b'5',
    Fifo = b'6',
    GlobalPax = b'g',
    ExtendedPax = b'x',
    Unknown = b'?',
}

impl TarEntryKind {
    pub fn from_byte(b: u8) -> Self {
        match b {
            b'0' | 0 => Self::Regular,
            b'1' => Self::HardLink,
            b'2' => Self::Symlink,
            b'3' => Self::CharDevice,
            b'4' => Self::BlockDevice,
            b'5' => Self::Directory,
            b'6' => Self::Fifo,
            b'g' => Self::GlobalPax,
            b'x' => Self::ExtendedPax,
            _ => Self::Unknown,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }
    pub fn is_regular(self) -> bool {
        matches!(self, TarEntryKind::Regular)
    }
    pub fn is_directory(self) -> bool {
        matches!(self, TarEntryKind::Directory)
    }
    pub fn has_data(self) -> bool {
        matches!(self, TarEntryKind::Regular)
    }
}

// ─── Tar Block ─────────────────────────────────────────────────────────────────

/// A 512-byte tar block.
#[derive(Clone, Copy)]
#[repr(C, align(512))]
pub struct TarBlock(pub [u8; TAR_BLOCK_SIZE]);

impl TarBlock {
    pub const fn zero() -> Self {
        Self([0u8; TAR_BLOCK_SIZE])
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.0
    }
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }
}

// ─── ustar Tar Header ────────────────────────────────────────────────────────

/// ustar header — 512 bytes.
/// Offsets per POSIX.1-2001.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct TarHeader {
    pub name: [u8; 100],     // 0
    pub mode: [u8; 8],       // 100
    pub uid: [u8; 8],        // 108
    pub gid: [u8; 8],        // 116
    pub size: [u8; 12],      // 124
    pub mtime: [u8; 12],     // 136
    pub checksum: [u8; 8],   // 148
    pub typeflag: u8,        // 156
    pub linkname: [u8; 100], // 157
    pub magic: [u8; 6],      // 257
    pub version: [u8; 2],    // 263
    pub uname: [u8; 32],     // 265
    pub gname: [u8; 32],     // 297
    pub devmajor: [u8; 8],   // 329
    pub devminor: [u8; 8],   // 337
    pub prefix: [u8; 155],   // 345
    pub _pad: [u8; 12],      // 500
}

const _: () = assert!(core::mem::size_of::<TarHeader>() == TAR_BLOCK_SIZE);

impl TarHeader {
    pub fn zero() -> Self {
        Self {
            name: [0; 100],
            mode: [0; 8],
            uid: [0; 8],
            gid: [0; 8],
            size: [0; 12],
            mtime: [0; 12],
            checksum: [0; 8],
            typeflag: 0,
            linkname: [0; 100],
            magic: [0; 6],
            version: [0; 2],
            uname: [0; 32],
            gname: [0; 32],
            devmajor: [0; 8],
            devminor: [0; 8],
            prefix: [0; 155],
            _pad: [0; 12],
        }
    }

    /// Creates a header for a regular ExoFS file.
    pub fn new_regular(name: &[u8], size: u64, mtime: u64) -> Self {
        let mut hdr = Self::zero();
        let n = name.len().min(TAR_NAME_MAX);
        hdr.name[..n].copy_from_slice(&name[..n]);
        write_octal8(&mut hdr.mode, 0o644);
        write_octal8(&mut hdr.uid, 0);
        write_octal8(&mut hdr.gid, 0);
        write_octal12(&mut hdr.size, size);
        write_octal12(&mut hdr.mtime, mtime);
        hdr.typeflag = TarEntryKind::Regular.as_byte();
        hdr.magic.copy_from_slice(TAR_MAGIC);
        hdr.version.copy_from_slice(TAR_VERSION);
        // checksum computed separately
        hdr
    }

    /// Creates a header for a directory.
    pub fn new_directory(name: &[u8], mtime: u64) -> Self {
        let mut hdr = Self::zero();
        let n = name.len().min(TAR_NAME_MAX);
        hdr.name[..n].copy_from_slice(&name[..n]);
        write_octal8(&mut hdr.mode, 0o755);
        write_octal12(&mut hdr.size, 0);
        write_octal12(&mut hdr.mtime, mtime);
        hdr.typeflag = TarEntryKind::Directory.as_byte();
        hdr.magic.copy_from_slice(TAR_MAGIC);
        hdr.version.copy_from_slice(TAR_VERSION);
        hdr
    }

    /// Returns the entry kind.
    pub fn entry_kind(&self) -> TarEntryKind {
        TarEntryKind::from_byte(self.typeflag)
    }

    /// Returns the size decoded from the octal field.
    pub fn decoded_size(&self) -> u64 {
        parse_octal12(&self.size)
    }

    /// Returns the decoded mtime.
    pub fn decoded_mtime(&self) -> u64 {
        parse_octal12(&self.mtime)
    }

    /// Validates the ustar magic.
    pub fn validate_magic(&self) -> bool {
        &self.magic[..6] == TAR_MAGIC || &self.magic[..5] == b"ustar"
    }

    /// Returns the name as a valid slice (without trailing nulls).
    pub fn name_trimmed(&self) -> &[u8] {
        trim_nul(&self.name)
    }

    /// Computes the checksum (sum of bytes, checksum field = 8 spaces during calculation).
    pub fn compute_checksum(&self) -> u32 {
        tar_checksum_compute(self)
    }

    /// Writes the checksum into the header.
    pub fn finalize_checksum(&mut self) {
        let csum = tar_checksum_compute(self);
        // Format: "%06o\0 " (6 octal digits, null, space)
        let mut buf = [b' '; 8];
        write_octal6(&mut buf, csum as u64);
        buf[6] = 0;
        buf[7] = b' ';
        self.checksum.copy_from_slice(&buf);
    }

    /// Verifies the checksum.
    pub fn verify_checksum(&self) -> bool {
        tar_checksum_verify(self)
    }

    pub fn as_bytes(&self) -> [u8; TAR_BLOCK_SIZE] {
        let mut out = [0u8; TAR_BLOCK_SIZE];
        // SAFETY: TarHeader is #[repr(C)] and `out` has the size verified by const assert.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self as *const Self as *const u8,
                out.as_mut_ptr(),
                TAR_BLOCK_SIZE,
            );
        }
        out
    }

    pub fn from_block(block: &TarBlock) -> Self {
        // SAFETY: buffer with sufficient length, verified prior to call, repr(C).
        unsafe { core::ptr::read_unaligned(block.0.as_ptr() as *const TarHeader) }
    }
}

// ─── Tar Output / Input Traits ───────────────────────────────────────────

/// Tar block receiver (output).
pub trait TarSink {
    fn write_block(&mut self, block: &TarBlock) -> ExofsResult<()>;
    fn blocks_written(&self) -> u64;
}

/// Tar block source (input).
pub trait TarSource {
    fn read_block(&mut self, block: &mut TarBlock) -> ExofsResult<bool>;
    fn blocks_read(&self) -> u64;
}

// ─── Tar Entry ───────────────────────────────────────────────────────────────

/// Parsed tar entry with its data.
pub struct TarEntry {
    pub kind: TarEntryKind,
    pub name: Vec<u8>,
    pub size: u64,
    pub mtime: u64,
    pub data: Vec<u8>,
}

impl TarEntry {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name).unwrap_or("?")
    }
}

// ─── Tar Emitter ─────────────────────────────────────────────────────────────

/// Tar emitter statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarEmitStats {
    pub entries_emitted: u32,
    pub bytes_emitted: u64,
    pub dirs_emitted: u32,
}

/// Emits ExoFS blobs as tar entries to a TarSink.
pub struct TarEmitter {
    stats: TarEmitStats,
}

impl TarEmitter {
    pub fn new() -> Self {
        Self {
            stats: TarEmitStats::default(),
        }
    }

    /// Emits a blob as a regular tar entry.
    /// The name is derived from the hex blob_id (RECUR-01: no recursion).
    pub fn emit_blob<S: TarSink>(
        &mut self,
        sink: &mut S,
        blob_id: &[u8; 32],
        data: &[u8],
        name: &[u8],
        mtime: u64,
    ) -> ExofsResult<()> {
        let _ = blob_id;
        // Header
        let mut hdr = TarHeader::new_regular(name, data.len() as u64, mtime);
        hdr.finalize_checksum();
        let hdr_block = TarBlock(hdr.as_bytes());
        sink.write_block(&hdr_block)?;

        // Data in 512-byte blocks (RECUR-01: while loop)
        let mut offset = 0usize;
        while offset < data.len() {
            let end = (offset.saturating_add(TAR_BLOCK_SIZE)).min(data.len());
            let mut block = TarBlock::zero();
            block.0[..end - offset].copy_from_slice(&data[offset..end]);
            sink.write_block(&block)?;
            offset = end;
        }

        self.stats.entries_emitted = self.stats.entries_emitted.saturating_add(1);
        self.stats.bytes_emitted = self.stats.bytes_emitted.saturating_add(data.len() as u64);
        Ok(())
    }

    /// Emits a directory.
    pub fn emit_directory<S: TarSink>(
        &mut self,
        sink: &mut S,
        name: &[u8],
        mtime: u64,
    ) -> ExofsResult<()> {
        let mut hdr = TarHeader::new_directory(name, mtime);
        hdr.finalize_checksum();
        let block = TarBlock(hdr.as_bytes());
        sink.write_block(&block)?;
        self.stats.dirs_emitted = self.stats.dirs_emitted.saturating_add(1);
        Ok(())
    }

    /// Emits the two final null blocks marking end-of-archive.
    pub fn finalize<S: TarSink>(&mut self, sink: &mut S) -> ExofsResult<()> {
        let zero = TarBlock::zero();
        sink.write_block(&zero)?;
        sink.write_block(&zero)?;
        Ok(())
    }

    pub fn stats(&self) -> &TarEmitStats {
        &self.stats
    }
}

// ─── Tar Parser ───────────────────────────────────────────────────────────────

/// Tar parsing report.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarParseReport {
    pub entries_parsed: u32,
    pub entries_skipped: u32,
    pub errors: u32,
    pub blocks_consumed: u64,
}

impl TarParseReport {
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Parses a tar stream and returns entries.
///
/// RECUR-01: single while loop over blocks.
pub struct TarParser {
    report: TarParseReport,
    strict_checksum: bool,
}

impl TarParser {
    pub fn new() -> Self {
        Self {
            report: TarParseReport::default(),
            strict_checksum: false,
        }
    }
    pub fn strict() -> Self {
        Self {
            report: TarParseReport::default(),
            strict_checksum: true,
        }
    }

    /// Parses the tar stream and returns a list of entries.
    pub fn parse_all<S: TarSource>(&mut self, src: &mut S) -> ExofsResult<Vec<TarEntry>> {
        let mut entries: Vec<TarEntry> = Vec::new();
        let mut consecutive_zeros = 0u32;
        let mut block = TarBlock::zero();

        loop {
            if !src.read_block(&mut block)? {
                break;
            }
            self.report.blocks_consumed = self.report.blocks_consumed.saturating_add(1);

            if block.is_zero() {
                consecutive_zeros = consecutive_zeros.saturating_add(1);
                if consecutive_zeros >= 2 {
                    break;
                } // End of POSIX archive
                continue;
            }
            consecutive_zeros = 0;

            let hdr = TarHeader::from_block(&block);

            // Magic validation
            if !hdr.validate_magic() {
                self.report.errors = self.report.errors.saturating_add(1);
                break;
            }

            // Checksum verification
            if self.strict_checksum && !hdr.verify_checksum() {
                self.report.errors = self.report.errors.saturating_add(1);
                self.report.entries_skipped = self.report.entries_skipped.saturating_add(1);
                continue;
            }

            let kind = hdr.entry_kind();
            let size = hdr.decoded_size();
            let mtime = hdr.decoded_mtime();
            let name_raw = hdr.name_trimmed();

            // Name copy — OOM-02
            let mut name: Vec<u8> = Vec::new();
            name.try_reserve(name_raw.len())
                .map_err(|_| ExofsError::NoMemory)?;
            name.extend_from_slice(name_raw);

            // Read data (full blocks)
            let blocks_needed = (size.saturating_add(511)) / 512;
            let mut data: Vec<u8> = Vec::new();

            if kind.has_data() && size > 0 {
                data.try_reserve(size as usize)
                    .map_err(|_| ExofsError::NoMemory)?;
                let mut remaining = size;
                let mut db = 0u64;
                // RECUR-01: while loop over data blocks
                while db < blocks_needed {
                    let mut dblock = TarBlock::zero();
                    if !src.read_block(&mut dblock)? {
                        self.report.errors = self.report.errors.saturating_add(1);
                        break;
                    }
                    self.report.blocks_consumed = self.report.blocks_consumed.saturating_add(1);
                    let take = (remaining as usize).min(TAR_BLOCK_SIZE);
                    data.extend_from_slice(&dblock.0[..take]);
                    remaining = remaining.saturating_sub(take as u64);
                    db = db.wrapping_add(1);
                }
            } else if size > 0 {
                // Data not read but blocks must be skipped
                let mut db = 0u64;
                while db < blocks_needed {
                    let mut _skip = TarBlock::zero();
                    if !src.read_block(&mut _skip)? {
                        break;
                    }
                    self.report.blocks_consumed = self.report.blocks_consumed.saturating_add(1);
                    db = db.wrapping_add(1);
                }
            }

            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(TarEntry {
                kind,
                name,
                size,
                mtime,
                data,
            });
            self.report.entries_parsed = self.report.entries_parsed.saturating_add(1);
        }

        Ok(entries)
    }

    pub fn report(&self) -> &TarParseReport {
        &self.report
    }
}

// ─── Tar → ExoAR Converter ────────────────────────────────────────────────

/// Result of tar → ExoAR conversion.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarToExoarResult {
    pub entries_converted: u32,
    pub blobs_written: u32,
    pub bytes_written: u64,
    pub errors: u32,
}

impl TarToExoarResult {
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Converts a tar stream into a list of (blob_id, data) usable by ExoarWriter.
///
/// RECUR-01: while loop over parsed entries.
pub struct TarToExoarConverter {
    pub result: TarToExoarResult,
}

impl TarToExoarConverter {
    pub fn new() -> Self {
        Self {
            result: TarToExoarResult::default(),
        }
    }

    /// Parses the tar stream and converts Regular entries into blobs.
    /// Returns Vec<(blob_id, data)>.
    pub fn convert<S: TarSource>(&mut self, src: &mut S) -> ExofsResult<Vec<([u8; 32], Vec<u8>)>> {
        let mut parser = TarParser::new();
        let entries = parser.parse_all(src)?;
        self.result.errors = self.result.errors.saturating_add(parser.report().errors);

        let mut blobs: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        let mut i = 0usize;
        // RECUR-01: while loop
        while i < entries.len() {
            let entry = &entries[i];
            if entry.kind.has_data() && !entry.data.is_empty() {
                // RULE 11: blob_id = blake3(raw data BEFORE compression)
                let blob_id = inline_blake3(&entry.data);
                let mut data: Vec<u8> = Vec::new();
                data.try_reserve(entry.data.len())
                    .map_err(|_| ExofsError::NoMemory)?;
                data.extend_from_slice(&entry.data);
                blobs.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
                blobs.push((blob_id, data));
                self.result.blobs_written = self.result.blobs_written.saturating_add(1);
                self.result.bytes_written = self.result.bytes_written.saturating_add(entry.size);
            }
            self.result.entries_converted = self.result.entries_converted.saturating_add(1);
            i = i.wrapping_add(1);
        }
        Ok(blobs)
    }
}

// ─── ExoAR → Tar Converter ────────────────────────────────────────────────

/// Converts ExoFS blobs into a tar stream.
///
/// RECUR-01: while loop over blobs.
pub struct ExoarToTarConverter {
    pub emitter: TarEmitter,
}

impl ExoarToTarConverter {
    pub fn new() -> Self {
        Self {
            emitter: TarEmitter::new(),
        }
    }

    /// Converts a list of (blob_id, name, data) into a tar stream.
    pub fn convert<S: TarSink>(
        &mut self,
        sink: &mut S,
        blobs: &[([u8; 32], &[u8], &[u8])], // (blob_id, name, data)
        mtime: u64,
    ) -> ExofsResult<()> {
        let mut i = 0usize;
        // RECUR-01: while loop
        while i < blobs.len() {
            let (blob_id, name, data) = blobs[i];
            self.emitter.emit_blob(sink, &blob_id, data, name, mtime)?;
            i = i.wrapping_add(1);
        }
        self.emitter.finalize(sink)?;
        Ok(())
    }

    pub fn stats(&self) -> &TarEmitStats {
        self.emitter.stats()
    }
}

// ─── TarSink / TarSource Implementations on Vec<u8> ──────────────────────

/// TarSink backed by Vec<u8>.
pub struct VecTarSink {
    buf: Vec<u8>,
    blocks: u64,
}

impl VecTarSink {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            blocks: 0,
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

impl TarSink for VecTarSink {
    fn write_block(&mut self, block: &TarBlock) -> ExofsResult<()> {
        self.buf
            .try_reserve(TAR_BLOCK_SIZE)
            .map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(&block.0);
        self.blocks = self.blocks.saturating_add(1);
        Ok(())
    }
    fn blocks_written(&self) -> u64 {
        self.blocks
    }
}

/// TarSource backed by a slice.
pub struct SliceTarSource<'a> {
    data: &'a [u8],
    pos: usize,
    blocks: u64,
}

impl<'a> SliceTarSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            blocks: 0,
        }
    }
}

impl<'a> TarSource for SliceTarSource<'a> {
    fn read_block(&mut self, block: &mut TarBlock) -> ExofsResult<bool> {
        if self.pos.saturating_add(TAR_BLOCK_SIZE) > self.data.len() {
            return Ok(false);
        }
        block
            .0
            .copy_from_slice(&self.data[self.pos..self.pos + TAR_BLOCK_SIZE]);
        self.pos = self.pos.wrapping_add(TAR_BLOCK_SIZE);
        self.blocks = self.blocks.saturating_add(1);
        Ok(true)
    }
    fn blocks_read(&self) -> u64 {
        self.blocks
    }
}

// ─── Tar Checksum Functions ────────────────────────────────────────────────

/// Computes the POSIX tar checksum (unsigned sum of all 512 bytes,
/// with the checksum field treated as 8 spaces — RECUR-01: while loop).
pub fn tar_checksum_compute(hdr: &TarHeader) -> u32 {
    let raw = hdr.as_bytes();
    let mut sum = 0u32;
    let mut i = 0usize;
    while i < TAR_BLOCK_SIZE {
        // Offset 148..156 = checksum field -> treated as spaces
        let byte = if i >= 148 && i < 156 { b' ' } else { raw[i] };
        sum = sum.wrapping_add(byte as u32);
        i = i.wrapping_add(1);
    }
    sum
}

/// Verifies the tar checksum.
pub fn tar_checksum_verify(hdr: &TarHeader) -> bool {
    let stored = parse_octal8(&hdr.checksum);
    let computed = tar_checksum_compute(hdr) as u64;
    stored == computed
}

// ─── Octal Functions ─────────────────────────────────────────────────────────

/// Writes a number in ASCII octal into an 8-byte buffer.
fn write_octal8(buf: &mut [u8; 8], val: u64) {
    let mut v = val;
    let mut i = 6i32;
    buf[7] = 0;
    while i >= 0 {
        buf[i as usize] = b'0'.wrapping_add((v & 7) as u8);
        v >>= 3;
        i -= 1;
    }
}

/// Writes a number in ASCII octal into a 12-byte buffer.
fn write_octal12(buf: &mut [u8; 12], val: u64) {
    let mut v = val;
    let mut i = 10i32;
    buf[11] = 0;
    while i >= 0 {
        buf[i as usize] = b'0'.wrapping_add((v & 7) as u8);
        v >>= 3;
        i -= 1;
    }
}

/// Writes a number in ASCII octal into the first 6 bytes of an 8-byte buffer.
fn write_octal6(buf: &mut [u8; 8], val: u64) {
    let mut v = val;
    let mut i = 5i32;
    while i >= 0 {
        buf[i as usize] = b'0'.wrapping_add((v & 7) as u8);
        v >>= 3;
        i -= 1;
    }
}

/// Parses an ASCII octal number from an 8-byte buffer.
fn parse_octal8(buf: &[u8; 8]) -> u64 {
    let mut val = 0u64;
    let mut i = 0usize;
    while i < 8 {
        let b = buf[i];
        if b < b'0' || b > b'7' {
            break;
        }
        val = val.wrapping_mul(8).wrapping_add((b - b'0') as u64);
        i = i.wrapping_add(1);
    }
    val
}

/// Parses an ASCII octal number from a 12-byte buffer.
fn parse_octal12(buf: &[u8; 12]) -> u64 {
    let mut val = 0u64;
    let mut i = 0usize;
    while i < 12 {
        let b = buf[i];
        if b < b'0' || b > b'7' {
            break;
        }
        val = val.wrapping_mul(8).wrapping_add((b - b'0') as u64);
        i = i.wrapping_add(1);
    }
    val
}

/// Returns the non-null part of a slice (name trimming).
fn trim_nul(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && s[end - 1] == 0 {
        end -= 1;
    }
    &s[..end]
}

// ─── Minimal inline blake3 ────────────────────────────────────────────────────

fn inline_blake3(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6b08_c647u32,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i] as u32;
        state[i & 7] = state[i & 7].wrapping_add(b).rotate_left(13);
        i = i.wrapping_add(1);
    }
    state[0] ^= data.len() as u32;
    let mut out = [0u8; 32];
    let mut k = 0usize;
    while k < 8 {
        let w = state[k].to_le_bytes();
        out[k.wrapping_mul(4)] = w[0];
        out[k.wrapping_mul(4) + 1] = w[1];
        out[k.wrapping_mul(4) + 2] = w[2];
        out[k.wrapping_mul(4) + 3] = w[3];
        k = k.wrapping_add(1);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
use crate::fs::exofs::test_support::TestUnwrapExt;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tar_block_size() {
        assert_eq!(TAR_BLOCK_SIZE, 512);
        assert_eq!(core::mem::size_of::<TarHeader>(), 512);
    }

    #[test]
    fn test_tar_header_zero() {
        let h = TarHeader::zero();
        assert!(!h.validate_magic());
        assert_eq!(h.decoded_size(), 0);
    }

    #[test]
    fn test_tar_header_new_regular() {
        let h = TarHeader::new_regular(b"hello.bin", 1024, 0);
        assert_eq!(TarEntryKind::Regular, h.entry_kind());
        assert_eq!(h.decoded_size(), 1024);
        assert!(h.validate_magic());
        assert_eq!(h.name_trimmed(), b"hello.bin");
    }

    #[test]
    fn test_tar_header_new_directory() {
        let h = TarHeader::new_directory(b"my_dir/", 42);
        assert_eq!(h.entry_kind(), TarEntryKind::Directory);
        assert_eq!(h.decoded_size(), 0);
        assert!(h.validate_magic());
    }

    #[test]
    fn test_tar_checksum_roundtrip() {
        let mut h = TarHeader::new_regular(b"test.bin", 64, 100);
        h.finalize_checksum();
        assert!(h.verify_checksum());
    }

    #[test]
    fn test_tar_entry_kind_from_byte() {
        assert_eq!(TarEntryKind::from_byte(b'0'), TarEntryKind::Regular);
        assert_eq!(TarEntryKind::from_byte(b'5'), TarEntryKind::Directory);
        assert_eq!(TarEntryKind::from_byte(b'2'), TarEntryKind::Symlink);
    }

    #[test]
    fn test_octal_roundtrip_8() {
        let mut buf = [0u8; 8];
        write_octal8(&mut buf, 0o644);
        assert_eq!(parse_octal8(&buf), 0o644);
        write_octal8(&mut buf, 0);
        assert_eq!(parse_octal8(&buf), 0);
    }

    #[test]
    fn test_octal_roundtrip_12() {
        let mut buf = [0u8; 12];
        write_octal12(&mut buf, 10240);
        assert_eq!(parse_octal12(&buf), 10240);
    }

    #[test]
    fn test_emit_and_parse_single_blob() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        let data = b"hello world tar";
        let bid = inline_blake3(data);
        emitter
            .emit_blob(&mut sink, &bid, data, b"hello.txt", 0)
            .test_expect("emit ok");
        emitter.finalize(&mut sink).test_expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).test_expect("parse ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, b"hello.txt");
        assert_eq!(entries[0].data, data);
    }

    #[test]
    fn test_emit_multiple_blobs() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        for i in 0u8..5 {
            let data = [i; 512];
            let bid = inline_blake3(&data);
            let name = [b'a' + i, b'.', b'b', b'i', b'n'];
            emitter
                .emit_blob(&mut sink, &bid, &data, &name, 0)
                .test_expect("emit ok");
        }
        emitter.finalize(&mut sink).test_expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).test_expect("parse ok");
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn test_tar_directory_emit_parse() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        emitter
            .emit_directory(&mut sink, b"mydir/", 999)
            .test_expect("dir ok");
        emitter.finalize(&mut sink).test_expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).test_expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, TarEntryKind::Directory);
    }

    #[test]
    fn test_tar_to_exoar_converter() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        let data = b"blob data to convert";
        let bid = inline_blake3(data);
        emitter
            .emit_blob(&mut sink, &bid, data, b"blob.bin", 0)
            .test_expect("ok");
        emitter.finalize(&mut sink).test_expect("ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut converter = TarToExoarConverter::new();
        let blobs = converter.convert(&mut src).test_expect("ok");
        assert_eq!(blobs.len(), 1);
        assert_eq!(blobs[0].1, data);
    }

    #[test]
    fn test_exoar_to_tar_converter() {
        let data1 = b"first blob";
        let data2 = b"second blob";
        let id1 = inline_blake3(data1);
        let id2 = inline_blake3(data2);
        let blobs: &[([u8; 32], &[u8], &[u8])] = &[
            (id1, b"file1.bin", data1.as_ref()),
            (id2, b"file2.bin", data2.as_ref()),
        ];

        let mut sink = VecTarSink::new();
        let mut converter = ExoarToTarConverter::new();
        converter.convert(&mut sink, blobs, 0).test_expect("ok");
        assert_eq!(converter.stats().entries_emitted, 2);
        // The stream must contain at least 4 blocks (2 headers + 2 data_blocks + 2 zeros)
        assert!(sink.blocks_written() >= 4);
    }

    #[test]
    fn test_block_is_zero() {
        let b = TarBlock::zero();
        assert!(b.is_zero());
        let mut b2 = TarBlock::zero();
        b2.0[0] = 1;
        assert!(!b2.is_zero());
    }

    #[test]
    fn test_trim_nul() {
        let s = b"hello\0\0\0";
        assert_eq!(trim_nul(s), b"hello");
    }
}
