//! snapshot_streaming.rs — Export en stream TLV d'un snapshot ExoFS
//!
//! Produit un flux d'octets découpé en chunks TLV (Tag-Length-Value).
//! Chaque chunk a un en-tête (HDR-03 : magic + checksum Blake3) et
//! transporte soit un blob, soit un manifeste de snapshot.
//!
//! Règles spec :
//!   HDR-03   : magic vérifié EN PREMIER dans StreamChunkHeader::verify()
//!   HASH-02  : payload checksum = Blake3 sur données RAW
//!   WRITE-02 : bytes_written vérifié après chaque push
//!   OOM-02   : try_reserve avant chaque push


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::fs::exofs::core::{ExofsError, ExofsResult, BlobId, SnapshotId};
use crate::fs::exofs::core::blob_id::{blake3_hash, compute_blob_id};
use super::snapshot::flags;
use super::snapshot_list::SNAPSHOT_LIST;

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

pub const STREAM_MAGIC: u32     = 0x5354_524D; // "STRM"
pub const STREAM_CHUNK_HDR_SIZE: usize = 64;

pub const CHUNK_TYPE_MANIFEST: u8 = 0x01;
pub const CHUNK_TYPE_BLOB:     u8 = 0x02;
pub const CHUNK_TYPE_END:      u8 = 0xFF;

pub const STREAM_FORMAT_VERSION: u8 = 1;

// ─────────────────────────────────────────────────────────────
// StreamChunkHeader (HDR-03)
// ─────────────────────────────────────────────────────────────

/// En-tête d'un chunk de stream (64 octets — ONDISK-03 : pas d'Atomic)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StreamChunkHeader {
    /// Magic "STRM" — vérifié EN PREMIER (HDR-03)
    pub magic:       u32,
    /// Version du format
    pub version:     u8,
    /// Type de chunk (MANIFEST, BLOB, END)
    pub chunk_type:  u8,
    /// _padding
    pub _pad0:       [u8; 2],
    /// Numéro de séquence
    pub seq:         u64,
    /// Identifiant du snapshot source
    pub snap_id:     u64,
    /// Identifiant Blake3 du blob transporté (ou racine si manifest)
    pub blob_id:     [u8; 32],
    /// Taille du payload (octets)
    pub payload_len: u32,
    /// _padding
    pub _pad1:       [u8; 4],
    /// Blake3 checksum de l'en-tête (sans les 4 derniers octets du slot)
    /// NB : champ checksum = hash des 60 premiers octets de l'en-tête
    pub checksum:    [u8; 4],
}

// const _SCH_SIZE: () = assert!(
//     core::mem::size_of::<StreamChunkHeader>() == STREAM_CHUNK_HDR_SIZE,
//     "StreamChunkHeader doit faire exactement 64 octets"
// );

impl StreamChunkHeader {
    /// Calcule le checksum Blake3 (4 premiers octets du hash sur les 60 premiers octets)
    fn compute_checksum(&self) -> [u8; 4] {
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille connue
        let body = unsafe { core::slice::from_raw_parts(ptr, STREAM_CHUNK_HDR_SIZE - 4) };
        let h = blake3_hash(body);
        [h[0], h[1], h[2], h[3]]
    }

    pub fn finalize(&mut self) {
        self.checksum = self.compute_checksum();
    }

    /// HDR-03 : magic EN PREMIER, puis checksum
    pub fn verify(&self) -> ExofsResult<()> {
        if self.magic != STREAM_MAGIC {
            return Err(ExofsError::BadMagic);
        }
        if self.version != STREAM_FORMAT_VERSION {
            return Err(ExofsError::InvalidArgument);
        }
        let expected = self.compute_checksum();
        let mut diff: u8 = 0;
        for i in 0..4 { diff |= expected[i] ^ self.checksum[i]; }
        if diff != 0 { return Err(ExofsError::ChecksumMismatch); }
        Ok(())
    }

    pub fn as_bytes(&self) -> &[u8] {
        let ptr = self as *const Self as *const u8;
        // SAFETY: repr(C), taille fixe
        unsafe { core::slice::from_raw_parts(ptr, STREAM_CHUNK_HDR_SIZE) }
    }
}

// ─────────────────────────────────────────────────────────────
// Trait StreamWriter
// ─────────────────────────────────────────────────────────────

/// Destination du stream de sortie
pub trait StreamWriter: Send + Sync {
    /// WRITE-02 : retourne bytes_written ; doit == data.len()
    fn write(&mut self, data: &[u8]) -> ExofsResult<usize>;
    fn flush(&mut self) -> ExofsResult<()>;
}

/// StreamWriter en mémoire (pour tests)
pub struct VecStreamWriter {
    pub buf: Vec<u8>,
}

impl VecStreamWriter {
    pub fn new() -> Self { Self { buf: Vec::new() } }
}

impl StreamWriter for VecStreamWriter {
    fn write(&mut self, data: &[u8]) -> ExofsResult<usize> {
        self.buf.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        self.buf.extend_from_slice(data);
        Ok(data.len()) // WRITE-02 : retour correct
    }
    fn flush(&mut self) -> ExofsResult<()> { Ok(()) }
}

// ─────────────────────────────────────────────────────────────
// Options de streaming
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct StreamOptions {
    /// Taille max d'un chunk de payload (octets)
    pub chunk_size:  usize,
    /// Inclure le manifest du snapshot en premier
    pub with_manifest: bool,
    /// Envoyer un chunk END en fin de stream
    pub with_end_marker: bool,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self { chunk_size: 64 * 1024, with_manifest: true, with_end_marker: true }
    }
}

// ─────────────────────────────────────────────────────────────
// Résultat du streaming
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StreamResult {
    pub snap_id:      SnapshotId,
    pub n_chunks:     u64,
    pub n_blobs:      u64,
    pub bytes_sent:   u64,
    pub aborted:      bool,
}

// ─────────────────────────────────────────────────────────────
// Source de blobs pour le streaming
// ─────────────────────────────────────────────────────────────

pub trait StreamBlobSource: Send + Sync {
    fn list_blobs(&self, snap_id: SnapshotId) -> ExofsResult<Vec<BlobId>>;
    fn read_blob(&self, snap_id: SnapshotId, blob_id: BlobId) -> ExofsResult<Vec<u8>>;
}

/// Source en mémoire (pour tests)
pub struct MemStreamBlobSource {
    data: alloc::collections::BTreeMap<[u8; 32], Vec<u8>>,
    snap_blobs: alloc::collections::BTreeMap<u64, Vec<BlobId>>,
}

impl MemStreamBlobSource {
    pub fn new() -> Self {
        Self { data: alloc::collections::BTreeMap::new(), snap_blobs: alloc::collections::BTreeMap::new() }
    }
    pub fn add(&mut self, snap_id: SnapshotId, data: &[u8]) -> ExofsResult<BlobId> {
        let bid = compute_blob_id(data);
        self.data.insert(*bid.as_bytes(), data.to_vec());
        let list = self.snap_blobs.entry(snap_id.0).or_insert_with(Vec::new);
        list.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        list.push(bid);
        Ok(bid)
    }
}

impl StreamBlobSource for MemStreamBlobSource {
    fn list_blobs(&self, snap_id: SnapshotId) -> ExofsResult<Vec<BlobId>> {
        Ok(self.snap_blobs.get(&snap_id.0).cloned().unwrap_or_default())
    }
    fn read_blob(&self, _: SnapshotId, blob_id: BlobId) -> ExofsResult<Vec<u8>> {
        self.data.get(blob_id.as_bytes()).cloned().ok_or(ExofsError::NotFound)
    }
}

// ─────────────────────────────────────────────────────────────
// SnapshotStreamer
// ─────────────────────────────────────────────────────────────

pub struct SnapshotStreamer {
    seq:      AtomicU64,
    aborted:  AtomicBool,
}

impl SnapshotStreamer {
    pub const fn new() -> Self {
        Self { seq: AtomicU64::new(0), aborted: AtomicBool::new(false) }
    }

    // ── Point d'entrée ───────────────────────────────────────────────

    pub fn stream<S: StreamBlobSource, W: StreamWriter>(
        &self,
        snap_id: SnapshotId,
        source:  &S,
        writer:  &mut W,
        opts:    StreamOptions,
    ) -> ExofsResult<StreamResult> {
        // Vérifier que le snapshot existe
        let snap = SNAPSHOT_LIST.get(snap_id)?;

        // Marquer comme en streaming
        SNAPSHOT_LIST.set_flags(snap_id, flags::STREAMING)?;

        let result = self.run_stream(snap_id, &snap, source, writer, opts);

        let _ = SNAPSHOT_LIST.clear_flags(snap_id, flags::STREAMING);
        result
    }

    fn run_stream<S: StreamBlobSource, W: StreamWriter>(
        &self,
        snap_id: SnapshotId,
        snap:    &super::snapshot::Snapshot,
        source:  &S,
        writer:  &mut W,
        opts:    StreamOptions,
    ) -> ExofsResult<StreamResult> {
        let mut result = StreamResult {
            snap_id, n_chunks: 0, n_blobs: 0, bytes_sent: 0, aborted: false,
        };

        // ── Manifest ─────────────────────────────────────────────────
        if opts.with_manifest {
            let bytes = self.send_manifest(snap_id, snap, writer)?;
            result.bytes_sent = result.bytes_sent.saturating_add(bytes as u64);
            result.n_chunks   = result.n_chunks.checked_add(1).ok_or(ExofsError::Overflow)?;
        }

        // ── Blobs ─────────────────────────────────────────────────────
        let blob_ids = source.list_blobs(snap_id)?;
        for blob_id in &blob_ids {
            if self.aborted.load(Ordering::Acquire) {
                result.aborted = true;
                return Ok(result);
            }
            let data = source.read_blob(snap_id, *blob_id)?;
            let bytes = self.send_blob(snap_id, *blob_id, &data, writer, &opts)?;
            result.bytes_sent = result.bytes_sent.saturating_add(bytes as u64);
            result.n_chunks   = result.n_chunks.checked_add(1).ok_or(ExofsError::Overflow)?;
            result.n_blobs    = result.n_blobs.checked_add(1).ok_or(ExofsError::Overflow)?;
        }

        // ── Fin ───────────────────────────────────────────────────────
        if opts.with_end_marker {
            let bytes = self.send_end(snap_id, writer)?;
            result.bytes_sent = result.bytes_sent.saturating_add(bytes as u64);
            result.n_chunks   = result.n_chunks.checked_add(1).ok_or(ExofsError::Overflow)?;
        }

        writer.flush()?;
        Ok(result)
    }

    // ── Envoi d'un manifest ──────────────────────────────────────────

    fn send_manifest<W: StreamWriter>(
        &self,
        snap_id: SnapshotId,
        snap: &super::snapshot::Snapshot,
        writer: &mut W,
    ) -> ExofsResult<usize> {
        // Payload : root_blob (32) + n_blobs (8) + total_bytes (8) = 48 octets
        let mut payload: Vec<u8> = Vec::new();
        payload.try_reserve(48).map_err(|_| ExofsError::NoMemory)?;
        payload.extend_from_slice(snap.root_blob.as_bytes());
        payload.extend_from_slice(&snap.n_blobs.to_le_bytes());
        payload.extend_from_slice(&snap.total_bytes.to_le_bytes());

        let hdr = self.build_header(snap_id, CHUNK_TYPE_MANIFEST, snap.root_blob, payload.len() as u32);
        let total = self.write_chunk(&hdr, &payload, writer)?;
        Ok(total)
    }

    // ── Envoi d'un blob ──────────────────────────────────────────────

    fn send_blob<W: StreamWriter>(
        &self,
        snap_id: SnapshotId,
        blob_id: BlobId,
        data: &[u8],
        writer:  &mut W,
        opts: &StreamOptions,
    ) -> ExofsResult<usize> {
        // Pour les gros blobs on peut découper en sous-chunks
        let mut total_sent = 0usize;
        let mut offset = 0usize;
        while offset < data.len() || data.is_empty() {
            let end = (offset + opts.chunk_size).min(data.len());
            let chunk_data = &data[offset..end];
            let hdr = self.build_header(snap_id, CHUNK_TYPE_BLOB, blob_id, chunk_data.len() as u32);
            let n = self.write_chunk(&hdr, chunk_data, writer)?;
            total_sent = total_sent.saturating_add(n);
            offset = end;
            if data.is_empty() { break; }
        }
        Ok(total_sent)
    }

    // ── Envoi du marqueur END ────────────────────────────────────────

    fn send_end<W: StreamWriter>(&self, snap_id: SnapshotId, writer: &mut W) -> ExofsResult<usize> {
        let hdr = self.build_header(snap_id, CHUNK_TYPE_END, BlobId([0u8; 32]), 0);
        self.write_chunk(&hdr, &[], writer)
    }

    // ── Construction de l'en-tête ────────────────────────────────────

    fn build_header(&self, snap_id: SnapshotId, chunk_type: u8, blob_id: BlobId, payload_len: u32) -> StreamChunkHeader {
        let seq = self.seq.fetch_add(1, Ordering::AcqRel);
        let mut hdr = StreamChunkHeader {
            magic: STREAM_MAGIC,
            version: STREAM_FORMAT_VERSION,
            chunk_type,
            _pad0: [0u8; 2],
            seq,
            snap_id: snap_id.0,
            blob_id: *blob_id.as_bytes(),
            payload_len,
            _pad1: [0u8; 4],
            checksum: [0u8; 4],
        };
        hdr.finalize();
        hdr
    }

    // ── Écriture d'un chunk ──────────────────────────────────────────

    /// WRITE-02 : vérifie bytes_written après chaque write
    fn write_chunk<W: StreamWriter>(
        &self,
        hdr:     &StreamChunkHeader,
        payload: &[u8],
        writer:  &mut W,
    ) -> ExofsResult<usize> {
        // Écrire l'en-tête
        let hdr_bytes = hdr.as_bytes();
        let n = writer.write(hdr_bytes)?;
        if n != hdr_bytes.len() { return Err(ExofsError::ShortWrite); } // WRITE-02

        // Écrire le payload
        let mut total = n;
        if !payload.is_empty() {
            let m = writer.write(payload)?;
            if m != payload.len() { return Err(ExofsError::ShortWrite); } // WRITE-02
            total = total.saturating_add(m);
        }
        Ok(total)
    }

    // ── Annulation ───────────────────────────────────────────────────

    pub fn abort(&self) { self.aborted.store(true, Ordering::Release); }
    pub fn reset(&self) { self.aborted.store(false, Ordering::Release); }
    pub fn is_aborted(&self) -> bool { self.aborted.load(Ordering::Acquire) }
}

// ─────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::exofs::core::{BlobId, DiskOffset, EpochId, SnapshotId};
    use crate::fs::exofs::core::blob_id::compute_blob_id;
    use super::super::snapshot::{Snapshot, make_snapshot_name};
    use super::super::snapshot_list::SnapshotList;

    fn push_snap(list: &SnapshotList, id: u64, n_blobs: u64) {
        let root = BlobId([id as u8; 32]);
        list.register(Snapshot {
            id: SnapshotId(id), epoch_id: EpochId(1), parent_id: None,
            root_blob: root, created_at: 0, n_blobs,
            total_bytes: 0, flags: 0,
            blob_catalog_offset: DiskOffset(0), blob_catalog_size: 0,
            name: make_snapshot_name(b"stream-test"),
        }).unwrap();
    }

    #[test]
    fn stream_chunk_header_size() {
        assert_eq!(core::mem::size_of::<StreamChunkHeader>(), STREAM_CHUNK_HDR_SIZE);
    }

    #[test]
    fn stream_chunk_header_roundtrip() {
        let snap_id = SnapshotId(42);
        let bid = BlobId([0xAA; 32]);
        let streamer = SnapshotStreamer::new();
        let hdr = streamer.build_header(snap_id, CHUNK_TYPE_BLOB, bid, 128);
        assert!(hdr.verify().is_ok());
    }

    #[test]
    fn stream_bad_magic_detected() {
        let snap_id = SnapshotId(1);
        let bid = BlobId([0u8; 32]);
        let streamer = SnapshotStreamer::new();
        let mut hdr = streamer.build_header(snap_id, CHUNK_TYPE_BLOB, bid, 0);
        hdr.magic = 0xDEAD_BEEF;
        assert!(matches!(hdr.verify(), Err(ExofsError::BadMagic)));
    }

    #[test]
    fn stream_produces_output() {
        let list = SnapshotList::new_const();
        push_snap(&list, 1, 1);
        let mut source = MemStreamBlobSource::new();
        source.add(SnapshotId(1), b"hello kernel").unwrap();
        let streamer = SnapshotStreamer::new();
        let mut writer = VecStreamWriter::new();
        let result = streamer.stream(SnapshotId(1), &source, &mut writer, StreamOptions::default()).unwrap();
        assert!(result.bytes_sent > 0);
        // Au moins : manifest + 1 blob + END = 3 chunks
        assert!(result.n_chunks >= 3);
    }

    #[test]
    fn abort_stops_stream() {
        let streamer = SnapshotStreamer::new();
        streamer.abort();
        assert!(streamer.is_aborted());
        streamer.reset();
        assert!(!streamer.is_aborted());
    }
}
