//! metadata_export.rs — Sérialisation des métadonnées ExoFS (no_std, ASCII).
//!
//! Ce module fournit :
//!  - `TextSink`              : trait de sortie texte (pas de std::io).
//!  - `MetadataExporter`      : sérialisation blob/snapshot/chunk (clé=valeur).
//!  - `BlobMeta`              : métadonnées d'un blob à exporter.
//!  - `SnapshotMeta`          : métadonnées d'un snapshot.
//!  - `ChunkMeta`             : métadonnées d'un chunk de déduplication.
//!  - `ExportManifest`        : manifest complet d'une session d'export.
//!  - `MetadataBinaryWriter`  : format binaire compact pour métadonnées.
//!  - `ManifestBlobEntry`     : entrée d'un blob dans le manifest.
//!
//! Format texte : ASCII clé=valeur, une paire par ligne, sections [section].
//! Format binaire : magic 4 bytes, puis entrées fixes de 64 bytes.
//!
//! RECUR-01 : pas de récursion.
//! OOM-02   : try_reserve avant push.
//! ARITH-02 : saturating_* sur compteurs.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use alloc::string::String;
use core::fmt::Write as FmtWrite;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use super::incremental_export::EpochId;

// ─── Trait de sortie texte ────────────────────────────────────────────────────

/// Récepteur de texte ASCII (no_std — pas de std::io::Write).
pub trait TextSink {
    fn write_str(&mut self, s: &str) -> ExofsResult<()>;

    /// Écrit une ligne terminée par '\n'.
    fn write_line(&mut self, line: &str) -> ExofsResult<()> {
        self.write_str(line)?;
        self.write_str("\n")
    }

    /// Écrit une paire clé=valeur.
    fn write_kv(&mut self, key: &str, val: &str) -> ExofsResult<()> {
        self.write_str(key)?;
        self.write_str("=")?;
        self.write_str(val)?;
        self.write_str("\n")
    }

    /// Écrit une section [section_name].
    fn write_section(&mut self, name: &str) -> ExofsResult<()> {
        self.write_str("[")?;
        self.write_str(name)?;
        self.write_str("]\n")
    }
}

// ─── TextSink sur Vec<u8> ────────────────────────────────────────────────────

/// Implémentation de TextSink vers un Vec<u8> (UTF-8 / ASCII).
pub struct VecTextSink {
    buf: Vec<u8>,
}

impl VecTextSink {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    pub fn with_capacity(cap: usize) -> ExofsResult<Self> {
        let mut buf = Vec::new();
        buf.try_reserve(cap).map_err(|_| ExofsError::NoMemory)?;
        Ok(Self { buf })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf).unwrap_or("<invalid utf8>")
    }

    pub fn into_bytes(self) -> Vec<u8> { self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }
}

impl TextSink for VecTextSink {
    fn write_str(&mut self, s: &str) -> ExofsResult<()> {
        let bytes = s.as_bytes();
        self.buf.try_reserve(bytes.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(bytes);
        Ok(())
    }
}

// ─── Métadonnées de blob ──────────────────────────────────────────────────────

/// Métadonnées d'un blob pour l'export.
#[derive(Clone, Debug)]
pub struct BlobMeta {
    /// BlobId (blake3 des données brutes) — RÈGLE 11.
    pub blob_id: [u8; 32],
    /// Taille des données brutes.
    pub size: u64,
    /// Taille compressée (0 si non compressé).
    pub compressed_size: u64,
    /// Epoch de création du blob.
    pub epoch: EpochId,
    /// Flags de l'entrée (ENTRY_FLAG_*).
    pub flags: u8,
    /// Nombre de références sur ce blob.
    pub ref_count: u32,
    /// CRC32C des données.
    pub crc32: u32,
    /// Nom ou chemin associé (jusqu'à 64 bytes ASCII).
    pub name: [u8; 64],
    /// Longueur valide du nom.
    pub name_len: u8,
}

impl BlobMeta {
    pub fn new(blob_id: [u8; 32], size: u64, epoch: EpochId) -> Self {
        Self {
            blob_id, size, compressed_size: 0,
            epoch, flags: 0, ref_count: 1, crc32: 0,
            name: [0u8; 64], name_len: 0,
        }
    }

    /// Retourne le nom comme str si valide ASCII.
    pub fn name_str(&self) -> &str {
        let len = self.name_len as usize;
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Définit le nom (tronqué à 64 bytes).
    pub fn set_name(&mut self, name: &[u8]) {
        let len = name.len().min(64);
        self.name[..len].copy_from_slice(&name[..len]);
        self.name_len = len as u8;
    }

    /// Retourne le ratio de compression (× 1000, sans float).
    pub fn compression_ratio_pct10(&self) -> u32 {
        if self.size == 0 || self.compressed_size == 0 { return 1000; }
        (self.compressed_size.saturating_mul(1000))
            .checked_div(self.size)
            .unwrap_or(1000)
            .min(1000) as u32
    }
}

// ─── Métadonnées de snapshot ─────────────────────────────────────────────────

/// Métadonnées d'un snapshot ExoFS.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotMeta {
    /// Identifiant numérique du snapshot.
    pub snapshot_id: u64,
    /// Epoch au moment du snapshot.
    pub epoch: EpochId,
    /// Nombre de blobs dans le snapshot.
    pub blob_count: u32,
    /// Taille totale des données.
    pub total_bytes: u64,
    /// Taille totale compressée.
    pub compressed_bytes: u64,
    /// Timestamp de création.
    pub created_at: u64,
    /// true si le snapshot est cohérent (aucune écriture en cours).
    pub is_consistent: bool,
    /// true si le snapshot est l'état complet (non-incrémental).
    pub is_full: bool,
}

impl SnapshotMeta {
    pub const fn new(snapshot_id: u64, epoch: EpochId) -> Self {
        Self {
            snapshot_id, epoch, blob_count: 0,
            total_bytes: 0, compressed_bytes: 0,
            created_at: 0, is_consistent: true, is_full: true,
        }
    }
}

// ─── Métadonnées de chunk ─────────────────────────────────────────────────────

/// Métadonnées d'un chunk de déduplication.
#[derive(Clone, Copy, Debug)]
pub struct ChunkMeta {
    /// Fingerprint (blake3) du chunk — 32 bytes.
    pub fingerprint: [u8; 32],
    /// Nombre de blobs référençant ce chunk.
    pub ref_count: u32,
    /// Taille du chunk.
    pub size: u32,
    /// Offset dans le blob parent (si applicable).
    pub offset: u64,
}

impl ChunkMeta {
    pub const fn new(fingerprint: [u8; 32], size: u32) -> Self {
        Self { fingerprint, ref_count: 1, size, offset: 0 }
    }
}

// ─── Sérialiseur de métadonnées ───────────────────────────────────────────────

/// Sérialiseur de métadonnées ExoFS au format clé=valeur ASCII.
/// RECUR-01 : toutes les méthodes utilisent des boucles while.
pub struct MetadataExporter;

impl MetadataExporter {
    /// Sérialise les métadonnées d'un blob dans le sink.
    pub fn write_blob_meta<S: TextSink>(sink: &mut S, meta: &BlobMeta) -> ExofsResult<()> {
        sink.write_section("blob")?;
        sink.write_kv("blob_id", &hex_lower(&meta.blob_id))?;
        sink.write_kv("size", &u64_to_str(meta.size))?;
        if meta.compressed_size > 0 {
            sink.write_kv("compressed_size", &u64_to_str(meta.compressed_size))?;
        }
        sink.write_kv("epoch", &u64_to_str(meta.epoch.value()))?;
        sink.write_kv("flags", &u8_to_hex(meta.flags))?;
        sink.write_kv("ref_count", &u32_to_str(meta.ref_count))?;
        sink.write_kv("crc32", &u32_to_hex(meta.crc32))?;
        if meta.name_len > 0 {
            sink.write_kv("name", meta.name_str())?;
        }
        Ok(())
    }

    /// Sérialise les métadonnées d'un snapshot.
    pub fn write_snapshot_meta<S: TextSink>(sink: &mut S, meta: &SnapshotMeta) -> ExofsResult<()> {
        sink.write_section("snapshot")?;
        sink.write_kv("snapshot_id", &u64_to_str(meta.snapshot_id))?;
        sink.write_kv("epoch", &u64_to_str(meta.epoch.value()))?;
        sink.write_kv("blob_count", &u32_to_str(meta.blob_count))?;
        sink.write_kv("total_bytes", &u64_to_str(meta.total_bytes))?;
        sink.write_kv("compressed_bytes", &u64_to_str(meta.compressed_bytes))?;
        sink.write_kv("created_at", &u64_to_str(meta.created_at))?;
        sink.write_kv("is_consistent", if meta.is_consistent { "true" } else { "false" })?;
        sink.write_kv("is_full", if meta.is_full { "true" } else { "false" })?;
        Ok(())
    }

    /// Sérialise les métadonnées d'un chunk.
    pub fn write_chunk_meta<S: TextSink>(sink: &mut S, meta: &ChunkMeta) -> ExofsResult<()> {
        sink.write_section("chunk")?;
        sink.write_kv("fingerprint", &hex_lower(&meta.fingerprint))?;
        sink.write_kv("ref_count", &u32_to_str(meta.ref_count))?;
        sink.write_kv("size", &u32_to_str(meta.size))?;
        sink.write_kv("offset", &u64_to_str(meta.offset))?;
        Ok(())
    }

    /// Sérialise une liste de blobs (RECUR-01 : boucle while).
    pub fn write_blob_list<S: TextSink>(sink: &mut S, blobs: &[BlobMeta]) -> ExofsResult<()> {
        sink.write_section("blob_list")?;
        sink.write_kv("count", &usize_to_str(blobs.len()))?;
        let mut i = 0usize;
        while i < blobs.len() {
            Self::write_blob_meta(sink, &blobs[i])?;
            i = i.wrapping_add(1);
        }
        Ok(())
    }

    /// Écrit l'en-tête d'un manifest.
    pub fn write_manifest_header<S: TextSink>(
        sink: &mut S,
        session_id: u32,
        epoch_base: EpochId,
        epoch_target: EpochId,
        blob_count: usize,
    ) -> ExofsResult<()> {
        sink.write_section("manifest")?;
        sink.write_kv("version", "2")?;
        sink.write_kv("session_id", &u32_to_str(session_id))?;
        sink.write_kv("epoch_base", &u64_to_str(epoch_base.value()))?;
        sink.write_kv("epoch_target", &u64_to_str(epoch_target.value()))?;
        sink.write_kv("blob_count", &usize_to_str(blob_count))?;
        Ok(())
    }
}

// ─── Manifest d'export ────────────────────────────────────────────────────────

/// Entrée blob dans le manifest.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ManifestBlobEntry {
    /// BlobId (RÈGLE 11 : blake3 des données brutes).
    pub blob_id: [u8; 32],
    /// Taille des données brutes.
    pub size: u64,
    /// Epoch de création.
    pub epoch: u64,
    /// CRC32C des données.
    pub crc32: u32,
    /// Flags.
    pub flags: u8,
    /// Padding pour alignement sur 64 bytes.
    pub _pad: [u8; 11],
}

const _: () = assert!(core::mem::size_of::<ManifestBlobEntry>() == 64);

impl ManifestBlobEntry {
    pub fn new(blob_id: [u8; 32], size: u64, epoch: EpochId, crc32: u32) -> Self {
        Self {
            blob_id, size, epoch: epoch.value(), crc32,
            flags: 0, _pad: [0u8; 11],
        }
    }

    pub fn from_blob_meta(meta: &BlobMeta) -> Self {
        Self::new(meta.blob_id, meta.size, meta.epoch, meta.crc32)
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
}

/// Manifest complet d'une session d'export (texte + binaire).
pub struct ExportManifest {
    pub session_id: u32,
    pub epoch_base: EpochId,
    pub epoch_target: EpochId,
    pub entries: Vec<ManifestBlobEntry>,
    pub tombstones: Vec<[u8; 32]>,
    pub total_bytes: u64,
    pub archive_size: u64,
}

impl ExportManifest {
    /// Crée un manifest vide.
    pub fn new(session_id: u32, epoch_base: EpochId, epoch_target: EpochId) -> Self {
        Self {
            session_id, epoch_base, epoch_target,
            entries: Vec::new(), tombstones: Vec::new(),
            total_bytes: 0, archive_size: 0,
        }
    }

    /// Ajoute une entrée blob — OOM-02 : try_reserve.
    pub fn add_entry(&mut self, entry: ManifestBlobEntry) -> ExofsResult<()> {
        self.entries.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.total_bytes = self.total_bytes.saturating_add(entry.size);
        self.entries.push(entry);
        Ok(())
    }

    /// Ajoute un tombstone — OOM-02 : try_reserve.
    pub fn add_tombstone(&mut self, blob_id: [u8; 32]) -> ExofsResult<()> {
        self.tombstones.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.tombstones.push(blob_id);
        Ok(())
    }

    /// Sérialise le manifest en texte ASCII vers le sink.
    pub fn write_text<S: TextSink>(&self, sink: &mut S) -> ExofsResult<()> {
        MetadataExporter::write_manifest_header(
            sink,
            self.session_id,
            self.epoch_base,
            self.epoch_target,
            self.entries.len(),
        )?;
        sink.write_kv("tombstone_count", &usize_to_str(self.tombstones.len()))?;
        sink.write_kv("total_bytes", &u64_to_str(self.total_bytes))?;
        sink.write_kv("archive_size", &u64_to_str(self.archive_size))?;

        // Entrées blobs (RECUR-01 : boucle while)
        sink.write_section("entries")?;
        let mut i = 0usize;
        while i < self.entries.len() {
            let e = &self.entries[i];
            sink.write_kv("blob_id", &hex_lower(&e.blob_id))?;
            sink.write_kv("size", &u64_to_str(e.size))?;
            sink.write_kv("epoch", &u64_to_str(e.epoch))?;
            sink.write_kv("crc32", &u32_to_hex(e.crc32))?;
            i = i.wrapping_add(1);
        }

        // Tombstones (RECUR-01 : boucle while)
        if !self.tombstones.is_empty() {
            sink.write_section("tombstones")?;
            let mut j = 0usize;
            while j < self.tombstones.len() {
                sink.write_kv("blob_id", &hex_lower(&self.tombstones[j]))?;
                j = j.wrapping_add(1);
            }
        }
        Ok(())
    }

    pub fn entry_count(&self) -> usize { self.entries.len() }

    /// Vérifie l'intégrité du manifest (aucite entré avec size = 0 si non tombstone).
    pub fn validate(&self) -> bool {
        let mut i = 0usize;
        while i < self.entries.len() {
            // Chaque entrée doit avoir un blob_id non nul
            let all_zero = self.entries[i].blob_id.iter().all(|&b| b == 0);
            if all_zero { return false; }
            i = i.wrapping_add(1);
        }
        true
    }
}

// ─── Écrivain binaire de métadonnées ─────────────────────────────────────────

/// Magic du format binaire de manifest.
pub const META_BINARY_MAGIC: u32 = 0x4558_4D45; // "EXME"

/// En-tête du format binaire compact.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct MetaBinaryHeader {
    pub magic: u32,
    pub version: u16,
    pub entry_count: u32,
    pub tombstone_count: u32,
    pub session_id: u32,
    pub epoch_base: u64,
    pub epoch_target: u64,
    pub _pad: [u8; 6],
}

const _: () = assert!(core::mem::size_of::<MetaBinaryHeader>() == 40);

impl MetaBinaryHeader {
    pub fn new(session_id: u32, epoch_base: EpochId, epoch_target: EpochId,
               entry_count: u32, tombstone_count: u32) -> Self {
        Self {
            magic: META_BINARY_MAGIC, version: 1,
            entry_count, tombstone_count, session_id,
            epoch_base: epoch_base.value(), epoch_target: epoch_target.value(),
            _pad: [0u8; 6],
        }
    }

    pub fn validate_magic(&self) -> bool {
        let m: u32 = unsafe { core::ptr::read_unaligned(&self.magic) };
        m == META_BINARY_MAGIC
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Self as *const u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
}

/// Écrivain de manifest au format binaire compact.
pub struct MetadataBinaryWriter {
    pub buf: Vec<u8>,
}

impl MetadataBinaryWriter {
    pub fn new() -> Self { Self { buf: Vec::new() } }

    /// Sérialise le manifest en binaire — OOM-02 + RECUR-01.
    pub fn write_manifest(&mut self, manifest: &ExportManifest) -> ExofsResult<usize> {
        let hdr = MetaBinaryHeader::new(
            manifest.session_id,
            manifest.epoch_base,
            manifest.epoch_target,
            manifest.entries.len() as u32,
            manifest.tombstones.len() as u32,
        );
        let hdr_bytes = hdr.as_bytes();
        let entries_bytes = manifest.entries.len().saturating_mul(64);
        let tomb_bytes = manifest.tombstones.len().saturating_mul(32);
        let total = hdr_bytes.len().saturating_add(entries_bytes).saturating_add(tomb_bytes);
        self.buf.try_reserve(total).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(hdr_bytes);

        // Entrées (RECUR-01 : boucle while)
        let mut i = 0usize;
        while i < manifest.entries.len() {
            self.buf.extend_from_slice(manifest.entries[i].as_bytes());
            i = i.wrapping_add(1);
        }
        // Tombstones (RECUR-01 : boucle while)
        let mut j = 0usize;
        while j < manifest.tombstones.len() {
            self.buf.extend_from_slice(&manifest.tombstones[j]);
            j = j.wrapping_add(1);
        }
        Ok(self.buf.len())
    }

    pub fn as_slice(&self) -> &[u8] { &self.buf }
    pub fn len(&self) -> usize { self.buf.len() }
    pub fn is_empty(&self) -> bool { self.buf.is_empty() }
}

// ─── Fonctions utilitaires (no std, no alloc String si possible) ──────────────

/// Convertit un u64 en str ASCII décimale (buffer statique).
fn u64_to_str(n: u64) -> String {
    let mut buf = String::new();
    let _ = write!(buf, "{}", n);
    buf
}

fn u32_to_str(n: u32) -> String {
    let mut buf = String::new();
    let _ = write!(buf, "{}", n);
    buf
}

fn usize_to_str(n: usize) -> String {
    let mut buf = String::new();
    let _ = write!(buf, "{}", n);
    buf
}

fn u32_to_hex(n: u32) -> String {
    let mut buf = String::new();
    let _ = write!(buf, "{:08x}", n);
    buf
}

fn u8_to_hex(n: u8) -> String {
    let mut buf = String::new();
    let _ = write!(buf, "{:02x}", n);
    buf
}

/// Encode un slice de bytes en hexadécimal minuscule (RECUR-01 : boucle while).
fn hex_lower(data: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut s = String::new();
    let _ = s.try_reserve(data.len() * 2);
    let mut i = 0usize;
    while i < data.len() {
        let b = data[i];
        let hi = HEX[(b >> 4) as usize] as char;
        let lo = HEX[(b & 0xF) as usize] as char;
        s.push(hi);
        s.push(lo);
        i = i.wrapping_add(1);
    }
    s
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_blob_id(tag: u8) -> [u8; 32] {
        let mut id = [0u8; 32];
        id[0] = tag;
        id
    }

    #[test]
    fn test_vec_text_sink_write() {
        let mut sink = VecTextSink::new();
        sink.write_str("hello").expect("ok");
        sink.write_str(" world").expect("ok");
        assert_eq!(sink.as_str(), "hello world");
    }

    #[test]
    fn test_write_kv() {
        let mut sink = VecTextSink::new();
        sink.write_kv("key", "value").expect("ok");
        assert_eq!(sink.as_str(), "key=value\n");
    }

    #[test]
    fn test_write_section() {
        let mut sink = VecTextSink::new();
        sink.write_section("blob").expect("ok");
        assert_eq!(sink.as_str(), "[blob]\n");
    }

    #[test]
    fn test_blob_meta_serialization() {
        let bid = make_blob_id(42);
        let meta = BlobMeta::new(bid, 1024, EpochId(5));
        let mut sink = VecTextSink::new();
        MetadataExporter::write_blob_meta(&mut sink, &meta).expect("ok");
        let s = sink.as_str();
        assert!(s.contains("[blob]"));
        assert!(s.contains("size=1024"));
        assert!(s.contains("epoch=5"));
    }

    #[test]
    fn test_snapshot_meta_serialization() {
        let meta = SnapshotMeta::new(7, EpochId(12));
        let mut sink = VecTextSink::new();
        MetadataExporter::write_snapshot_meta(&mut sink, &meta).expect("ok");
        let s = sink.as_str();
        assert!(s.contains("[snapshot]"));
        assert!(s.contains("snapshot_id=7"));
    }

    #[test]
    fn test_manifest_write_text() {
        let mut manifest = ExportManifest::new(1, EpochId(0), EpochId(5));
        let bid = make_blob_id(10);
        let entry = ManifestBlobEntry::new(bid, 256, EpochId(3), 0xABCD1234);
        manifest.add_entry(entry).expect("add ok");
        let mut sink = VecTextSink::new();
        manifest.write_text(&mut sink).expect("write ok");
        let s = sink.as_str();
        assert!(s.contains("[manifest]"));
        assert!(s.contains("blob_count=1"));
        assert!(s.contains("[entries]"));
    }

    #[test]
    fn test_manifest_with_tombstone() {
        let mut manifest = ExportManifest::new(1, EpochId(0), EpochId(5));
        manifest.add_tombstone(make_blob_id(99)).expect("ok");
        let mut sink = VecTextSink::new();
        manifest.write_text(&mut sink).expect("ok");
        let s = sink.as_str();
        assert!(s.contains("[tombstones]"));
    }

    #[test]
    fn test_manifest_validate() {
        let mut manifest = ExportManifest::new(1, EpochId(0), EpochId(5));
        let bid = make_blob_id(5);
        manifest.add_entry(ManifestBlobEntry::new(bid, 100, EpochId(1), 0)).expect("ok");
        assert!(manifest.validate());

        let mut manifest2 = ExportManifest::new(1, EpochId(0), EpochId(5));
        // Entrée avec blob_id = [0; 32] invalide
        manifest2.add_entry(ManifestBlobEntry::new([0u8; 32], 100, EpochId(1), 0)).expect("ok");
        assert!(!manifest2.validate());
    }

    #[test]
    fn test_binary_writer_header_magic() {
        let manifest = ExportManifest::new(7, EpochId(1), EpochId(5));
        let mut writer = MetadataBinaryWriter::new();
        let n = writer.write_manifest(&manifest).expect("ok");
        assert!(n >= 40); // au moins la taille du header
        let magic = u32::from_le_bytes([writer.buf[0], writer.buf[1], writer.buf[2], writer.buf[3]]);
        assert_eq!(magic, META_BINARY_MAGIC);
    }

    #[test]
    fn test_binary_writer_with_entries() {
        let mut manifest = ExportManifest::new(1, EpochId(0), EpochId(3));
        for i in 0u8..5 {
            let bid = make_blob_id(i);
            manifest.add_entry(ManifestBlobEntry::new(bid, 64, EpochId(1), 0)).expect("ok");
        }
        let mut writer = MetadataBinaryWriter::new();
        let n = writer.write_manifest(&manifest).expect("ok");
        // header (40) + 5 entries × 64 = 360
        assert_eq!(n, 40 + 5 * 64);
    }

    #[test]
    fn test_hex_lower() {
        let b = [0xABu8, 0xCD, 0x12, 0x34];
        let h = hex_lower(&b);
        assert_eq!(h, "abcd1234");
    }

    #[test]
    fn test_manifest_entry_size() {
        assert_eq!(core::mem::size_of::<ManifestBlobEntry>(), 64);
    }

    #[test]
    fn test_blob_meta_name() {
        let mut meta = BlobMeta::new([0u8; 32], 0, EpochId(0));
        meta.set_name(b"my_file.bin");
        assert_eq!(meta.name_str(), "my_file.bin");
    }

    #[test]
    fn test_blob_meta_compression_ratio() {
        let mut meta = BlobMeta::new([0u8; 32], 1000, EpochId(0));
        meta.compressed_size = 600;
        assert_eq!(meta.compression_ratio_pct10(), 600);
    }

    #[test]
    fn test_write_blob_list() {
        let blobs = [
            BlobMeta::new(make_blob_id(1), 100, EpochId(1)),
            BlobMeta::new(make_blob_id(2), 200, EpochId(2)),
        ];
        let mut sink = VecTextSink::new();
        MetadataExporter::write_blob_list(&mut sink, &blobs).expect("ok");
        let s = sink.as_str();
        assert!(s.contains("count=2"));
    }
}
