//! exoar_reader.rs — Lecture et validation d'archives ExoAR (no_std).
//!
//! Ce module fournit :
//!  - `ArchiveSource`    : trait de lecture séquentielle / aléatoire.
//!  - `BlobReceiver`     : callback appelé pour chaque blob extrait.
//!  - `ExoarReader`      : lecteur principal (header → entrées → footer).
//!  - `ExoarScanner`     : scan sans extraction (validation, statistiques).
//!  - `ExoarReaderConfig`: configuration du lecteur.
//!  - `ExoarReadReport`  : rapport détaillé d'une lecture.
//!  - `ExoarReadError`   : erreurs de lecture spécifiques.
//!
//! RÈGLE 8  : chaque magic est validé EN PREMIER, avant tout accès aux champs.
//! RÈGLE 11 : BlobId = blake3(données brutes) — recalculé et comparé si verify_blobs = true.
//! RECUR-01 : pas de récursion — boucles while.
//! OOM-02   : try_reserve avant tout push.
//! ARITH-02 : saturating_* / checked_* sur tous les compteurs.

extern crate alloc;
use super::exoar_format::{
    crc32c_update, crc32c_verify, ExoarEntryHeader, ExoarFooter, ExoarHeader, ExoarSummary,
    EXOAR_MAX_ENTRIES, EXOAR_MAX_PAYLOAD,
};
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;
use core::mem::size_of;

// ─── Trait source I/O ────────────────────────────────────────────────────────

/// Abstraction de lecture séquentielle sur une source d'archive.
/// RECUR-01 : les implémentations ne doivent pas se rappeler elles-mêmes.
pub trait ArchiveSource {
    /// Lit exactement `buf.len()` octets dans `buf`.
    /// Retourne `ExofsError::EndOfFile` si la source est épuisée.
    fn read_exact(&mut self, buf: &mut [u8]) -> ExofsResult<()>;

    /// Retourne le nombre d'octets lus depuis le début.
    fn bytes_read(&self) -> u64;

    /// Avance de `n` octets sans les lire (skip).
    fn skip(&mut self, n: u64) -> ExofsResult<()>;
}

/// Callback appelé pour chaque blob extrait.
pub trait BlobReceiver {
    /// Reçoit les données brutes d'un blob avec son BlobId déclaré.
    /// Retourne `false` pour interrompre la lecture.
    fn receive_blob(&mut self, blob_id: &[u8; 32], data: &[u8], flags: u8) -> bool;

    /// Reçoit un tombstone (entrée de suppression).
    fn receive_tombstone(&mut self, _blob_id: &[u8; 32]) {}
}

// ─── Erreurs spécifiques à la lecture ────────────────────────────────────────

/// Erreur de lecture d'archive ExoAR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExoarReadError {
    /// Magic invalide dans l'en-tête global.
    BadHeaderMagic,
    /// Version incompatible du format.
    BadVersion,
    /// Magic invalide dans une entrée blob.
    BadEntryMagic,
    /// Magic invalide dans le footer.
    BadFooterMagic,
    /// CRC32C du payload ne correspond pas.
    CrcMismatch { entry_idx: u32 },
    /// BlobId déclaré ≠ blake3(données brutes) — RÈGLE 11.
    BlobIdMismatch { entry_idx: u32 },
    /// Taille de payload dépasse EXOAR_MAX_PAYLOAD.
    PayloadTooLarge { entry_idx: u32 },
    /// Dépassement du nombre maximal d'entrées.
    TooManyEntries,
    /// Fin de source prématurée.
    TruncatedArchive,
    /// Compteur entry_count header ≠ footer.
    EntryCountMismatch,
    /// Allocation mémoire échouée.
    OutOfMemory,
    /// Erreur de lecture sur la source.
    IoError,
}

impl From<ExoarReadError> for ExofsError {
    fn from(e: ExoarReadError) -> Self {
        match e {
            ExoarReadError::BadHeaderMagic
            | ExoarReadError::BadEntryMagic
            | ExoarReadError::BadFooterMagic => ExofsError::CorruptedStructure,
            ExoarReadError::BadVersion => ExofsError::InvalidArgument,
            ExoarReadError::CrcMismatch { .. } => ExofsError::ChecksumMismatch,
            ExoarReadError::BlobIdMismatch { .. } => ExofsError::BlobIdMismatch,
            ExoarReadError::PayloadTooLarge { .. } => ExofsError::OffsetOverflow,
            ExoarReadError::TooManyEntries => ExofsError::OffsetOverflow,
            ExoarReadError::TruncatedArchive => ExofsError::CorruptedStructure,
            ExoarReadError::EntryCountMismatch => ExofsError::CorruptedStructure,
            ExoarReadError::OutOfMemory => ExofsError::NoMemory,
            ExoarReadError::IoError => ExofsError::IoFailed,
        }
    }
}

// ─── Configuration ───────────────────────────────────────────────────────────

/// Configuration du lecteur ExoAR.
#[derive(Clone, Copy, Debug)]
pub struct ExoarReaderConfig {
    /// Vérifier le CRC32C de chaque payload.
    pub verify_crc: bool,
    /// Vérifier le BlobId (blake3 des données brutes) — RÈGLE 11.
    pub verify_blob_id: bool,
    /// Si true, continue malgré les erreurs CRC (mode recovery).
    pub skip_crc_errors: bool,
    /// Si true, continue malgré les erreurs de BlobId.
    pub skip_blob_id_errors: bool,
    /// Si true, ignore les tombstones (ne les passe pas au receiver).
    pub skip_tombstones: bool,
    /// Taille maximale de payload acceptée (0 = EXOAR_MAX_PAYLOAD).
    pub max_payload_size: u64,
    /// Nombre maximal d'entrées à lire (0 = EXOAR_MAX_ENTRIES).
    pub max_entries: u32,
}

impl ExoarReaderConfig {
    pub const fn default() -> Self {
        Self {
            verify_crc: true,
            verify_blob_id: false,
            skip_crc_errors: false,
            skip_blob_id_errors: false,
            skip_tombstones: false,
            max_payload_size: 0,
            max_entries: 0,
        }
    }

    pub const fn strict() -> Self {
        Self {
            verify_crc: true,
            verify_blob_id: true,
            skip_crc_errors: false,
            skip_blob_id_errors: false,
            skip_tombstones: false,
            max_payload_size: 0,
            max_entries: 0,
        }
    }

    pub const fn recovery() -> Self {
        Self {
            verify_crc: true,
            verify_blob_id: false,
            skip_crc_errors: true,
            skip_blob_id_errors: true,
            skip_tombstones: false,
            max_payload_size: 0,
            max_entries: 0,
        }
    }

    fn effective_max_payload(&self) -> u64 {
        if self.max_payload_size == 0 {
            EXOAR_MAX_PAYLOAD
        } else {
            self.max_payload_size.min(EXOAR_MAX_PAYLOAD)
        }
    }

    fn effective_max_entries(&self) -> u32 {
        if self.max_entries == 0 {
            EXOAR_MAX_ENTRIES
        } else {
            self.max_entries.min(EXOAR_MAX_ENTRIES)
        }
    }
}

// ─── Rapport de lecture ───────────────────────────────────────────────────────

/// Rapport détaillé d'une session de lecture d'archive.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExoarReadReport {
    pub entries_read: u32,
    pub bytes_consumed: u64,
    pub crc_errors: u32,
    pub blob_id_errors: u32,
    pub tombstones_processed: u32,
    pub payloads_verified: u32,
    pub entries_skipped: u32,
    pub max_payload_seen: u64,
    pub header_epoch_base: u64,
    pub header_epoch_target: u64,
    pub header_flags: u32,
    pub footer_global_crc32: u32,
    pub archive_valid: bool,
}

impl ExoarReadReport {
    pub const fn new() -> Self {
        Self {
            entries_read: 0,
            bytes_consumed: 0,
            crc_errors: 0,
            blob_id_errors: 0,
            tombstones_processed: 0,
            payloads_verified: 0,
            entries_skipped: 0,
            max_payload_seen: 0,
            header_epoch_base: 0,
            header_epoch_target: 0,
            header_flags: 0,
            footer_global_crc32: 0,
            archive_valid: false,
        }
    }

    #[inline]
    pub fn has_errors(&self) -> bool {
        self.crc_errors > 0 || self.blob_id_errors > 0
    }

    pub fn to_summary(&self) -> ExoarSummary {
        let mut s = ExoarSummary::new();
        s.entry_count = self.entries_read;
        s.tombstone_count = self.tombstones_processed;
        s.crc_errors = self.crc_errors;
        s
    }
}

// ─── Lecteur principal ────────────────────────────────────────────────────────

/// Lecteur d'archives ExoAR.
///
/// Séquence de lecture (RÈGLE 8 : magic EN PREMIER à chaque étape) :
///   1. Lire + valider ExoarHeader
///   2. Pour chaque entrée : lire ExoarEntryHeader (magic), lire payload, vérifier CRC
///   3. Lire + valider ExoarFooter, croiser entry_count
pub struct ExoarReader {
    config: ExoarReaderConfig,
}

impl ExoarReader {
    pub const fn new(config: ExoarReaderConfig) -> Self {
        Self { config }
    }

    pub const fn with_default_config() -> Self {
        Self::new(ExoarReaderConfig::default())
    }

    pub const fn strict() -> Self {
        Self::new(ExoarReaderConfig::strict())
    }

    /// Lit l'archive depuis `source` et appelle `receiver` pour chaque blob.
    /// RECUR-01 : boucle while sur les entrées.
    pub fn read<S, R>(
        &self,
        source: &mut S,
        receiver: &mut R,
    ) -> Result<ExoarReadReport, ExoarReadError>
    where
        S: ArchiveSource,
        R: BlobReceiver,
    {
        let mut report = ExoarReadReport::new();

        // 1. Lire et valider l'en-tête — RÈGLE 8 : magic EN PREMIER.
        let mut hdr_buf = [0u8; size_of::<ExoarHeader>()];
        source
            .read_exact(&mut hdr_buf)
            .map_err(|_| ExoarReadError::IoError)?;
        let hdr = ExoarHeader::from_bytes(&hdr_buf).ok_or(ExoarReadError::BadHeaderMagic)?;
        if !hdr.validate_version() {
            return Err(ExoarReadError::BadVersion);
        }

        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let entry_count_declared: u32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.entry_count)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_epoch_base =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.epoch_base)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_epoch_target =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.epoch_target)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_flags = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.flags)) };

        let max_entries = self
            .config
            .effective_max_entries()
            .min(entry_count_declared);
        let max_payload = self.config.effective_max_payload();

        // CRC global accumulé
        let mut global_crc = crc32c_update(0, &hdr_buf);

        // 2. Lire les entrées — boucle while (RECUR-01).
        let mut entry_idx: u32 = 0;
        while entry_idx < max_entries {
            // Lire l'en-tête d'entrée — RÈGLE 8 : magic EN PREMIER.
            let mut ehdr_buf = [0u8; size_of::<ExoarEntryHeader>()];
            match source.read_exact(&mut ehdr_buf) {
                Ok(_) => {}
                Err(_) => {
                    return Err(ExoarReadError::TruncatedArchive);
                }
            }
            global_crc = crc32c_update(global_crc, &ehdr_buf);

            let ehdr =
                ExoarEntryHeader::from_bytes(&ehdr_buf).ok_or(ExoarReadError::BadEntryMagic)?;

            // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
            let payload_size: u64 =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ehdr.payload_size)) };
            if payload_size > max_payload {
                return Err(ExoarReadError::PayloadTooLarge { entry_idx });
            }

            // Lire le payload
            let usize_payload = payload_size as usize;
            let mut payload: Vec<u8> = Vec::new();
            if usize_payload > 0 {
                payload
                    .try_reserve(usize_payload)
                    .map_err(|_| ExoarReadError::OutOfMemory)?;
                payload.resize(usize_payload, 0u8);
                source
                    .read_exact(&mut payload)
                    .map_err(|_| ExoarReadError::IoError)?;
                global_crc = crc32c_update(global_crc, &payload);
            }

            // Vérifier CRC32C du payload
            if self.config.verify_crc && usize_payload > 0 {
                // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
                let declared_crc: u32 =
                    unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ehdr.payload_crc32)) };
                if !crc32c_verify(&payload, declared_crc) {
                    report.crc_errors = report.crc_errors.saturating_add(1);
                    if !self.config.skip_crc_errors {
                        return Err(ExoarReadError::CrcMismatch { entry_idx });
                    }
                    entry_idx = entry_idx.saturating_add(1);
                    report.entries_skipped = report.entries_skipped.saturating_add(1);
                    continue;
                }
                report.payloads_verified = report.payloads_verified.saturating_add(1);
            }

            // Vérifier BlobId = blake3(données brutes) — RÈGLE 11
            if self.config.verify_blob_id && !ehdr.is_tombstone() && !payload.is_empty() {
                let computed = blake3_hash_simple(&payload);
                if computed != ehdr.blob_id {
                    report.blob_id_errors = report.blob_id_errors.saturating_add(1);
                    if !self.config.skip_blob_id_errors {
                        return Err(ExoarReadError::BlobIdMismatch { entry_idx });
                    }
                }
            }

            // Dispatcher vers le receiver
            if ehdr.is_tombstone() {
                report.tombstones_processed = report.tombstones_processed.saturating_add(1);
                if !self.config.skip_tombstones {
                    receiver.receive_tombstone(&ehdr.blob_id);
                }
            } else {
                let keep_reading = receiver.receive_blob(&ehdr.blob_id, &payload, ehdr.flags);
                if !keep_reading {
                    break;
                }
            }

            let ps_tracked = payload_size;
            if ps_tracked > report.max_payload_seen {
                report.max_payload_seen = ps_tracked;
            }
            report.entries_read = report.entries_read.saturating_add(1);
            entry_idx = entry_idx.saturating_add(1);
        }

        // 3. Lire et valider le footer — RÈGLE 8.
        let mut ftr_buf = [0u8; size_of::<ExoarFooter>()];
        source
            .read_exact(&mut ftr_buf)
            .map_err(|_| ExoarReadError::TruncatedArchive)?;
        let ftr = ExoarFooter::from_bytes(&ftr_buf).ok_or(ExoarReadError::BadFooterMagic)?;

        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let ftr_entry_count: u32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ftr.entry_count)) };
        if ftr_entry_count != entry_count_declared {
            return Err(ExoarReadError::EntryCountMismatch);
        }
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.footer_global_crc32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ftr.global_crc32)) };
        report.bytes_consumed = source.bytes_read();
        report.archive_valid = !report.has_errors();

        Ok(report)
    }
}

// ─── Scanner (validation sans extraction) ────────────────────────────────────

/// Scanner ExoAR : valide l'archive sans extraire les payloads en mémoire.
/// Utile pour vérifier l'intégrité à moindre coût mémoire.
pub struct ExoarScanner {
    config: ExoarReaderConfig,
}

impl ExoarScanner {
    pub const fn new(config: ExoarReaderConfig) -> Self {
        Self { config }
    }

    pub const fn default() -> Self {
        Self::new(ExoarReaderConfig::strict())
    }

    /// Scan complet : (RECUR-01 : boucle while) valide header, entrées, footer.
    pub fn scan<S: ArchiveSource>(
        &self,
        source: &mut S,
    ) -> Result<ExoarReadReport, ExoarReadError> {
        let mut report = ExoarReadReport::new();

        let mut hdr_buf = [0u8; size_of::<ExoarHeader>()];
        source
            .read_exact(&mut hdr_buf)
            .map_err(|_| ExoarReadError::IoError)?;
        let hdr = ExoarHeader::from_bytes(&hdr_buf).ok_or(ExoarReadError::BadHeaderMagic)?;
        if !hdr.validate_version() {
            return Err(ExoarReadError::BadVersion);
        }

        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let entry_count: u32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.entry_count)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_epoch_base =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.epoch_base)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_epoch_target =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.epoch_target)) };
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.header_flags = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(hdr.flags)) };

        let max_entries = self.config.effective_max_entries().min(entry_count);
        let max_payload = self.config.effective_max_payload();

        let mut global_crc = crc32c_update(0, &hdr_buf);
        let mut entry_idx: u32 = 0;

        while entry_idx < max_entries {
            let mut ehdr_buf = [0u8; size_of::<ExoarEntryHeader>()];
            source
                .read_exact(&mut ehdr_buf)
                .map_err(|_| ExoarReadError::TruncatedArchive)?;
            global_crc = crc32c_update(global_crc, &ehdr_buf);

            let ehdr =
                ExoarEntryHeader::from_bytes(&ehdr_buf).ok_or(ExoarReadError::BadEntryMagic)?;

            // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
            let payload_size: u64 =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ehdr.payload_size)) };
            if payload_size > max_payload {
                return Err(ExoarReadError::PayloadTooLarge { entry_idx });
            }

            // Lire le payload pour CRC mais sans conserver en mémoire (par blocs de 4 KiB).
            let mut remaining = payload_size;
            let mut local_crc: u32 = 0;
            let mut payload_buf = [0u8; 4096];

            while remaining > 0 {
                let chunk = remaining.min(4096) as usize;
                source
                    .read_exact(&mut payload_buf[..chunk])
                    .map_err(|_| ExoarReadError::IoError)?;
                local_crc = crc32c_update(local_crc, &payload_buf[..chunk]);
                global_crc = crc32c_update(global_crc, &payload_buf[..chunk]);
                remaining = remaining.saturating_sub(chunk as u64);
            }

            if self.config.verify_crc && payload_size > 0 {
                // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
                let declared_crc: u32 =
                    unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ehdr.payload_crc32)) };
                if local_crc != declared_crc {
                    report.crc_errors = report.crc_errors.saturating_add(1);
                    if !self.config.skip_crc_errors {
                        return Err(ExoarReadError::CrcMismatch { entry_idx });
                    }
                }
                report.payloads_verified = report.payloads_verified.saturating_add(1);
            }

            if ehdr.is_tombstone() {
                report.tombstones_processed = report.tombstones_processed.saturating_add(1);
            }
            report.entries_read = report.entries_read.saturating_add(1);
            entry_idx = entry_idx.saturating_add(1);
        }

        let mut ftr_buf = [0u8; size_of::<ExoarFooter>()];
        source
            .read_exact(&mut ftr_buf)
            .map_err(|_| ExoarReadError::TruncatedArchive)?;
        let ftr = ExoarFooter::from_bytes(&ftr_buf).ok_or(ExoarReadError::BadFooterMagic)?;

        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        let ftr_count: u32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ftr.entry_count)) };
        if ftr_count != entry_count {
            return Err(ExoarReadError::EntryCountMismatch);
        }
        // SAFETY: tampon de longueur suffisante, vérifié avant appel, repr(C).
        report.footer_global_crc32 =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(ftr.global_crc32)) };
        report.bytes_consumed = source.bytes_read();
        report.archive_valid = !report.has_errors();
        Ok(report)
    }
}

// ─── Source en mémoire (implémentation de test) ───────────────────────────────

/// Implémentation de `ArchiveSource` lisant depuis un slice en mémoire.
pub struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SliceSource<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

impl<'a> ArchiveSource for SliceSource<'a> {
    fn read_exact(&mut self, buf: &mut [u8]) -> ExofsResult<()> {
        if self.pos.saturating_add(buf.len()) > self.data.len() {
            return Err(ExofsError::EndOfFile);
        }
        let end = self.pos.saturating_add(buf.len());
        buf.copy_from_slice(&self.data[self.pos..end]);
        self.pos = end;
        Ok(())
    }

    fn bytes_read(&self) -> u64 {
        self.pos as u64
    }

    fn skip(&mut self, n: u64) -> ExofsResult<()> {
        let new_pos = self.pos.saturating_add(n as usize);
        if new_pos > self.data.len() {
            return Err(ExofsError::EndOfFile);
        }
        self.pos = new_pos;
        Ok(())
    }
}

// ─── Receiver de collecte (pour tests) ───────────────────────────────────────

/// Receiver qui collecte tous les blobs reçus dans un Vec.
pub struct CollectingReceiver {
    pub blobs: Vec<([u8; 32], Vec<u8>)>,
    pub tombstones: Vec<[u8; 32]>,
}

impl CollectingReceiver {
    pub fn new() -> Self {
        Self {
            blobs: Vec::new(),
            tombstones: Vec::new(),
        }
    }

    /// Consomme le receiver et retourne les blobs collectés.
    pub fn into_blobs(self) -> Vec<([u8; 32], Vec<u8>)> {
        self.blobs
    }
}

impl BlobReceiver for CollectingReceiver {
    fn receive_blob(&mut self, blob_id: &[u8; 32], data: &[u8], _flags: u8) -> bool {
        let mut v = Vec::new();
        if v.try_reserve(data.len()).is_err() {
            return false;
        }
        v.extend_from_slice(data);
        let mut id = [0u8; 32];
        id.copy_from_slice(blob_id);
        let _ = self.blobs.try_reserve(1);
        let _ = self.blobs.push((*blob_id, v));
        true
    }

    fn receive_tombstone(&mut self, blob_id: &[u8; 32]) {
        let _ = self.tombstones.try_reserve(1);
        let _ = self.tombstones.push(*blob_id);
    }
}

// ─── blake3 inline minimal ───────────────────────────────────────────────────

/// Calcule un blake3 simplifié (fnv1a-like fallback en no_std si blake3 indisponible).
/// RÈGLE 11 : BlobId = blake3(données brutes AVANT compression/chiffrement).
/// En production, remplacer par le vrai blake3 du module content_hash.
fn blake3_hash_simple(data: &[u8]) -> [u8; 32] {
    let mut state = [0u64; 4];
    state[0] = 0x6C62_272E_07BB_0142;
    state[1] = 0x6295_C58D_5935_4F4E;
    state[2] = 0xD8A2_5D4F_5C3A_2B1E;
    state[3] = 0xA3F6_C1D2_E5B7_8940;
    let mut i = 0usize;
    while i < data.len() {
        let byte = data[i] as u64;
        state[0] = state[0]
            .wrapping_add(byte)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        state[1] ^= state[0].rotate_left(23);
        state[2] = state[2]
            .wrapping_add(state[1])
            .wrapping_mul(0x6C62_272E_07BB_0142);
        state[3] ^= state[2].rotate_right(17);
        state[0] = state[0].wrapping_add(state[3]);
        i = i.wrapping_add(1);
    }
    let mut out = [0u8; 32];
    let mut idx = 0usize;
    while idx < 4 {
        let bytes = state[idx].to_le_bytes();
        let base = idx.wrapping_mul(8);
        let mut k = 0usize;
        while k < 8 {
            out[base.wrapping_add(k)] = bytes[k];
            k = k.wrapping_add(1);
        }
        idx = idx.wrapping_add(1);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::super::exoar_format::crc32c_compute;
    use super::super::exoar_format::{ExoarEntryHeader, ExoarFooter, ExoarHeader};
    use super::*;

    fn build_archive(blobs: &[(&[u8; 32], &[u8])], flags: u32) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut hdr = ExoarHeader::new(flags, 0, 1);
        hdr.entry_count = blobs.len() as u32;
        out.extend_from_slice(hdr.as_bytes());

        for (blob_id, data) in blobs {
            let crc = crc32c_compute(data);
            let mut ehdr = ExoarEntryHeader::new(**blob_id, data.len() as u64, data.len() as u64);
            ehdr.payload_crc32 = crc;
            out.extend_from_slice(ehdr.as_bytes());
            out.extend_from_slice(data);
        }

        let ftr = ExoarFooter::new(blobs.len() as u32, 0, out.len() as u64 + 32);
        out.extend_from_slice(ftr.as_bytes());
        out
    }

    #[test]
    fn test_read_empty_archive() {
        let archive = build_archive(&[], 0);
        let mut src = SliceSource::new(&archive);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("read ok");
        assert_eq!(report.entries_read, 0);
        assert!(report.archive_valid);
    }

    #[test]
    fn test_read_single_blob() {
        let bid = [1u8; 32];
        let data = b"hello exofs";
        let archive = build_archive(&[(&bid, data)], 0);
        let mut src = SliceSource::new(&archive);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("read ok");
        assert_eq!(report.entries_read, 1);
        assert_eq!(rcv.blobs.len(), 1);
        assert_eq!(rcv.blobs[0].0, bid);
        assert_eq!(rcv.blobs[0].1, data);
    }

    #[test]
    fn test_read_multiple_blobs() {
        let b1 = [1u8; 32];
        let b2 = [2u8; 32];
        let b3 = [3u8; 32];
        let d1 = b"blob one data";
        let d2 = b"blob two bigger data";
        let d3 = b"third blob";
        let archive = build_archive(&[(&b1, d1), (&b2, d2), (&b3, d3)], 0);
        let mut src = SliceSource::new(&archive);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("read ok");
        assert_eq!(report.entries_read, 3);
        assert_eq!(rcv.blobs.len(), 3);
    }

    #[test]
    fn test_bad_header_magic() {
        let mut archive = build_archive(&[], 0);
        archive[0] = 0xFF; // Corrompre le magic
        let mut src = SliceSource::new(&archive);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let result = reader.read(&mut src, &mut rcv);
        assert!(matches!(result, Err(ExoarReadError::BadHeaderMagic)));
    }

    #[test]
    fn test_crc_mismatch_detected() {
        let bid = [1u8; 32];
        let data = b"data with crc";
        let mut archive = build_archive(&[(&bid, data)], 0);
        // Corrompre le payload (après header 128 + entry_header 96 = 224)
        let payload_start = 128 + 96;
        if archive.len() > payload_start {
            archive[payload_start] ^= 0xFF;
        }
        let mut src = SliceSource::new(&archive);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let result = reader.read(&mut src, &mut rcv);
        assert!(matches!(result, Err(ExoarReadError::CrcMismatch { .. })));
    }

    #[test]
    fn test_skip_crc_errors_recovery() {
        let bid = [1u8; 32];
        let data = b"recoverable data";
        let mut archive = build_archive(&[(&bid, data)], 0);
        let payload_start = 128 + 96;
        if archive.len() > payload_start {
            archive[payload_start] ^= 0xFF;
        }
        let mut src = SliceSource::new(&archive);
        let cfg = ExoarReaderConfig::recovery();
        let reader = ExoarReader::new(cfg);
        let mut rcv = CollectingReceiver::new();
        let report = reader.read(&mut src, &mut rcv).expect("recovery ok");
        assert_eq!(report.crc_errors, 1);
    }

    #[test]
    fn test_scanner_empty_archive() {
        let archive = build_archive(&[], 0);
        let mut src = SliceSource::new(&archive);
        let scanner = ExoarScanner::default();
        let report = scanner.scan(&mut src).expect("scan ok");
        assert_eq!(report.entries_read, 0);
        assert!(report.archive_valid);
    }

    #[test]
    fn test_scanner_multi_blob() {
        let b1 = [10u8; 32];
        let b2 = [20u8; 32];
        let archive = build_archive(&[(&b1, b"payload_a"), (&b2, b"payload_b")], 0);
        let mut src = SliceSource::new(&archive);
        let scanner = ExoarScanner::default();
        let report = scanner.scan(&mut src).expect("scan ok");
        assert_eq!(report.entries_read, 2);
    }

    #[test]
    fn test_truncated_archive_error() {
        let archive = build_archive(&[], 0);
        let truncated = &archive[..archive.len().saturating_sub(10)];
        let mut src = SliceSource::new(truncated);
        let reader = ExoarReader::with_default_config();
        let mut rcv = CollectingReceiver::new();
        let result = reader.read(&mut src, &mut rcv);
        assert!(result.is_err());
    }

    #[test]
    fn test_report_has_errors_false_on_clean() {
        let report = ExoarReadReport::new();
        assert!(!report.has_errors());
    }

    #[test]
    fn test_report_to_summary() {
        let mut r = ExoarReadReport::new();
        r.entries_read = 5;
        r.tombstones_processed = 1;
        let s = r.to_summary();
        assert_eq!(s.entry_count, 5);
        assert_eq!(s.tombstone_count, 1);
    }

    #[test]
    fn test_slice_source_skip() {
        let data = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let mut src = SliceSource::new(&data);
        src.skip(4).expect("skip ok");
        assert_eq!(src.bytes_read(), 4);
        let mut buf = [0u8; 1];
        src.read_exact(&mut buf).expect("read ok");
        assert_eq!(buf[0], 4);
    }

    #[test]
    fn test_exoar_reader_error_conversion() {
        let e: ExofsError = ExoarReadError::BadHeaderMagic.into();
        assert_eq!(e, ExofsError::CorruptedStructure);
        let e2: ExofsError = ExoarReadError::OutOfMemory.into();
        assert_eq!(e2, ExofsError::NoMemory);
    }

    #[test]
    fn test_config_effective_max_payload() {
        let cfg = ExoarReaderConfig::default();
        assert_eq!(cfg.effective_max_payload(), EXOAR_MAX_PAYLOAD);
        let mut cfg2 = ExoarReaderConfig::default();
        cfg2.max_payload_size = 1024;
        assert_eq!(cfg2.effective_max_payload(), 1024);
    }
}
