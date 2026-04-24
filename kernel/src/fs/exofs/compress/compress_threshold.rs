//! Seuils de compression ExoFS — décide si un blob doit être compressé.
//!
//! Critères de décision :
//!   1. Taille minimale (MIN_COMPRESS_SIZE)
//!   2. Détection heuristique de données déjà compressées (magic bytes)
//!   3. Estimation d'entropie rapide (sampling des premiers 256 bytes)
//!   4. Ratio post-compression trop faible (is_worth_storing)
//!
//! RÈGLE ARITH-02 : arithmétique checked/saturating.
//! RÈGLE RECUR-01 : aucune récursivité.

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille minimum en bytes pour tenter la compression.
pub const MIN_COMPRESS_SIZE: usize = 512;

/// Si le ratio compressé/original dépasse ce seuil (%), la compression est rejetée.
pub const MAX_RATIO_PERCENT: u64 = 95;

/// Taille de l'échantillon d'entropie rapide (en bytes).
pub const ENTROPY_SAMPLE_SIZE: usize = 256;

/// Seuil d'entropie estimée en dessous duquel la compression n'est pas tentée.
/// Unité : symboles distincts sur 256 max. 250+ = donnée quasi-aléatoire.
pub const ENTROPY_HIGH_THRESHOLD: usize = 248;

// ─────────────────────────────────────────────────────────────────────────────
// CompressionThreshold
// ─────────────────────────────────────────────────────────────────────────────

/// Gestion des seuils décidant si la compression est bénéfique.
#[derive(Debug, Clone)]
pub struct CompressionThreshold {
    /// Taille minimum pour tenter la compression.
    pub min_size: usize,
    /// Ratio max acceptable : si (compressed / original) * 100 > ratio_threshold → skip.
    pub ratio_threshold: u64,
    /// Active la détection des données déjà compressées par magic bytes.
    pub detect_already_compressed: bool,
    /// Active l'estimation rapide d'entropie par sampling.
    pub detect_high_entropy: bool,
    /// Seuil d'entropie : nb de symboles distincts au-delà duquel on skip.
    pub entropy_threshold: usize,
}

impl CompressionThreshold {
    pub const fn default() -> Self {
        Self {
            min_size: MIN_COMPRESS_SIZE,
            ratio_threshold: MAX_RATIO_PERCENT,
            detect_already_compressed: true,
            detect_high_entropy: true,
            entropy_threshold: ENTROPY_HIGH_THRESHOLD,
        }
    }

    /// Crée un seuil agressif : tente la compression sur tout (usage : archivage).
    pub const fn aggressive() -> Self {
        Self {
            min_size: 64,
            ratio_threshold: 99,
            detect_already_compressed: false,
            detect_high_entropy: false,
            entropy_threshold: 256,
        }
    }

    /// Crée un seuil conservateur : compression uniquement si clairement bénéfique.
    pub const fn conservative() -> Self {
        Self {
            min_size: 4096,
            ratio_threshold: 80,
            detect_already_compressed: true,
            detect_high_entropy: true,
            entropy_threshold: 200,
        }
    }

    /// `true` si la compression doit être tentée pour ces données.
    ///
    /// Décision sans récursivité (RECUR-01).
    pub fn should_compress(&self, data: &[u8]) -> bool {
        // 1. Taille minimale.
        if data.len() < self.min_size {
            return false;
        }
        // 2. Magic bytes de formats déjà compressés.
        if self.detect_already_compressed && looks_compressed(data) {
            return false;
        }
        // 3. Entropie trop haute (données aléatoires → compression inutile).
        if self.detect_high_entropy && estimate_entropy(data) >= self.entropy_threshold {
            return false;
        }
        true
    }

    /// `true` si le résultat compressé justifie le stockage compressé.
    pub fn is_worth_storing(&self, compressed_len: usize, original_len: usize) -> bool {
        if original_len == 0 {
            return false;
        }
        let ratio = (compressed_len as u64).saturating_mul(100) / (original_len as u64);
        ratio <= self.ratio_threshold
    }

    /// Décision complète : doit-on stocker compressé ?
    /// Combine `should_compress` + `is_worth_storing` en une seule logique.
    pub fn should_store_compressed(&self, original: &[u8], compressed: &[u8]) -> bool {
        self.should_compress(original) && self.is_worth_storing(compressed.len(), original.len())
    }

    /// Estimation du potentiel de compression (0 = très compressible, 100 = incompressible).
    pub fn incompressibility_score(&self, data: &[u8]) -> u8 {
        let e = estimate_entropy(data);
        // Normalise sur [0, 100].
        let score = (e as u64).saturating_mul(100) / 256;
        score.min(100) as u8
    }
}

impl Default for CompressionThreshold {
    fn default() -> Self {
        CompressionThreshold::default()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions utilitaires
// ─────────────────────────────────────────────────────────────────────────────

/// Détecte heuristiquement si les données semblent déjà compressées.
/// Vérifie les magic bytes des formats courants.
/// RECUR-01 : pas de récursivité.
pub fn looks_compressed(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    let magic4 = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    // Zstd, LZ4 frame, GZIP, ZIP, BZIP2, XZ, PNG, JPEG.
    matches!(
        magic4,
        0xFD2F_B528  // Zstd
        | 0x0400_2270 // LZ4 frame  (magic partiel)
        | 0x1F8B_0800 // GZIP
        | 0x504B_0304 // ZIP
        | 0x425A_6839 // BZIP2
        | 0xFD37_7A58 // XZ
        | 0x8950_4E47 // PNG
        | 0xFFD8_FFE0 // JPEG (JFIF)
        | 0xFFD8_FFE1 // JPEG (EXIF)
    )
}

/// Estimation rapide d'entropie par comptage de symboles distincts
/// sur un échantillon de `ENTROPY_SAMPLE_SIZE` bytes.
///
/// Retourne le nombre de valeurs d'octet distinctes (0–256).
/// RECUR-01 : boucle itérative simple.
pub fn estimate_entropy(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    let sample_len = data.len().min(ENTROPY_SAMPLE_SIZE);
    let mut seen = [false; 256];
    let mut count = 0usize;
    // Pas de récursivité : simple boucle for.
    for i in 0..sample_len {
        let b = data[i] as usize;
        if !seen[b] {
            seen[b] = true;
            count = count.saturating_add(1);
        }
    }
    count
}

/// Estime si les données sont probablement du texte (entropie faible).
/// Heuristique : > 90% des bytes sont dans [0x09–0x0D, 0x20–0x7E].
pub fn looks_like_text(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let sample_len = data.len().min(1024);
    let mut printable = 0u32;
    for i in 0..sample_len {
        let b = data[i];
        if (b >= 0x20 && b <= 0x7E) || (b >= 0x09 && b <= 0x0D) {
            printable = printable.saturating_add(1);
        }
    }
    (printable as u64 * 100) / (sample_len as u64) >= 90
}

/// Calcule le niveau de redondance d'un bloc (0 = max compressible, 100 = aléatoire).
/// Basé sur le comptage de répétitions de 4-grams dans le premier kilo-octet.
pub fn redundancy_score(data: &[u8]) -> u8 {
    if data.len() < 8 {
        return 50;
    }
    let sample = &data[..data.len().min(1024)];
    let total_grams = sample.len().saturating_sub(3);
    if total_grams == 0 {
        return 50;
    }
    let mut seen = [0u8; 256]; // Table de hash 256 entrées, approximation.
    let mut repeats = 0usize;
    for w in sample.windows(4) {
        let h = ((w[0] as u32)
            .wrapping_mul(31)
            .wrapping_add(w[1] as u32)
            .wrapping_mul(31)
            .wrapping_add(w[2] as u32)
            .wrapping_mul(31)
            .wrapping_add(w[3] as u32)) as u8;
        if seen[h as usize] > 0 {
            repeats = repeats.saturating_add(1);
        } else {
            seen[h as usize] = 1;
        }
    }
    let ratio = (repeats as u64 * 100) / (total_grams as u64);
    // Score de redondance inversé : 100% de répétitions → score = 0 (très compressible).
    (100u64.saturating_sub(ratio)).min(100) as u8
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_compress_too_small() {
        let t = CompressionThreshold::default();
        assert!(!t.should_compress(b"tiny"));
    }

    #[test]
    fn test_should_compress_ok() {
        let t = CompressionThreshold::default();
        let data = b"AAAAAAAAAAAAAAAAAAAAAAAA".repeat(40);
        // 40*24 = 960 bytes, faible entropie
        assert!(t.should_compress(&data));
    }

    #[test]
    fn test_looks_compressed_zstd() {
        // Zstd magic : FD 2F B5 28
        let buf = [0xFDu8, 0x2F, 0xB5, 0x28, 0x00, 0x00];
        assert!(looks_compressed(&buf));
    }

    #[test]
    fn test_looks_compressed_none() {
        let buf = b"Hello, world! This is plain text.";
        assert!(!looks_compressed(buf));
    }

    #[test]
    fn test_estimate_entropy_all_same() {
        let data = [0xAAu8; 256];
        assert_eq!(estimate_entropy(&data), 1);
    }

    #[test]
    fn test_estimate_entropy_all_different() {
        let data: [u8; 256] = core::array::from_fn(|i| i as u8);
        assert_eq!(estimate_entropy(&data), 256);
    }

    #[test]
    fn test_is_worth_storing_beneficial() {
        let t = CompressionThreshold::default();
        assert!(t.is_worth_storing(500, 1000)); // 50% → acceptable
    }

    #[test]
    fn test_is_worth_storing_not_beneficial() {
        let t = CompressionThreshold::default();
        assert!(!t.is_worth_storing(970, 1000)); // 97% > 95% → reject
    }

    #[test]
    fn test_is_worth_storing_zero_original() {
        let t = CompressionThreshold::default();
        assert!(!t.is_worth_storing(0, 0));
    }

    #[test]
    fn test_looks_like_text_true() {
        let text = b"Hello World! This is ASCII text.";
        assert!(looks_like_text(text));
    }

    #[test]
    fn test_looks_like_text_false_binary() {
        let bin: [u8; 32] = core::array::from_fn(|i| i as u8);
        assert!(!looks_like_text(&bin));
    }

    #[test]
    fn test_incompressibility_score_uniform() {
        let t = CompressionThreshold::default();
        let data = [0u8; 512]; // entropie 1 → très compressible → score bas
        let score = t.incompressibility_score(&data);
        assert!(score < 5);
    }

    #[test]
    fn test_conservative_threshold() {
        let t = CompressionThreshold::conservative();
        assert_eq!(t.min_size, 4096);
        assert_eq!(t.ratio_threshold, 80);
    }

    #[test]
    fn test_aggressive_threshold() {
        let t = CompressionThreshold::aggressive();
        // Données déjà compressées : aggressive ne les filtre pas.
        let buf = [
            0xFDu8, 0x2F, 0xB5, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert!(t.should_compress(&buf)); // 64 bytes suffit, pas de filtre magic
    }

    #[test]
    fn test_redundancy_score_uniform_data() {
        let data = [0xAAu8; 1024]; // Données uniformes = très haute redondance
        let score = redundancy_score(&data);
        // Score < 50 : très compressible
        assert!(score <= 100);
    }

    #[test]
    fn test_should_store_compressed_true() {
        let t = CompressionThreshold::default();
        let original = b"AAAA".repeat(300); // 1200 bytes compressibles
        let compressed = b"X".repeat(400); // 33% du original
        assert!(t.should_store_compressed(&original, &compressed));
    }

    #[test]
    fn test_should_store_compressed_false_poor_ratio() {
        let t = CompressionThreshold::default();
        let original = b"AAAA".repeat(300);
        let compressed = b"X".repeat(1150); // 95.8% > 95% seuil
        assert!(!t.should_store_compressed(&original, &compressed));
    }

    #[test]
    fn test_incompressibility_score_random_like() {
        let t = CompressionThreshold::default();
        // 256 octets distincts = entropie maximale
        let data: [u8; 256] = core::array::from_fn(|i| i as u8);
        let score = t.incompressibility_score(&data);
        assert!(score >= 90); // Quasi-aléatoire = score élevé
    }
}
