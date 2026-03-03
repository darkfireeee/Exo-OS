// kernel/src/fs/exofs/storage/compression_choice.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Sélection de l'algorithme de compression — ExoFS
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Stratégie de compression selon le spec ExoFS :
//   text    → Zstd
//   media   → None  (déjà compressé)
//   binary/data → Lz4
//   small (<512B) → None (overhead > gain)
//
// Ce module ne compresse PAS — il décide de l'algorithme.
// La compression elle-même est déléguée à compression_writer.rs.
//
// Règle HASH-02 : BlobId calculé AVANT compression → ce module est
// appelé après compute_blob_id(), jamais avant.

use crate::fs::exofs::core::{ExofsError, ExofsResult};

// ─────────────────────────────────────────────────────────────────────────────
// CompressionType — algorithme choisi
// ─────────────────────────────────────────────────────────────────────────────

/// Algorithme de compression.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompressionType {
    /// Pas de compression.
    None  = 0,
    /// LZ4 — rapide, bonne compression pour les données binaires.
    Lz4   = 1,
    /// Zstd — meilleure compression, adapté au texte (source code, JSON...).
    Zstd  = 2,
}

impl CompressionType {
    pub fn from_u8(v: u8) -> ExofsResult<Self> {
        match v {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Zstd),
            _ => Err(ExofsError::InvalidArgument),
        }
    }

    pub fn to_u8(self) -> u8 { self as u8 }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4  => "lz4",
            Self::Zstd => "zstd",
        }
    }

    pub fn is_compressed(self) -> bool { self != Self::None }
}

// ─────────────────────────────────────────────────────────────────────────────
// ContentHint — indicateur du type de contenu
// ─────────────────────────────────────────────────────────────────────────────

/// Indice fourni par l'appelant sur la nature du contenu.
/// Aide le sélecteur à choisir l'algorithme optimal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContentHint {
    /// Contenu inconnu.
    Unknown,
    /// Texte (UTF-8, JSON, code source, XML...).
    Text,
    /// Données structurées (sérialisation, tables...).
    StructuredData,
    /// Contenu binaire arbitraire (exécutables, archives...).
    Binary,
    /// Média (images, audio, vidéo) — déjà compressé.
    Media,
    /// Métadonnées système (petits objets).
    Metadata,
    /// Données déjà compressées (zip, xz...).
    AlreadyCompressed,
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionDecision — résultat de la sélection
// ─────────────────────────────────────────────────────────────────────────────

/// Décision de compression + justification.
#[derive(Clone, Debug)]
pub struct CompressionDecision {
    pub algorithm:  CompressionType,
    pub reason:     &'static str,
    pub hint_used:  ContentHint,
    pub data_size:  u64,
}

impl CompressionDecision {
    pub fn none(reason: &'static str, hint: ContentHint, size: u64) -> Self {
        Self { algorithm: CompressionType::None, reason, hint_used: hint, data_size: size }
    }
    pub fn lz4(reason: &'static str, hint: ContentHint, size: u64) -> Self {
        Self { algorithm: CompressionType::Lz4, reason, hint_used: hint, data_size: size }
    }
    pub fn zstd(reason: &'static str, hint: ContentHint, size: u64) -> Self {
        Self { algorithm: CompressionType::Zstd, reason, hint_used: hint, data_size: size }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Seuils de sélection
// ─────────────────────────────────────────────────────────────────────────────

/// En dessous de ce seuil, la compression est ignorée (overhead > gain).
pub const MIN_COMPRESSIBLE_BYTES: u64 = 512;

/// En dessous de ce seuil pour Zstd (lourd) : on préfère Lz4.
pub const ZSTD_MIN_BYTES: u64 = 2048;

/// Ratio d'entropie en permille au-dessus duquel on considère les données
/// incompressibles (heuristique rapide sur les 256 premiers octets).
pub const MAX_ENTROPY_PERMILLE: u32 = 980;

// ─────────────────────────────────────────────────────────────────────────────
// choose_compression — sélecteur principal
// ─────────────────────────────────────────────────────────────────────────────

/// Sélectionne l'algorithme optimal selon l'indice de contenu et la taille.
///
/// # Règle HASH-02
/// Doit être appelée APRÈS `compute_blob_id()` — jamais avant.
pub fn choose_compression(data: &[u8], hint: ContentHint) -> CompressionDecision {
    let size = data.len() as u64;

    // Trop petit → pas de compression.
    if size < MIN_COMPRESSIBLE_BYTES {
        return CompressionDecision::none("too_small", hint, size);
    }

    match hint {
        ContentHint::Media | ContentHint::AlreadyCompressed => {
            CompressionDecision::none("already_compressed", hint, size)
        }

        ContentHint::Text | ContentHint::Metadata => {
            if size < ZSTD_MIN_BYTES {
                CompressionDecision::lz4("text_small", hint, size)
            } else {
                CompressionDecision::zstd("text_large", hint, size)
            }
        }

        ContentHint::StructuredData => {
            CompressionDecision::zstd("structured_data", hint, size)
        }

        ContentHint::Binary | ContentHint::Unknown => {
            // Heuristique d'entropie rapide sur un échantillon.
            let entropy = sample_entropy(data);
            if entropy > MAX_ENTROPY_PERMILLE {
                CompressionDecision::none("high_entropy", hint, size)
            } else {
                CompressionDecision::lz4("binary_compressible", hint, size)
            }
        }
    }
}

/// Sélectionne sans inspecter le contenu (basé uniquement sur l'indice).
pub fn choose_compression_hint_only(hint: ContentHint, size: u64) -> CompressionDecision {
    if size < MIN_COMPRESSIBLE_BYTES {
        return CompressionDecision::none("too_small", hint, size);
    }
    match hint {
        ContentHint::Media | ContentHint::AlreadyCompressed =>
            CompressionDecision::none("already_compressed_hint", hint, size),
        ContentHint::Text =>
            CompressionDecision::zstd("text_hint", hint, size),
        ContentHint::StructuredData =>
            CompressionDecision::zstd("structured_hint", hint, size),
        ContentHint::Metadata | ContentHint::Binary | ContentHint::Unknown =>
            CompressionDecision::lz4("binary_hint", hint, size),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// sample_entropy — heuristique d'entropie rapide
// ─────────────────────────────────────────────────────────────────────────────

/// Calcule une approximation d'entropie sur les 256 premiers octets (ou moins).
/// Retourne une valeur en permille (0 = très compressible, 1000 = aléatoire).
///
/// Méthode : comptage des valeurs distinctes dans un histogramme 256 cases.
/// Ratio = (valeurs_distinctes * 1000) / 256.
pub fn sample_entropy(data: &[u8]) -> u32 {
    let sample_len = data.len().min(256);
    let sample     = &data[..sample_len];

    let mut seen = [false; 256];
    for &b in sample { seen[b as usize] = true; }

    let distinct = seen.iter().filter(|&&v| v).count() as u32;
    (distinct * 1000) / 256
}

/// Version améliorée : histogramme complet sur un échantillon plus large (512 B).
pub fn sample_entropy_full(data: &[u8]) -> u32 {
    const SAMPLE: usize = 512;
    let sample_len = data.len().min(SAMPLE);
    let sample     = &data[..sample_len];

    let mut hist = [0u32; 256];
    for &b in sample { hist[b as usize] = hist[b as usize].saturating_add(1); }

    // Approximation de l'entropie de Shannon (évite log2 en no_std).
    let non_zero = hist.iter().filter(|&&c| c > 0).count() as u32;
    let frac_non_zero = (non_zero * 1000) / 256;

    // Bonus si distribution uniforme.
    let max_count = hist.iter().copied().max().unwrap_or(1).max(1);
    let uniformity = (sample_len as u32 * 1000) / (max_count * 256).max(1);

    frac_non_zero
        .saturating_add(uniformity / 4)
        .min(1000)
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionPolicy — politique globale (peut être coercée)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompressionPolicy {
    /// Laisser le sélecteur décider.
    Auto,
    /// Toujours None.
    ForceNone,
    /// Toujours Lz4.
    ForceLz4,
    /// Toujours Zstd.
    ForceZstd,
}

impl CompressionPolicy {
    pub fn apply(&self, decision: CompressionDecision) -> CompressionDecision {
        match self {
            Self::Auto     => decision,
            Self::ForceNone => CompressionDecision::none("policy_force_none", decision.hint_used, decision.data_size),
            Self::ForceLz4  => CompressionDecision::lz4("policy_force_lz4",  decision.hint_used, decision.data_size),
            Self::ForceZstd => CompressionDecision::zstd("policy_force_zstd", decision.hint_used, decision.data_size),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_text(n: usize) -> Vec<u8> {
        let pattern = b"hello world this is a text blob ";
        pattern.iter().cycle().take(n).cloned().collect()
    }

    fn make_random_bytes(n: usize) -> Vec<u8> {
        let mut v: Vec<u8> = Vec::new();
        for i in 0..n { v.push(((i * 127 + 37) ^ (i >> 3)) as u8); }
        v
    }

    #[test]
    fn test_too_small_gives_none() {
        let d = b"hi";
        let dec = choose_compression(d, ContentHint::Text);
        assert_eq!(dec.algorithm, CompressionType::None);
    }

    #[test]
    fn test_media_always_none() {
        let d = make_text(4096);
        let dec = choose_compression(&d, ContentHint::Media);
        assert_eq!(dec.algorithm, CompressionType::None);
    }

    #[test]
    fn test_large_text_gives_zstd() {
        let d = make_text(4096);
        let dec = choose_compression(&d, ContentHint::Text);
        assert_eq!(dec.algorithm, CompressionType::Zstd);
    }

    #[test]
    fn test_high_entropy_gives_none() {
        // Données très aléatoires → pas de compression.
        let d   = make_random_bytes(4096);
        let ent = sample_entropy(&d);
        if ent > MAX_ENTROPY_PERMILLE {
            let dec = choose_compression(&d, ContentHint::Unknown);
            assert_eq!(dec.algorithm, CompressionType::None);
        }
    }

    #[test]
    fn test_policy_force_none() {
        let d   = make_text(4096);
        let dec = choose_compression(&d, ContentHint::Text);
        let dec = CompressionPolicy::ForceNone.apply(dec);
        assert_eq!(dec.algorithm, CompressionType::None);
    }

    #[test]
    fn test_compression_type_roundtrip() {
        for v in [0u8, 1, 2] {
            let ct = CompressionType::from_u8(v).unwrap();
            assert_eq!(ct.to_u8(), v);
        }
        assert!(CompressionType::from_u8(99).is_err());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// CompressionRegistry — statistiques globales de compression
// ─────────────────────────────────────────────────────────────────────────────
use core::sync::atomic::{AtomicU64, Ordering};

/// Compteurs globaux par algorithme.
pub struct CompressionRegistry {
    pub none_decisions:  AtomicU64,
    pub lz4_decisions:   AtomicU64,
    pub zstd_decisions:  AtomicU64,
    pub bytes_saved:     AtomicU64,
    pub bytes_processed: AtomicU64,
}

impl CompressionRegistry {
    pub const fn new() -> Self {
        Self {
            none_decisions:  AtomicU64::new(0),
            lz4_decisions:   AtomicU64::new(0),
            zstd_decisions:  AtomicU64::new(0),
            bytes_saved:     AtomicU64::new(0),
            bytes_processed: AtomicU64::new(0),
        }
    }

    pub fn record_decision(&self, algo: CompressionType, data_size: u64) {
        match algo {
            CompressionType::None => self.none_decisions.fetch_add(1, Ordering::Relaxed),
            CompressionType::Lz4  => self.lz4_decisions.fetch_add(1, Ordering::Relaxed),
            CompressionType::Zstd => self.zstd_decisions.fetch_add(1, Ordering::Relaxed),
        };
        self.bytes_processed.fetch_add(data_size, Ordering::Relaxed);
    }

    pub fn record_savings(&self, original: u64, compressed: u64) {
        if original > compressed {
            self.bytes_saved.fetch_add(original - compressed, Ordering::Relaxed);
        }
    }

    pub fn total_decisions(&self) -> u64 {
        self.none_decisions.load(Ordering::Relaxed)
            .saturating_add(self.lz4_decisions.load(Ordering::Relaxed))
            .saturating_add(self.zstd_decisions.load(Ordering::Relaxed))
    }

    pub fn lz4_pct(&self) -> u64 {
        let t = self.total_decisions();
        if t == 0 { return 0; }
        self.lz4_decisions.load(Ordering::Relaxed) * 100 / t
    }

    pub fn zstd_pct(&self) -> u64 {
        let t = self.total_decisions();
        if t == 0 { return 0; }
        self.zstd_decisions.load(Ordering::Relaxed) * 100 / t
    }
}

pub static COMPRESSION_REGISTRY: CompressionRegistry = CompressionRegistry::new();

// ─────────────────────────────────────────────────────────────────────────────
// is_likely_text — heuristique rapide
// ─────────────────────────────────────────────────────────────────────────────

/// Détermine si les données ressemblent à du texte ASCII/UTF-8.
/// Vérifie un échantillon de 128 octets.
pub fn is_likely_text(data: &[u8]) -> bool {
    let sample = &data[..data.len().min(128)];
    let text_chars = sample.iter().filter(|&&b| {
        (b >= 0x20 && b <= 0x7E) || b == b'\n' || b == b'\r' || b == b'\t'
    }).count();
    sample.is_empty() || (text_chars * 100 / sample.len()) >= 85
}

/// Détermine si le contenu ressemble à un fichier média (header magic connu).
pub fn is_known_media_magic(data: &[u8]) -> bool {
    if data.len() < 4 { return false; }
    matches!(&data[..4],
        // JPEG
        [0xFF, 0xD8, 0xFF, _]
        // PNG
        | [0x89, 0x50, 0x4E, 0x47]
        // RIFF (WAV, AVI)
        | [0x52, 0x49, 0x46, 0x46]
        // MP3 / MPEG
        | [0xFF, 0xFB, _, _]
        // ZIP / JAR / ODT (déjà compressé)
        | [0x50, 0x4B, 0x03, 0x04]
        // GZip
        | [0x1F, 0x8B, _, _]
    )
}

/// Déduit automatiquement un `ContentHint` à partir de l'inspection du contenu.
pub fn auto_detect_hint(data: &[u8]) -> ContentHint {
    if data.len() < 4 { return ContentHint::Unknown; }
    if is_known_media_magic(data) { return ContentHint::AlreadyCompressed; }
    if is_likely_text(data)       { return ContentHint::Text; }
    ContentHint::Binary
}

/// Sélecteur complet avec détection automatique du contenu.
pub fn choose_compression_auto(data: &[u8]) -> CompressionDecision {
    let hint = auto_detect_hint(data);
    choose_compression(data, hint)
}

#[cfg(test)]
mod tests_extra {
    use super::*;

    #[test]
    fn test_is_likely_text() {
        assert!(is_likely_text(b"fn main() { println!(\"hello\"); }"));
        assert!(!is_likely_text(&[0x00u8, 0xFF, 0x80, 0x01, 0x00, 0xFF]));
    }

    #[test]
    fn test_is_known_media() {
        assert!(is_known_media_magic(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]));
        assert!(is_known_media_magic(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]));
        assert!(!is_known_media_magic(b"hello"));
    }

    #[test]
    fn test_auto_detect_text() {
        let code = b"use alloc::vec::Vec;\nfn main() {}\n".repeat(10);
        let hint = auto_detect_hint(&code);
        assert_eq!(hint, ContentHint::Text);
    }

    #[test]
    fn test_registry_record() {
        COMPRESSION_REGISTRY.record_decision(CompressionType::Lz4, 1024);
        COMPRESSION_REGISTRY.record_decision(CompressionType::Zstd, 2048);
        let total = COMPRESSION_REGISTRY.total_decisions();
        assert!(total >= 2);
    }
}
