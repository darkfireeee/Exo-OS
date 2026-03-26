//! blob_writer.rs — Pipeline complet d'écriture de blobs ExoFS
//!
//! Pipeline : raw_data → BlobId(Blake3) → dédup → compression → chiffrement(XChaCha20) → checksum → disque
//!
//! Règles spec :
//!   HASH-02 : BlobId calculé sur données RAW, AVANT toute compression
//!   WRITE-02 : vérification bytes_written == expected après chaque écriture
//!   HDR-03   : en-tête BlobHeader avec magic + checksum AVANT payload
//!   OOM-02   : try_reserve(1) avant chaque Vec::push
//!   ARITH-02 : checked_add/mul pour toute arithmétique


extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, BlobId, DiskOffset, EpochId,
};
use crate::fs::exofs::core::blob_id::compute_blob_id;
use crate::fs::exofs::crypto::key_derivation::KeyDerivation;
use crate::fs::exofs::crypto::secret_writer::SecretWriter;
use crate::fs::exofs::storage::storage_stats::STORAGE_STATS;
use crate::fs::exofs::storage::layout::{BLOCK_SIZE, align_up};
use crate::fs::exofs::storage::compression_choice::{
    choose_compression, CompressionType, ContentHint,
};
use crate::fs::exofs::storage::compression_writer::CompressWriter;

// ─────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────

/// Magic number de l'en-tête blob sur disque : "EXBL"
pub const BLOB_HEADER_MAGIC: u32 = 0x4558_424C;

/// Taille fixe de l'en-tête blob : 64 octets (aligné sur 8B)
pub const BLOB_HEADER_SIZE: usize = 64;

/// Version courante du format
pub const BLOB_FORMAT_VERSION: u8 = 1;

/// Taille minimale pour déclencher la compression
const COMPRESS_MIN_BYTES: usize = 256;

/// Taille maximale d'un blob (512 MiB)
pub const BLOB_MAX_SIZE: usize = 512 * 1024 * 1024;

/// Flag header : payload chiffré.
const BLOB_FLAG_ENCRYPTED: u8 = 0b0000_0100;

// ─────────────────────────────────────────────────────────────
// Structures disque (repr C, pas d'AtomicXxx — ONDISK-03)
// ─────────────────────────────────────────────────────────────

/// En-tête blob stocké sur disque (BLOB_HEADER_SIZE = 64B)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BlobHeaderDisk {
    /// Magic "EXBL"
    pub magic: u32,
    /// Version format
    pub version: u8,
    /// Algorithme de compression (0=None, 1=Lz4, 2=Zstd)
    pub compression_algo: u8,
    /// Flags (bit0 = dédup_hit, bit1 = checksum présent)
    pub flags: u8,
    /// Réservé
    pub _reserved0: u8,
    /// Taille originale (avant compression)
    pub original_size: u32,
    /// Taille stockée (après compression)
    pub stored_size: u32,
    /// Époque de création
    pub epoch: u64,
    /// BlobId (Blake3 sur données RAW — HASH-02)
    pub blob_id: [u8; 32],
    /// Checksum de CET en-tête (Blake3 sur les 56 premiers octets)
    pub header_checksum: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<BlobHeaderDisk>() == BLOB_HEADER_SIZE,
    "BlobHeaderDisk doit faire exactement 64 octets"
);

impl BlobHeaderDisk {
    /// Vérifie le checksum d'en-tête (HDR-03)
    pub fn verify_header_checksum(&self) -> bool {
        let raw = self.as_bytes();
        // Checksum = 4 premiers octets du Blake3 sur les 60 premiers bytes
        let h = crate::fs::exofs::core::blob_id::blake3_hash(&raw[..60]);
        self.header_checksum == [h[0], h[1], h[2], h[3]]
    }

    /// Construit le checksum d'en-tête
    pub fn compute_header_checksum(raw60: &[u8; 60]) -> [u8; 4] {
        let h = crate::fs::exofs::core::blob_id::blake3_hash(raw60);
        [h[0], h[1], h[2], h[3]]
    }

    /// Accès brut (pour calcul checksum)
    fn as_bytes(&self) -> [u8; BLOB_HEADER_SIZE] {
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        unsafe { core::mem::transmute(*self) }
    }
}

// ─────────────────────────────────────────────────────────────
// Configuration du writer
// ─────────────────────────────────────────────────────────────

/// Configuration pour écriture d'un blob
#[derive(Clone)]
pub struct BlobWriterConfig {
    /// Algorithme de compression forcé (None = auto-detect)
    pub forced_algo: Option<CompressionType>,
    /// Indice de contenu pour l'auto-détection
    pub hint: ContentHint,
    /// Activer la déduplication
    pub dedup_enabled: bool,
    /// Vérifier l'intégrité après écriture (read-back)
    pub verify_after_write: bool,
    /// Époque courante
    pub epoch: EpochId,
}

impl Default for BlobWriterConfig {
    fn default() -> Self {
        Self {
            forced_algo: None,
            hint: ContentHint::Unknown,
            dedup_enabled: true,
            verify_after_write: false,
            epoch: EpochId(0),
        }
    }
}

impl BlobWriterConfig {
    pub fn new(epoch: EpochId) -> Self {
        Self { epoch, ..Default::default() }
    }

    pub fn with_algo(mut self, algo: CompressionType) -> Self {
        self.forced_algo = Some(algo);
        self
    }

    pub fn with_hint(mut self, hint: ContentHint) -> Self {
        self.hint = hint;
        self
    }

    pub fn no_dedup(mut self) -> Self {
        self.dedup_enabled = false;
        self
    }

    pub fn verify(mut self) -> Self {
        self.verify_after_write = true;
        self
    }
}

// ─────────────────────────────────────────────────────────────
// Résultat d'écriture
// ─────────────────────────────────────────────────────────────

/// Résultat d'une écriture de blob
#[derive(Debug, Clone)]
pub struct BlobWriteResult {
    /// Identifiant calculé sur données RAW (HASH-02)
    pub blob_id: BlobId,
    /// Offset disque où le blob est stocké
    pub offset: DiskOffset,
    /// Taille totale occupée sur disque (header + payload)
    pub disk_size: u64,
    /// Taille originale (avant compression)
    pub original_size: u64,
    /// Taille stockée (après compression)
    pub stored_size: u64,
    /// Algorithme de compression effectivement utilisé
    pub algo: CompressionType,
    /// Vrai si un blob identique existait déjà (dédup)
    pub dedup_hit: bool,
    /// Époque d'écriture
    pub epoch: EpochId,
    /// Nombre de blocs physiques occupés
    pub blocks_used: u64,
}

impl BlobWriteResult {
    /// Ratio de compression (0.0 = pas de compression, 1.0 = tout compressé)
    pub fn compression_ratio(&self) -> f32 {
        if self.original_size == 0 { return 0.0; }
        1.0 - (self.stored_size as f32 / self.original_size as f32)
    }

    /// Efficacité de stockage en pourcentage
    pub fn storage_efficiency_pct(&self) -> u64 {
        if self.original_size == 0 { return 0; }
        let ratio = self.stored_size.saturating_mul(100) / self.original_size;
        100u64.saturating_sub(ratio)
    }
}

// ─────────────────────────────────────────────────────────────
// Contexte d'écriture interne
// ─────────────────────────────────────────────────────────────

/// Contexte intermédiaire après compression
struct WriteContext {
    #[allow(dead_code)]
    blob_id: BlobId,
    original_size: u32,
    stored_size: u32,
    algo: CompressionType,
    payload: Vec<u8>,
    encrypted: bool,
}

// ─────────────────────────────────────────────────────────────
// Statistiques globales du writer
// ─────────────────────────────────────────────────────────────

/// Compteurs internes du BlobWriter (thread-safe)
pub struct BlobWriterStats {
    pub total_writes: AtomicU64,
    pub total_bytes_raw: AtomicU64,
    pub total_bytes_stored: AtomicU64,
    pub dedup_hits: AtomicU64,
    pub compress_ops: AtomicU64,
    pub write_errors: AtomicU64,
    pub header_writes: AtomicU64,
}

/// Instance globale
pub static BLOB_WRITER_STATS: BlobWriterStats = BlobWriterStats {
    total_writes: AtomicU64::new(0),
    total_bytes_raw: AtomicU64::new(0),
    total_bytes_stored: AtomicU64::new(0),
    dedup_hits: AtomicU64::new(0),
    compress_ops: AtomicU64::new(0),
    write_errors: AtomicU64::new(0),
    header_writes: AtomicU64::new(0),
};

impl BlobWriterStats {
    pub fn snapshot(&self) -> BlobWriterStatsSnapshot {
        BlobWriterStatsSnapshot {
            total_writes: self.total_writes.load(Ordering::Relaxed),
            total_bytes_raw: self.total_bytes_raw.load(Ordering::Relaxed),
            total_bytes_stored: self.total_bytes_stored.load(Ordering::Relaxed),
            dedup_hits: self.dedup_hits.load(Ordering::Relaxed),
            compress_ops: self.compress_ops.load(Ordering::Relaxed),
            write_errors: self.write_errors.load(Ordering::Relaxed),
            header_writes: self.header_writes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlobWriterStatsSnapshot {
    pub total_writes: u64,
    pub total_bytes_raw: u64,
    pub total_bytes_stored: u64,
    pub dedup_hits: u64,
    pub compress_ops: u64,
    pub write_errors: u64,
    pub header_writes: u64,
}

// ─────────────────────────────────────────────────────────────
// BlobWriter principal
// ─────────────────────────────────────────────────────────────

/// Writer de blobs ExoFS — pipeline complet
///
/// Utilisation :
/// ```no_run
/// let result = BlobWriter::write_blob(data, &config, alloc_fn, write_fn)?;
/// ```
pub struct BlobWriter;

impl BlobWriter {
    /// Point d'entrée principal : écrit un blob complet sur disque.
    ///
    /// Pipeline (dans l'ordre) :
    /// 1. Validation des entrées
    /// 2. Calcul BlobId sur données RAW (HASH-02)
    /// 3. Vérification déduplication  
    /// 4. Sélection et application de la compression
    /// 5. Construction et écriture de l'en-tête (HDR-03)
    /// 6. Écriture du payload compressé (WRITE-02)
    /// 7. Mise à jour des statistiques
    ///
    /// # Paramètres
    /// - `data` : données brutes du blob
    /// - `config` : configuration d'écriture
    /// - `alloc_fn` : function d'allocation → (`offset`, `size_in_blocks`)
    /// - `write_fn` : function d'écriture physique → `bytes_written`
    /// - `dedup_check` : vérifie si BlobId déjà présent → Option<DiskOffset>
    pub fn write_blob<AllocFn, WriteFn, DedupFn>(
        data: &[u8],
        config: &BlobWriterConfig,
        alloc_fn: AllocFn,
        write_fn: WriteFn,
        dedup_check: DedupFn,
    ) -> ExofsResult<BlobWriteResult>
    where
        AllocFn: FnOnce(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
        DedupFn: FnOnce(&BlobId) -> Option<DiskOffset>,
    {
        // ── 1. Validation ─────────────────────────────────────────────
        if data.is_empty() {
            return Err(ExofsError::InvalidArgument);
        }
        if data.len() > BLOB_MAX_SIZE {
            return Err(ExofsError::InvalidSize);
        }

        // ── 2. BlobId sur données RAW — HASH-02 ───────────────────────
        let blob_id = compute_blob_id(data);

        // ── 3. Déduplication ──────────────────────────────────────────
        if config.dedup_enabled {
            if let Some(existing_offset) = dedup_check(&blob_id) {
                BLOB_WRITER_STATS.dedup_hits.fetch_add(1, Ordering::Relaxed);
                STORAGE_STATS.inc_dedup_hit(data.len() as u64);
                let n_blocks = Self::size_to_blocks(
                    BLOB_HEADER_SIZE.checked_add(data.len())
                        .ok_or(ExofsError::Overflow)? as u64
                );
                return Ok(BlobWriteResult {
                    blob_id,
                    offset: existing_offset,
                    disk_size: n_blocks.saturating_mul(BLOCK_SIZE as u64),
                    original_size: data.len() as u64,
                    stored_size: data.len() as u64,
                    algo: CompressionType::None,
                    dedup_hit: true,
                    epoch: config.epoch,
                    blocks_used: n_blocks,
                });
            }
            STORAGE_STATS.inc_dedup_miss();
        }

        // ── 4. Compression ────────────────────────────────────────────
        let ctx = Self::compress_data(data, config)?;

        // ── 5 + 6. Allocation + écriture ──────────────────────────────
        let result = Self::write_to_disk(ctx, blob_id, config, alloc_fn, write_fn)?;

        // ── 7. Statistiques ───────────────────────────────────────────
        BLOB_WRITER_STATS.total_writes.fetch_add(1, Ordering::Relaxed);
        BLOB_WRITER_STATS.total_bytes_raw
            .fetch_add(result.original_size, Ordering::Relaxed);
        BLOB_WRITER_STATS.total_bytes_stored
            .fetch_add(result.stored_size, Ordering::Relaxed);
        STORAGE_STATS.add_write(result.disk_size);
        STORAGE_STATS.inc_blob_created();

        Ok(result)
    }

    // ── Étape compression ───────────────────────────────────────────────

    fn compress_data(data: &[u8], config: &BlobWriterConfig) -> ExofsResult<WriteContext> {
        // Calcul BlobId AVANT compression (HASH-02)
        let blob_id = compute_blob_id(data);
        let original_size = data.len() as u32;

        // Sélection de l'algorithme
        let algo = if let Some(forced) = config.forced_algo {
            forced
        } else if data.len() < COMPRESS_MIN_BYTES {
            CompressionType::None
        } else {
            choose_compression(data, config.hint).algorithm
        };

        let (payload, effective_algo) = if algo == CompressionType::None {
            // Pas de compression : copie directe
            let mut v = Vec::new();
            v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
            v.extend_from_slice(data);
            (v, CompressionType::None)
        } else {
            // Compression
            BLOB_WRITER_STATS.compress_ops.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_compress_op();
            STORAGE_STATS.add_compress_bytes_in(data.len() as u64);

            let compressed = CompressWriter::new(algo).compress(data)?;

            // N'utiliser la compression que si elle réduit la taille
            if compressed.len() < data.len() {
                STORAGE_STATS.add_compress_bytes_out(compressed.len() as u64);
                (compressed.data, algo)
            } else {
                // Compression inefficace → stocker brut
                let mut v = Vec::new();
                v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
                v.extend_from_slice(data);
                STORAGE_STATS.add_compress_bytes_out(data.len() as u64);
                (v, CompressionType::None)
            }
        };

        // CRYPTO-02 : chiffrement APRÈS compression, AVANT écriture disque.
        let encrypted_payload = Self::encrypt_payload(&blob_id, &payload)?;
        let stored_size = encrypted_payload.len() as u32;

        Ok(WriteContext {
            blob_id,
            original_size,
            stored_size,
            algo: effective_algo,
            payload: encrypted_payload,
            encrypted: true,
        })
    }

    fn derive_blob_payload_key(blob_id: &BlobId) -> ExofsResult<[u8; 32]> {
        let dk = KeyDerivation::derive_key(
            &blob_id.0,
            b"exofs-blob-payload-salt-v1",
            b"exofs-blob-payload-key-v1",
        )?;
        Ok(*dk.as_bytes())
    }

    fn encrypt_payload(blob_id: &BlobId, payload: &[u8]) -> ExofsResult<Vec<u8>> {
        let key = Self::derive_blob_payload_key(blob_id)?;
        SecretWriter::new(&key).encrypt(payload)
    }

    /// Dérive la clé utilisée pour chiffrer le payload d'un blob.
    ///
    /// Exposée pour la symétrie avec le pipeline de lecture (`BlobReader`).
    pub(crate) fn payload_key_for(blob_id: &BlobId) -> ExofsResult<[u8; 32]> {
        Self::derive_blob_payload_key(blob_id)
    }

    // ── Étape écriture disque ───────────────────────────────────────────

    fn write_to_disk<AllocFn, WriteFn>(
        ctx: WriteContext,
        blob_id: BlobId,
        config: &BlobWriterConfig,
        alloc_fn: AllocFn,
        mut write_fn: WriteFn,
    ) -> ExofsResult<BlobWriteResult>
    where
        AllocFn: FnOnce(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
    {
        // Taille totale à allouer (header + payload aligné sur BLOCK_SIZE)
        let total_raw = BLOB_HEADER_SIZE
            .checked_add(ctx.payload.len())
            .ok_or(ExofsError::Overflow)?;
        let total_aligned = align_up(DiskOffset(total_raw as u64), BLOCK_SIZE as u64)?.0;
        let n_blocks = Self::size_to_blocks(total_aligned);

        // Allocation
        let base_offset = alloc_fn(n_blocks)?;

        // Construction de l'en-tête (HDR-03)
        let header = Self::build_header(&ctx, blob_id, config)?;
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let hdr_bytes: [u8; BLOB_HEADER_SIZE] = unsafe { core::mem::transmute(header) };

        // Écriture de l'en-tête (WRITE-02)
        let written_hdr = write_fn(base_offset, &hdr_bytes)?;
        if written_hdr != BLOB_HEADER_SIZE {
            BLOB_WRITER_STATS.write_errors.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_io_error();
            return Err(ExofsError::ShortWrite);
        }
        BLOB_WRITER_STATS.header_writes.fetch_add(1, Ordering::Relaxed);

        // Offset du payload = base + BLOB_HEADER_SIZE
        let payload_offset = DiskOffset(
            base_offset.0
                .checked_add(BLOB_HEADER_SIZE as u64)
                .ok_or(ExofsError::Overflow)?
        );

        // Écriture du payload (WRITE-02)
        let written_payload = write_fn(payload_offset, &ctx.payload)?;
        if written_payload != ctx.payload.len() {
            BLOB_WRITER_STATS.write_errors.fetch_add(1, Ordering::Relaxed);
            STORAGE_STATS.inc_io_error();
            return Err(ExofsError::ShortWrite);
        }

        let disk_size = total_aligned;
        let original_size = ctx.original_size as u64;
        let stored_size = ctx.stored_size as u64;
        let algo = ctx.algo;

        Ok(BlobWriteResult {
            blob_id,
            offset: base_offset,
            disk_size,
            original_size,
            stored_size,
            algo,
            dedup_hit: false,
            epoch: config.epoch,
            blocks_used: n_blocks,
        })
    }

    // ── Construction de l'en-tête (HDR-03) ─────────────────────────────

    fn build_header(
        ctx: &WriteContext,
        blob_id: BlobId,
        config: &BlobWriterConfig,
    ) -> ExofsResult<BlobHeaderDisk> {
        let algo_byte: u8 = match ctx.algo {
            CompressionType::None => 0,
            CompressionType::Lz4  => 1,
            CompressionType::Zstd => 2,
        };

        let mut flags: u8 = 0b0000_0011; // checksum présent, dédup possible
        if ctx.encrypted {
            flags |= BLOB_FLAG_ENCRYPTED;
        }
        let epoch = config.epoch.0;

        // Construit les 60 premiers octets pour le checksum d'en-tête
        let mut hdr = BlobHeaderDisk {
            magic: BLOB_HEADER_MAGIC,
            version: BLOB_FORMAT_VERSION,
            compression_algo: algo_byte,
            flags,
            _reserved0: 0,
            original_size: ctx.original_size,
            stored_size: ctx.stored_size,
            epoch,
            blob_id: blob_id.0,
            header_checksum: [0u8; 4],
        };

        // Calcul du checksum sur les 60 premiers octets (avant injection)
        // SAFETY: cast byte-by-byte d'une struct #[repr(C, packed)] — taille vérifiée par const assert.
        let raw: [u8; BLOB_HEADER_SIZE] = unsafe { core::mem::transmute(hdr) };
        let mut raw60 = [0u8; 60];
        raw60.copy_from_slice(&raw[..60]);
        hdr.header_checksum = BlobHeaderDisk::compute_header_checksum(&raw60);

        Ok(hdr)
    }

    // ── Utilitaires ─────────────────────────────────────────────────────

    /// Convertit une taille en nombre de blocs (arrondi supérieur)
    #[inline]
    pub fn size_to_blocks(size: u64) -> u64 {
        let bs = BLOCK_SIZE as u64;
        size.checked_add(bs - 1)
            .unwrap_or(u64::MAX)
            / bs
    }

    /// Taille totale d'un blob sur disque donnée une taille originale
    pub fn disk_size_for(original_size: usize) -> u64 {
        let total = BLOB_HEADER_SIZE
            .saturating_add(original_size);
        align_up(DiskOffset(total as u64), BLOCK_SIZE as u64).map(|d| d.0).unwrap_or(u64::MAX)
    }
}

// ─────────────────────────────────────────────────────────────
// BatchBlobWriter — écriture multiple optimisée
// ─────────────────────────────────────────────────────────────

/// Résultat pour un blob dans un batch
#[derive(Debug)]
pub struct BatchBlobResult {
    pub index: usize,
    pub result: ExofsResult<BlobWriteResult>,
}

/// Writer de blobs en batch (pour réduire les allers-retours disque)
pub struct BatchBlobWriter {
    config: BlobWriterConfig,
    pending: Vec<(usize, Vec<u8>)>,
    results: Vec<BatchBlobResult>,
}

impl BatchBlobWriter {
    pub fn new(config: BlobWriterConfig) -> Self {
        Self {
            config,
            pending: Vec::new(),
            results: Vec::new(),
        }
    }

    /// Ajoute un blob au batch
    pub fn add(&mut self, index: usize, data: &[u8]) -> ExofsResult<()> {
        if data.len() > BLOB_MAX_SIZE {
            return Err(ExofsError::InvalidSize);
        }
        let mut v = Vec::new();
        v.try_reserve(data.len()).map_err(|_| ExofsError::NoMemory)?;
        v.extend_from_slice(data);
        self.pending.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
        self.pending.push((index, v));
        Ok(())
    }

    /// Soumet tous les blobs en attente
    pub fn flush<AllocFn, WriteFn, DedupFn>(
        &mut self,
        mut alloc_fn: AllocFn,
        mut write_fn: WriteFn,
        mut dedup_check: DedupFn,
    ) -> ExofsResult<&[BatchBlobResult]>
    where
        AllocFn: FnMut(u64) -> ExofsResult<DiskOffset>,
        WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
        DedupFn: FnMut(&BlobId) -> Option<DiskOffset>,
    {
        self.results.clear();

        for (idx, data) in self.pending.drain(..) {
            let blob_id = compute_blob_id(&data);
            let dedup_ref = dedup_check(&blob_id);
            let res = BlobWriter::write_blob(
                &data,
                &self.config,
                |n| alloc_fn(n),
                |off, buf| write_fn(off, buf),
                |_id| dedup_ref,
            );
            self.results.try_reserve(1).map_err(|_| ExofsError::NoMemory)?;
            self.results.push(BatchBlobResult { index: idx, result: res });
        }

        Ok(&self.results)
    }

    /// Nombre de blobs en attente
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Octets totaux en attente
    pub fn pending_bytes(&self) -> u64 {
        self.pending.iter().fold(0u64, |acc, (_, d)| acc.saturating_add(d.len() as u64))
    }

    /// Vide le batch sans écrire
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

// ─────────────────────────────────────────────────────────────
// Utilitaires d'écriture bas niveau
// ─────────────────────────────────────────────────────────────

/// Écrit un tampon padé à la taille d'un multiple de BLOCK_SIZE (WRITE-02)
pub fn write_padded<WriteFn>(
    mut write_fn: WriteFn,
    offset: DiskOffset,
    data: &[u8],
) -> ExofsResult<usize>
where
    WriteFn: FnMut(DiskOffset, &[u8]) -> ExofsResult<usize>,
{
    let aligned = align_up(DiskOffset(data.len() as u64), BLOCK_SIZE as u64)?.0 as usize;
    if aligned == data.len() {
        // Déjà aligné
        let n = write_fn(offset, data)?;
        if n != data.len() { return Err(ExofsError::ShortWrite); }
        return Ok(n);
    }

    // Pad avec des zéros
    let mut padded = Vec::new();
    padded.try_reserve(aligned).map_err(|_| ExofsError::NoMemory)?;
    padded.extend_from_slice(data);
    padded.resize(aligned, 0u8);

    let n = write_fn(offset, &padded)?;
    if n != padded.len() { return Err(ExofsError::ShortWrite); }
    Ok(data.len()) // retourne la taille des données utiles
}

/// Vérifie l'en-tête d'un blob lu (HDR-03)
pub fn verify_blob_header(raw: &[u8]) -> ExofsResult<&BlobHeaderDisk> {
    if raw.len() < BLOB_HEADER_SIZE {
        return Err(ExofsError::InvalidSize);
    }
    // SAFETY: taille vérifiée ci-dessus, repr(C) garanti
    let hdr: &BlobHeaderDisk = unsafe {
        &*(raw.as_ptr() as *const BlobHeaderDisk)
    };
    if hdr.magic != BLOB_HEADER_MAGIC {
        return Err(ExofsError::BadMagic);
    }
    if !hdr.verify_header_checksum() {
        return Err(ExofsError::ChecksumMismatch);
    }
    Ok(hdr)
}

/// Taille totale sur disque pour un blob connaissant stored_size
pub fn blob_total_disk_size(stored_size: u32) -> u64 {
    let raw = (BLOB_HEADER_SIZE as u64)
        .saturating_add(stored_size as u64);
    align_up(DiskOffset(raw), BLOCK_SIZE as u64).map(|d| d.0).unwrap_or(u64::MAX)
}

// ─────────────────────────────────────────────────────────────
// Tests unitaires
// ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data(n: usize, fill: u8) -> Vec<u8> {
        let mut v = alloc::vec![fill; n];
        v
    }

    #[test]
    fn header_size_constant() {
        assert_eq!(core::mem::size_of::<BlobHeaderDisk>(), BLOB_HEADER_SIZE);
    }

    #[test]
    fn compute_blob_id_is_raw() {
        let data = make_data(1024, 0xAB);
        let id1 = compute_blob_id(&data);
        let id2 = compute_blob_id(&data);
        assert_eq!(id1.0, id2.0);
    }

    #[test]
    fn write_blob_basic() {
        let data = make_data(512, 0x42);
        let config = BlobWriterConfig::new(EpochId(1)).no_dedup();

        let mut disk = alloc::vec![0u8; 65536];
        let alloc_fn = |_n: u64| Ok(DiskOffset(0));
        let write_fn = |off: DiskOffset, buf: &[u8]| -> ExofsResult<usize> {
            let start = off.0 as usize;
            let end = start + buf.len();
            if end <= disk.len() {
                disk[start..end].copy_from_slice(buf);
            }
            Ok(buf.len())
        };
        let dedup_fn = |_id: &BlobId| -> Option<DiskOffset> { None };

        let result = BlobWriter::write_blob(&data, &config, alloc_fn, write_fn, dedup_fn);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.original_size, 512);
        assert!(!r.dedup_hit);
    }

    #[test]
    fn verify_header_after_write() {
        let data = make_data(256, 0x77);
        let config = BlobWriterConfig::new(EpochId(5)).no_dedup().with_algo(CompressionType::None);

        let mut disk = alloc::vec![0u8; 16384];
        let alloc_fn = |_n: u64| Ok(DiskOffset(0));
        let write_fn = |off: DiskOffset, buf: &[u8]| -> ExofsResult<usize> {
            let s = off.0 as usize;
            if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
            Ok(buf.len())
        };

        let _ = BlobWriter::write_blob(&data, &config, alloc_fn, write_fn, |_| None);
        let hdr = verify_blob_header(&disk[..BLOB_HEADER_SIZE]);
        assert!(hdr.is_ok());
        let h = hdr.unwrap();
        assert_eq!(h.magic, BLOB_HEADER_MAGIC);
        assert_eq!(h.original_size, 256);
    }

    #[test]
    fn dedup_hit_returns_existing_offset() {
        let data = make_data(128, 0x11);
        let _id = compute_blob_id(&data);
        let config = BlobWriterConfig::new(EpochId(1));
        let existing = DiskOffset(4096);

        let alloc_fn = |_: u64| -> ExofsResult<DiskOffset> {
            panic!("alloc ne doit pas être appelé sur dédup hit")
        };
        let write_fn = |_: DiskOffset, _: &[u8]| -> ExofsResult<usize> {
            panic!("write ne doit pas être appelé sur dédup hit")
        };

        let r = BlobWriter::write_blob(
            &data, &config, alloc_fn, write_fn,
            |_bid| Some(existing),
        ).unwrap();

        assert!(r.dedup_hit);
        assert_eq!(r.offset.0, existing.0);
    }

    #[test]
    fn short_write_detected() {
        let data = make_data(512, 0x55);
        let config = BlobWriterConfig::new(EpochId(1)).no_dedup();
        let write_fn = |_: DiskOffset, _: &[u8]| -> ExofsResult<usize> { Ok(0) };
        let r = BlobWriter::write_blob(&data, &config, |_| Ok(DiskOffset(0)), write_fn, |_| None);
        assert!(matches!(r, Err(ExofsError::ShortWrite)));
    }

    #[test]
    fn batch_writer_multiple_blobs() {
        let config = BlobWriterConfig::new(EpochId(2)).no_dedup();
        let mut bw = BatchBlobWriter::new(config);
        for i in 0..4u8 {
            bw.add(i as usize, &make_data(64, i)).unwrap();
        }
        assert_eq!(bw.pending_count(), 4);

        let mut off = 0u64;
        let mut disk = alloc::vec![0u8; 65536];
        let results = bw.flush(
            |n| { let o = DiskOffset(off); off += n * BLOCK_SIZE as u64; Ok(o) },
            |o, buf| {
                let s = o.0 as usize;
                if s + buf.len() <= disk.len() { disk[s..s+buf.len()].copy_from_slice(buf); }
                Ok(buf.len())
            },
            |_| None,
        ).unwrap();

        assert_eq!(results.len(), 4);
        for r in results { assert!(r.result.is_ok()); }
    }

    #[test]
    fn disk_size_aligned_to_block() {
        let sz = BlobWriter::disk_size_for(100);
        assert_eq!(sz % BLOCK_SIZE as u64, 0);
    }

    #[test]
    fn size_to_blocks_rounds_up() {
        assert_eq!(BlobWriter::size_to_blocks(1), 1);
        assert_eq!(BlobWriter::size_to_blocks(4096), 1);
        assert_eq!(BlobWriter::size_to_blocks(4097), 2);
    }
}
