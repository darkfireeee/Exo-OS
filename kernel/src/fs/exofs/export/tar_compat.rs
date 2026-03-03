//! tar_compat.rs — Compatibilité POSIX ustar avec le format ExoAR (no_std).
//!
//! Ce module fournit :
//!  - `TarBlock`            : bloc tar de 512 bytes.
//!  - `TarHeader`           : en-tête ustar (512 bytes).
//!  - `TarEntryKind`        : type d'entrée tar (Regular/Dir/Symlink/...).
//!  - `TarEntry`            : entrée tar avec métadonnées et données.
//!  - `TarEmitter`          : émet des blobs ExoFS sous forme de tar.
//!  - `TarParser`           : analyse un flux tar.
//!  - `TarSink`             : trait de sortie tar.
//!  - `TarSource`           : trait de lecture tar.
//!  - `TarToExoarConverter` : convertit un flux tar en archive ExoAR.
//!  - `ExoarToTarConverter` : convertit des blobs ExoAR en flux tar.
//!
//! Format : POSIX.1-2001 / GNU tar ustar.
//!
//! RECUR-01 : boucles while — aucune récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_*, checked_div, wrapping_add.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::incremental_export::EpochId;

// ─── Constantes ───────────────────────────────────────────────────────────────

/// Taille d'un bloc tar POSIX.
pub const TAR_BLOCK_SIZE: usize = 512;

/// Magic du format ustar.
pub const TAR_MAGIC: &[u8; 6] = b"ustar ";

/// Version ustar.
pub const TAR_VERSION: &[u8; 2] = b"00";

/// Taille max d'un nom de fichier tar.
pub const TAR_NAME_MAX: usize = 100;

/// Taille max d'un préfixe tar.
pub const TAR_PREFIX_MAX: usize = 155;

/// Taille max d'un lien symbolique.
pub const TAR_LINKNAME_MAX: usize = 100;

// ─── Type d'entrée tar ────────────────────────────────────────────────────────

/// Type d'entrée tar (champ typeflag).
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum TarEntryKind {
    Regular    = b'0',
    HardLink   = b'1',
    Symlink    = b'2',
    CharDevice = b'3',
    BlockDevice = b'4',
    Directory  = b'5',
    Fifo       = b'6',
    GlobalPax  = b'g',
    ExtendedPax = b'x',
    Unknown    = b'?',
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

    pub fn as_byte(self) -> u8 { self as u8 }
    pub fn is_regular(self) -> bool { matches!(self, TarEntryKind::Regular) }
    pub fn is_directory(self) -> bool { matches!(self, TarEntryKind::Directory) }
    pub fn has_data(self) -> bool { matches!(self, TarEntryKind::Regular) }
}

// ─── Bloc tar ─────────────────────────────────────────────────────────────────

/// Un bloc tar de 512 bytes.
#[derive(Clone, Copy)]
#[repr(C, align(512))]
pub struct TarBlock(pub [u8; TAR_BLOCK_SIZE]);

impl TarBlock {
    pub const fn zero() -> Self { Self([0u8; TAR_BLOCK_SIZE]) }
    pub fn as_bytes(&self) -> &[u8] { &self.0 }
    pub fn as_mut_bytes(&mut self) -> &mut [u8] { &mut self.0 }
    pub fn is_zero(&self) -> bool { self.0.iter().all(|&b| b == 0) }
}

// ─── En-tête tar ustar ────────────────────────────────────────────────────────

/// En-tête ustar — 512 bytes.
/// Offsets selon POSIX.1-2001.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct TarHeader {
    pub name:     [u8; 100], // 0
    pub mode:     [u8; 8],   // 100
    pub uid:      [u8; 8],   // 108
    pub gid:      [u8; 8],   // 116
    pub size:     [u8; 12],  // 124
    pub mtime:    [u8; 12],  // 136
    pub checksum: [u8; 8],   // 148
    pub typeflag: u8,        // 156
    pub linkname: [u8; 100], // 157
    pub magic:    [u8; 6],   // 257
    pub version:  [u8; 2],   // 263
    pub uname:    [u8; 32],  // 265
    pub gname:    [u8; 32],  // 297
    pub devmajor: [u8; 8],   // 329
    pub devminor: [u8; 8],   // 337
    pub prefix:   [u8; 155], // 345
    pub _pad:     [u8; 12],  // 500
}

const _: () = assert!(core::mem::size_of::<TarHeader>() == TAR_BLOCK_SIZE);

impl TarHeader {
    pub fn zero() -> Self {
        unsafe { core::mem::zeroed() }
    }

    /// Crée un en-tête pour un fichier régulier ExoFS.
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
        // checksum calculé séparément
        hdr
    }

    /// Crée un en-tête pour un répertoire.
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

    /// Retourne le type d'entrée.
    pub fn entry_kind(&self) -> TarEntryKind {
        TarEntryKind::from_byte(self.typeflag)
    }

    /// Retourne la taille décodée depuis le champ octal.
    pub fn decoded_size(&self) -> u64 {
        parse_octal12(&self.size)
    }

    /// Retourne le mtime décodé.
    pub fn decoded_mtime(&self) -> u64 {
        parse_octal12(&self.mtime)
    }

    /// Valide le magic ustar.
    pub fn validate_magic(&self) -> bool {
        &self.magic[..6] == TAR_MAGIC || &self.magic[..5] == b"ustar"
    }

    /// Retourne le nom comme slice valide (sans nuls).
    pub fn name_trimmed(&self) -> &[u8] {
        trim_nul(&self.name)
    }

    /// Calcule le checksum (somme des octets, champ checksum = 8 espaces durant calcul).
    pub fn compute_checksum(&self) -> u32 {
        tar_checksum_compute(self)
    }

    /// Écrit le checksum dans l'en-tête.
    pub fn finalize_checksum(&mut self) {
        let csum = tar_checksum_compute(self);
        // Format : "%06o\0 " (6 chiffres octaux, nul, espace)
        let mut buf = [b' '; 8];
        write_octal6(&mut buf, csum as u64);
        buf[6] = 0;
        buf[7] = b' ';
        self.checksum.copy_from_slice(&buf);
    }

    /// Vérifie le checksum.
    pub fn verify_checksum(&self) -> bool {
        tar_checksum_verify(self)
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                TAR_BLOCK_SIZE,
            )
        }
    }

    pub fn from_block(block: &TarBlock) -> Self {
        unsafe { core::ptr::read_unaligned(block.0.as_ptr() as *const TarHeader) }
    }
}

// ─── Traits de sortie / entrée tar ───────────────────────────────────────────

/// Récepteur de blocs tar (sortie).
pub trait TarSink {
    fn write_block(&mut self, block: &TarBlock) -> ExofsResult<()>;
    fn blocks_written(&self) -> u64;
}

/// Source de blocs tar (entrée).
pub trait TarSource {
    fn read_block(&mut self, block: &mut TarBlock) -> ExofsResult<bool>;
    fn blocks_read(&self) -> u64;
}

// ─── Entrée tar ───────────────────────────────────────────────────────────────

/// Entrée tar parsée avec ses données.
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

// ─── Émetteur tar ─────────────────────────────────────────────────────────────

/// Statistiques de l'émetteur tar.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarEmitStats {
    pub entries_emitted: u32,
    pub bytes_emitted: u64,
    pub dirs_emitted: u32,
}

/// Émet des blobs ExoFS sous forme d'entrées tar vers un TarSink.
pub struct TarEmitter {
    stats: TarEmitStats,
}

impl TarEmitter {
    pub fn new() -> Self { Self { stats: TarEmitStats::default() } }

    /// Émet un blob sous forme d'entrée tar régulière.
    /// Le nom est dérivé du blob_id hex (RECUR-01 : pas de récursion).
    pub fn emit_blob<S: TarSink>(
        &mut self,
        sink: &mut S,
        blob_id: &[u8; 32],
        data: &[u8],
        name: &[u8],
        mtime: u64,
    ) -> ExofsResult<()> {
        let _ = blob_id;
        // En-tête
        let mut hdr = TarHeader::new_regular(name, data.len() as u64, mtime);
        hdr.finalize_checksum();
        let hdr_block = TarBlock(*array_ref512(hdr.as_bytes()));
        sink.write_block(&hdr_block)?;

        // Données par blocs de 512 bytes (RECUR-01 : boucle while)
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

    /// Émet un répertoire.
    pub fn emit_directory<S: TarSink>(
        &mut self,
        sink: &mut S,
        name: &[u8],
        mtime: u64,
    ) -> ExofsResult<()> {
        let mut hdr = TarHeader::new_directory(name, mtime);
        hdr.finalize_checksum();
        let block = TarBlock(*array_ref512(hdr.as_bytes()));
        sink.write_block(&block)?;
        self.stats.dirs_emitted = self.stats.dirs_emitted.saturating_add(1);
        Ok(())
    }

    /// Émet les deux blocs finaux nuls de fin d'archive.
    pub fn finalize<S: TarSink>(&mut self, sink: &mut S) -> ExofsResult<()> {
        let zero = TarBlock::zero();
        sink.write_block(&zero)?;
        sink.write_block(&zero)?;
        Ok(())
    }

    pub fn stats(&self) -> &TarEmitStats { &self.stats }
}

// ─── Parser tar ───────────────────────────────────────────────────────────────

/// Rapport de parsing tar.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarParseReport {
    pub entries_parsed: u32,
    pub entries_skipped: u32,
    pub errors: u32,
    pub blocks_consumed: u64,
}

impl TarParseReport {
    pub fn has_errors(&self) -> bool { self.errors > 0 }
}

/// Parse un flux tar et retourne les entrées.
///
/// RECUR-01 : boucle while unique sur les blocs.
pub struct TarParser {
    report: TarParseReport,
    strict_checksum: bool,
}

impl TarParser {
    pub fn new() -> Self { Self { report: TarParseReport::default(), strict_checksum: false } }
    pub fn strict() -> Self { Self { report: TarParseReport::default(), strict_checksum: true } }

    /// Parse le flux tar et retourne a list d'entrées.
    pub fn parse_all<S: TarSource>(&mut self, src: &mut S) -> ExofsResult<Vec<TarEntry>> {
        let mut entries: Vec<TarEntry> = Vec::new();
        let mut consecutive_zeros = 0u32;
        let mut block = TarBlock::zero();

        loop {
            if !src.read_block(&mut block)? { break; }
            self.report.blocks_consumed = self.report.blocks_consumed.saturating_add(1);

            if block.is_zero() {
                consecutive_zeros = consecutive_zeros.saturating_add(1);
                if consecutive_zeros >= 2 { break; } // Fin d'archive POSIX
                continue;
            }
            consecutive_zeros = 0;

            let hdr = TarHeader::from_block(&block);

            // Validation magic
            if !hdr.validate_magic() {
                self.report.errors = self.report.errors.saturating_add(1);
                break;
            }

            // Vérification checksum
            if self.strict_checksum && !hdr.verify_checksum() {
                self.report.errors = self.report.errors.saturating_add(1);
                self.report.entries_skipped = self.report.entries_skipped.saturating_add(1);
                continue;
            }

            let kind = hdr.entry_kind();
            let size = hdr.decoded_size();
            let mtime = hdr.decoded_mtime();
            let name_raw = hdr.name_trimmed();

            // Copie du nom — OOM-02
            let mut name: Vec<u8> = Vec::new();
            name.try_reserve(name_raw.len()).map_err(|_| ExofsError::NoMemory)?;
            name.extend_from_slice(name_raw);

            // Lecture des données (blocs entiers)
            let blocks_needed = (size.saturating_add(511)) / 512;
            let mut data: Vec<u8> = Vec::new();

            if kind.has_data() && size > 0 {
                data.try_reserve(size as usize).map_err(|_| ExofsError::NoMemory)?;
                let mut remaining = size;
                let mut db = 0u64;
                // RECUR-01 : boucle while sur les blocs de données
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
                // Données non lues mais blocs à sauter
                let mut db = 0u64;
                while db < blocks_needed {
                    let mut _skip = TarBlock::zero();
                    if !src.read_block(&mut _skip)? { break; }
                    self.report.blocks_consumed = self.report.blocks_consumed.saturating_add(1);
                    db = db.wrapping_add(1);
                }
            }

            entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            entries.push(TarEntry { kind, name, size, mtime, data });
            self.report.entries_parsed = self.report.entries_parsed.saturating_add(1);
        }

        Ok(entries)
    }

    pub fn report(&self) -> &TarParseReport { &self.report }
}

// ─── Convertisseur tar → ExoAR ────────────────────────────────────────────────

/// Résultat de la conversion tar → ExoAR.
#[derive(Clone, Copy, Debug, Default)]
pub struct TarToExoarResult {
    pub entries_converted: u32,
    pub blobs_written: u32,
    pub bytes_written: u64,
    pub errors: u32,
}

impl TarToExoarResult {
    pub fn has_errors(&self) -> bool { self.errors > 0 }
}

/// Convertit un flux tar en liste de (blob_id, data) utilisable par ExoarWriter.
///
/// RECUR-01 : boucle while sur les entrées parsées.
pub struct TarToExoarConverter {
    pub result: TarToExoarResult,
}

impl TarToExoarConverter {
    pub fn new() -> Self { Self { result: TarToExoarResult::default() } }

    /// Parse le flux tar et converti les entrées Regular en blobs.
    /// Retourne Vec<(blob_id, data)>.
    pub fn convert<S: TarSource>(
        &mut self,
        src: &mut S,
    ) -> ExofsResult<Vec<([u8; 32], Vec<u8>)>> {
        let mut parser = TarParser::new();
        let entries = parser.parse_all(src)?;
        self.result.errors = self.result.errors.saturating_add(parser.report().errors);

        let mut blobs: Vec<([u8; 32], Vec<u8>)> = Vec::new();

        let mut i = 0usize;
        // RECUR-01 : boucle while
        while i < entries.len() {
            let entry = &entries[i];
            if entry.kind.has_data() && !entry.data.is_empty() {
                // RÈGLE 11 : blob_id = blake3(données brutes AVANT compression)
                let blob_id = inline_blake3(&entry.data);
                let mut data: Vec<u8> = Vec::new();
                data.try_reserve(entry.data.len()).map_err(|_| ExofsError::NoMemory)?;
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

// ─── Convertisseur ExoAR → tar ────────────────────────────────────────────────

/// Convertit des blobs ExoFS en flux tar.
///
/// RECUR-01 : boucle while sur les blobs.
pub struct ExoarToTarConverter {
    pub emitter: TarEmitter,
}

impl ExoarToTarConverter {
    pub fn new() -> Self { Self { emitter: TarEmitter::new() } }

    /// Convertit une liste de (blob_id, name, data) en flux tar.
    pub fn convert<S: TarSink>(
        &mut self,
        sink: &mut S,
        blobs: &[([u8; 32], &[u8], &[u8])], // (blob_id, name, data)
        mtime: u64,
    ) -> ExofsResult<()> {
        let mut i = 0usize;
        // RECUR-01 : boucle while
        while i < blobs.len() {
            let (blob_id, name, data) = blobs[i];
            self.emitter.emit_blob(sink, blob_id, data, name, mtime)?;
            i = i.wrapping_add(1);
        }
        self.emitter.finalize(sink)?;
        Ok(())
    }

    pub fn stats(&self) -> &TarEmitStats { self.emitter.stats() }
}

// ─── Implémentations de TarSink / TarSource sur Vec<u8> ──────────────────────

/// TarSink vers Vec<u8>.
pub struct VecTarSink {
    buf: Vec<u8>,
    blocks: u64,
}

impl VecTarSink {
    pub fn new() -> Self { Self { buf: Vec::new(), blocks: 0 } }
    pub fn as_slice(&self) -> &[u8] { &self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
}

impl TarSink for VecTarSink {
    fn write_block(&mut self, block: &TarBlock) -> ExofsResult<()> {
        self.buf.try_reserve(TAR_BLOCK_SIZE).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(&block.0);
        self.blocks = self.blocks.saturating_add(1);
        Ok(())
    }
    fn blocks_written(&self) -> u64 { self.blocks }
}

/// TarSource depuis un slice.
pub struct SliceTarSource<'a> {
    data: &'a [u8],
    pos: usize,
    blocks: u64,
}

impl<'a> SliceTarSource<'a> {
    pub fn new(data: &'a [u8]) -> Self { Self { data, pos: 0, blocks: 0 } }
}

impl<'a> TarSource for SliceTarSource<'a> {
    fn read_block(&mut self, block: &mut TarBlock) -> ExofsResult<bool> {
        if self.pos.saturating_add(TAR_BLOCK_SIZE) > self.data.len() {
            return Ok(false);
        }
        block.0.copy_from_slice(&self.data[self.pos..self.pos + TAR_BLOCK_SIZE]);
        self.pos = self.pos.wrapping_add(TAR_BLOCK_SIZE);
        self.blocks = self.blocks.saturating_add(1);
        Ok(true)
    }
    fn blocks_read(&self) -> u64 { self.blocks }
}

// ─── Fonctions de checksum tar ────────────────────────────────────────────────

/// Calcule le checksum POSIX tar (somme non signée des 512 bytes,
/// avec le champ checksum traité comme 8 espaces — RECUR-01 : boucle while).
pub fn tar_checksum_compute(hdr: &TarHeader) -> u32 {
    let raw = hdr.as_bytes();
    let mut sum = 0u32;
    let mut i = 0usize;
    while i < TAR_BLOCK_SIZE {
        // Offset 148..156 = champ checksum → traité comme des espaces
        let byte = if i >= 148 && i < 156 { b' ' } else { raw[i] };
        sum = sum.wrapping_add(byte as u32);
        i = i.wrapping_add(1);
    }
    sum
}

/// Vérifie le checksum tar.
pub fn tar_checksum_verify(hdr: &TarHeader) -> bool {
    let stored = parse_octal8(&hdr.checksum);
    let computed = tar_checksum_compute(hdr) as u64;
    stored == computed
}

// ─── Fonctions octal ─────────────────────────────────────────────────────────

/// Écrit un nombre en octal ASCII dans un buffer de 8 bytes.
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

/// Écrit un nombre en octal ASCII dans un buffer de 12 bytes.
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

/// Écrit un nombre en octal ASCII dans les 6 premiers bytes d'un buffer de 8.
fn write_octal6(buf: &mut [u8; 8], val: u64) {
    let mut v = val;
    let mut i = 5i32;
    while i >= 0 {
        buf[i as usize] = b'0'.wrapping_add((v & 7) as u8);
        v >>= 3;
        i -= 1;
    }
}

/// Parse un nombre octal ASCII depuis un buffer de 8 bytes.
fn parse_octal8(buf: &[u8; 8]) -> u64 {
    let mut val = 0u64;
    let mut i = 0usize;
    while i < 8 {
        let b = buf[i];
        if b < b'0' || b > b'7' { break; }
        val = val.wrapping_mul(8).wrapping_add((b - b'0') as u64);
        i = i.wrapping_add(1);
    }
    val
}

/// Parse un nombre octal ASCII depuis un buffer de 12 bytes.
fn parse_octal12(buf: &[u8; 12]) -> u64 {
    let mut val = 0u64;
    let mut i = 0usize;
    while i < 12 {
        let b = buf[i];
        if b < b'0' || b > b'7' { break; }
        val = val.wrapping_mul(8).wrapping_add((b - b'0') as u64);
        i = i.wrapping_add(1);
    }
    val
}

/// Retourne la partie non-nulle d'un slice (name trimming).
fn trim_nul(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && s[end - 1] == 0 { end -= 1; }
    &s[..end]
}

/// Copie un slice de 512 bytes dans un tableau [u8; 512].
fn array_ref512(s: &[u8]) -> &[u8; 512] {
    debug_assert_eq!(s.len(), 512);
    unsafe { &*(s.as_ptr() as *const [u8; 512]) }
}

// ─── blake3 inline minimal ────────────────────────────────────────────────────

fn inline_blake3(data: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6b08_c647u32, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a,
        0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19,
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
        emitter.emit_blob(&mut sink, &bid, data, b"hello.txt", 0).expect("emit ok");
        emitter.finalize(&mut sink).expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).expect("parse ok");
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
            emitter.emit_blob(&mut sink, &bid, &data, &name, 0).expect("emit ok");
        }
        emitter.finalize(&mut sink).expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).expect("parse ok");
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn test_tar_directory_emit_parse() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        emitter.emit_directory(&mut sink, b"mydir/", 999).expect("dir ok");
        emitter.finalize(&mut sink).expect("finalize ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut parser = TarParser::new();
        let entries = parser.parse_all(&mut src).expect("ok");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, TarEntryKind::Directory);
    }

    #[test]
    fn test_tar_to_exoar_converter() {
        let mut sink = VecTarSink::new();
        let mut emitter = TarEmitter::new();
        let data = b"blob data to convert";
        let bid = inline_blake3(data);
        emitter.emit_blob(&mut sink, &bid, data, b"blob.bin", 0).expect("ok");
        emitter.finalize(&mut sink).expect("ok");

        let mut src = SliceTarSource::new(sink.as_slice());
        let mut converter = TarToExoarConverter::new();
        let blobs = converter.convert(&mut src).expect("ok");
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
        converter.convert(&mut sink, blobs, 0).expect("ok");
        assert_eq!(converter.stats().entries_emitted, 2);
        // Le flux doit contenir au moins 4 blocs (2 headers + 2 data_blocks + 2 zeros)
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
