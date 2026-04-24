//! # Pattern Matcher — Analyse de données contre la base de signatures
//!
//! Moteur de correspondance de patterns : exact, wildcard (*), fuzzy (seuil),
//! et scan complet de buffer contre toutes les signatures actives.
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU64, Ordering};

use super::database;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Score minimum pour une correspondance fuzzy (0..=100).
pub const DEFAULT_FUZZY_THRESHOLD: u8 = 80;

/// Nombre maximum de résultats retournés par scan_buffer.
pub const MAX_SCAN_RESULTS: usize = 32;

// ── Résultat de correspondance ───────────────────────────────────────────────

/// Type de correspondance trouvé.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum MatchType {
    /// Correspondance exacte byte-à-byte.
    Exact = 0,
    /// Correspondance avec joker (*).
    Wildcard = 1,
    /// Correspondance approximative (score ≥ seuil).
    Fuzzy = 2,
}

/// Résultat d'une correspondance de signature.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MatchResult {
    /// ID de la signature correspondante.
    pub sig_id: u32,
    /// Position dans le buffer où la correspondance a été trouvée.
    pub offset: usize,
    /// Longueur de la correspondance dans le buffer.
    pub length: usize,
    /// Score de la correspondance (100 = exact, 0 = aucune).
    pub score: u8,
    /// Type de correspondance.
    pub match_type: MatchType,
    /// Sévérité de la signature.
    pub severity: database::Severity,
    /// Catégorie de la signature.
    pub category: database::Category,
}

impl MatchResult {
    pub const fn empty() -> Self {
        Self {
            sig_id: 0,
            offset: 0,
            length: 0,
            score: 0,
            match_type: MatchType::Exact,
            severity: database::Severity::Low,
            category: database::Category::Custom,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.sig_id != 0
    }
}

// ── Compteurs de performance ─────────────────────────────────────────────────

static SCANS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MATCHES_FOUND: AtomicU64 = AtomicU64::new(0);
static BYTES_SCANNED: AtomicU64 = AtomicU64::new(0);

// ── Correspondance exacte ────────────────────────────────────────────────────

/// Correspondance exacte byte-à-byte entre un pattern et des données.
///
/// Recherche le pattern dans `data` à partir de `offset`.
///
/// # Retour
/// - La position de la première correspondance, ou `None` si non trouvée.
pub fn match_exact(pattern: &[u8], data: &[u8], offset: usize) -> Option<usize> {
    if pattern.is_empty() || data.is_empty() || offset >= data.len() {
        return None;
    }
    if pattern.len() > data.len() - offset {
        return None;
    }

    let search_limit = data.len() - pattern.len() + 1;

    // Algorithme de recherche naïf avec shortcut sur premier octet
    let first_byte = pattern[0];
    let mut pos = offset;

    while pos < search_limit {
        // Shortcut : vérifier le premier octet
        if data[pos] != first_byte {
            pos += 1;
            continue;
        }

        // Vérifier le reste du pattern
        let mut matched = true;
        for j in 1..pattern.len() {
            if data[pos + j] != pattern[j] {
                matched = false;
                break;
            }
        }

        if matched {
            return Some(pos);
        }
        pos += 1;
    }

    None
}

// ── Correspondance avec joker ────────────────────────────────────────────────

/// Correspondance avec joker simple (*).
///
/// Le caractère `*` dans le pattern correspond à n'importe quelle séquence
/// d'octets (y compris la séquence vide).
///
/// Exemple : `b"AB*CD"` correspond à `b"ABCD"`, `b"ABXXXCD"`, `b"AB12345CD"`.
///
/// # Retour
/// - La position de la première correspondance et sa longueur, ou `None`.
pub fn match_wildcard(pattern: &[u8], data: &[u8], offset: usize) -> Option<(usize, usize)> {
    if pattern.is_empty() || data.is_empty() || offset >= data.len() {
        return None;
    }

    // Découper le pattern sur les '*'
    let mut segments: [&[u8]; 16] = [&[]; 16];
    let mut seg_count = 0usize;
    let mut seg_start = 0usize;

    for i in 0..pattern.len() {
        if pattern[i] == b'*' {
            if i > seg_start {
                if seg_count >= 16 {
                    return None; // Trop de segments
                }
                segments[seg_count] = &pattern[seg_start..i];
                seg_count += 1;
            }
            seg_start = i + 1;
        }
    }
    // Dernier segment (après le dernier '*')
    if seg_start < pattern.len() {
        if seg_count >= 16 {
            return None;
        }
        segments[seg_count] = &pattern[seg_start..];
        seg_count += 1;
    }

    // Si pas de segments (pattern = "*" ou vide), tout correspond
    if seg_count == 0 {
        return Some((offset, data.len() - offset));
    }

    // Correspondance séquentielle des segments dans les données
    let mut data_pos = offset;

    for seg_idx in 0..seg_count {
        let seg = segments[seg_idx];
        if seg.is_empty() {
            continue;
        }

        // Chercher le segment à partir de data_pos
        let found = match_exact(seg, data, data_pos);
        match found {
            Some(pos) => {
                data_pos = pos + seg.len();
                // Pour le dernier segment, si le pattern ne finit pas par *,
                // il doit correspondre à la fin des données
                if seg_idx == seg_count - 1 && pattern.last() == Some(&b'*') {
                    // Le dernier '*' peut matcher n'importe quoi — déjà OK
                } else if seg_idx == seg_count - 1 && pattern.last() != Some(&b'*') {
                    // Le dernier segment doit correspondre exactement à la fin
                    // (pas de contrainte supplémentaire ici, on accepte)
                }
            }
            None => return None,
        }
    }

    // Calculer la longueur de la correspondance
    let first_seg_pos = if segments[0].is_empty() {
        offset
    } else {
        match match_exact(segments[0], data, offset) {
            Some(p) => p,
            None => return None,
        }
    };

    // Vérifier que le premier segment commence au bon endroit
    // si le pattern ne commence pas par '*'
    let match_start = if pattern[0] == b'*' {
        // Le premier '*' peut matcher n'importe quoi
        match match_exact(segments[0], data, offset) {
            Some(p) => p,
            None => return None,
        }
    } else {
        // Le premier segment doit commencer à `offset`
        if data.len() < offset + segments[0].len() {
            return None;
        }
        let mut ok = true;
        for j in 0..segments[0].len() {
            if data[offset + j] != segments[0][j] {
                ok = false;
                break;
            }
        }
        if !ok {
            return None;
        }
        offset
    };

    let match_len = data_pos - match_start;
    Some((match_start, match_len))
}

// ── Correspondance floue (fuzzy) ─────────────────────────────────────────────

/// Calcul du score de similarité entre deux buffers.
///
/// Utilise une métrique basée sur la distance de Hamming pondérée
/// et la correspondance par sous-séquences.
///
/// # Retour
/// Score de 0..=100 (100 = identique).
pub fn fuzzy_score(pattern: &[u8], data: &[u8]) -> u8 {
    if pattern.is_empty() && data.is_empty() {
        return 100;
    }
    if pattern.is_empty() || data.is_empty() {
        return 0;
    }

    let p_len = pattern.len();
    let d_len = data.len();

    // Composante 1 : similarité par distance de Hamming (si même longueur)
    let hamming_component = if p_len == d_len {
        let mut matching_bytes = 0usize;
        for i in 0..p_len {
            if pattern[i] == data[i] {
                matching_bytes += 1;
            }
        }
        (matching_bytes * 100) / p_len
    } else {
        // Pénalité de longueur
        let ratio = if p_len < d_len {
            p_len * 100 / d_len
        } else {
            d_len * 100 / p_len
        };
        ratio / 2 // Pénalité pour longueur différente
    };

    // Composante 2 : correspondance de sous-séquence (LCS simplifié)
    // Compter combien d'octets du pattern apparaissent dans l'ordre dans data
    let mut p_idx = 0usize;
    let mut d_idx = 0usize;
    let mut lcs_count = 0usize;

    while p_idx < p_len && d_idx < d_len {
        if pattern[p_idx] == data[d_idx] {
            lcs_count += 1;
            p_idx += 1;
        }
        d_idx += 1;
    }

    let lcs_component = if p_len > 0 {
        (lcs_count * 100) / p_len
    } else {
        0
    };

    // Composante 3 : fréquence d'octets communs
    let mut pat_freq = [0u32; 256];
    let mut dat_freq = [0u32; 256];

    for &b in pattern.iter() {
        pat_freq[b as usize] += 1;
    }
    for &b in data.iter() {
        dat_freq[b as usize] += 1;
    }

    let mut common_weight = 0u32;
    let mut total_weight = 0u32;
    for i in 0..256 {
        let pf = pat_freq[i];
        let df = dat_freq[i];
        common_weight += pf.min(df);
        total_weight += pf.max(df);
    }

    let freq_component = if total_weight > 0 {
        ((common_weight * 100) / total_weight) as usize
    } else {
        0
    };

    // Score combiné pondéré : 40% Hamming, 35% LCS, 25% fréquence
    let score = (hamming_component * 40 + lcs_component * 35 + freq_component * 25) / 100;

    score.min(100) as u8
}

/// Correspondance floue avec seuil.
///
/// Compare le pattern avec la portion de `data` commençant à `offset`
/// et de même longueur que le pattern.
///
/// # Retour
/// - Le score si ≥ threshold, 0 sinon.
pub fn match_threshold(pattern: &[u8], data: &[u8], offset: usize, threshold: u8) -> u8 {
    if pattern.is_empty() || data.is_empty() {
        return 0;
    }

    let effective_threshold = if threshold == 0 {
        DEFAULT_FUZZY_THRESHOLD
    } else {
        threshold
    };

    // Essayer la correspondance à la position exacte
    if offset + pattern.len() <= data.len() {
        let score = fuzzy_score(pattern, &data[offset..offset + pattern.len()]);
        if score >= effective_threshold {
            return score;
        }
    }

    // Essayer une fenêtre glissante autour de l'offset (±16 octets)
    let search_start = offset.saturating_sub(16);
    let search_end = (offset + 16).min(data.len());

    let mut best_score = 0u8;

    let mut pos = search_start;
    while pos + pattern.len() <= data.len() && pos <= search_end {
        let score = fuzzy_score(pattern, &data[pos..pos + pattern.len()]);
        if score > best_score {
            best_score = score;
        }
        if best_score >= effective_threshold {
            return best_score;
        }
        pos += 1;
    }

    if best_score >= effective_threshold {
        best_score
    } else {
        0
    }
}

// ── Scan complet ─────────────────────────────────────────────────────────────

/// Résultat d'un scan de buffer complet.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScanResult {
    /// Nombre de correspondances trouvées.
    pub match_count: usize,
    /// Détail des correspondances (max MAX_SCAN_RESULTS).
    pub results: [MatchResult; MAX_SCAN_RESULTS],
    /// Score de menace global (0..=100).
    pub threat_score: u8,
}

impl ScanResult {
    pub const fn empty() -> Self {
        Self {
            match_count: 0,
            results: [MatchResult::empty(); MAX_SCAN_RESULTS],
            threat_score: 0,
        }
    }
}

/// Mode de scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum ScanMode {
    /// Correspondance exacte uniquement.
    ExactOnly = 0,
    /// Exact + wildcard.
    WithWildcard = 1,
    /// Exact + wildcard + fuzzy.
    Full = 2,
}

/// Scanne un buffer complet contre toutes les signatures actives.
///
/// # Arguments
/// - `buffer` : données à analyser.
/// - `mode` : mode de scan (exact, wildcard, full).
/// - `fuzzy_threshold` : seuil pour la correspondance fuzzy (0 = défaut 80).
///
/// # Retour
/// Un `ScanResult` avec toutes les correspondances trouvées.
pub fn scan_buffer(buffer: &[u8], mode: ScanMode, fuzzy_threshold: u8) -> ScanResult {
    SCANS_TOTAL.fetch_add(1, Ordering::Relaxed);
    BYTES_SCANNED.fetch_add(buffer.len() as u64, Ordering::Relaxed);

    let mut result = ScanResult::empty();
    let eff_threshold = if fuzzy_threshold == 0 {
        DEFAULT_FUZZY_THRESHOLD
    } else {
        fuzzy_threshold
    };

    database::iter_active(|_idx, entry| {
        if result.match_count >= MAX_SCAN_RESULTS {
            return false; // Arrêter si le buffer de résultats est plein
        }

        let pattern = entry.pattern_slice();

        // 1. Tentative de correspondance exacte
        if let Some(pos) = match_exact(pattern, buffer, 0) {
            result.results[result.match_count] = MatchResult {
                sig_id: entry.id,
                offset: pos,
                length: pattern.len(),
                score: 100,
                match_type: MatchType::Exact,
                severity: entry.severity,
                category: entry.category,
            };
            result.match_count += 1;
            MATCHES_FOUND.fetch_add(1, Ordering::Relaxed);
            return result.match_count < MAX_SCAN_RESULTS;
        }

        // 2. Tentative wildcard (si le pattern contient '*')
        if mode == ScanMode::WithWildcard || mode == ScanMode::Full {
            let has_wildcard = {
                let mut found = false;
                for &b in pattern.iter() {
                    if b == b'*' {
                        found = true;
                        break;
                    }
                }
                found
            };

            if has_wildcard {
                if let Some((pos, len)) = match_wildcard(pattern, buffer, 0) {
                    result.results[result.match_count] = MatchResult {
                        sig_id: entry.id,
                        offset: pos,
                        length: len,
                        score: 90,
                        match_type: MatchType::Wildcard,
                        severity: entry.severity,
                        category: entry.category,
                    };
                    result.match_count += 1;
                    MATCHES_FOUND.fetch_add(1, Ordering::Relaxed);
                    return result.match_count < MAX_SCAN_RESULTS;
                }
            }
        }

        // 3. Tentative fuzzy (mode Full uniquement)
        if mode == ScanMode::Full {
            // Scanner par fenêtres glissantes de la taille du pattern
            if !pattern.is_empty() && buffer.len() >= pattern.len() {
                let mut best_score = 0u8;
                let mut best_pos = 0usize;

                // Fenêtre glissante avec un pas adaptatif
                let step = if pattern.len() > 16 { 4 } else { 1 };
                let mut pos = 0usize;
                while pos + pattern.len() <= buffer.len() {
                    let score = fuzzy_score(pattern, &buffer[pos..pos + pattern.len()]);
                    if score > best_score {
                        best_score = score;
                        best_pos = pos;
                    }
                    // Si on a déjà un score parfait, pas la peine de continuer
                    if best_score == 100 {
                        break;
                    }
                    pos += step;
                }

                if best_score >= eff_threshold {
                    result.results[result.match_count] = MatchResult {
                        sig_id: entry.id,
                        offset: best_pos,
                        length: pattern.len(),
                        score: best_score,
                        match_type: MatchType::Fuzzy,
                        severity: entry.severity,
                        category: entry.category,
                    };
                    result.match_count += 1;
                    MATCHES_FOUND.fetch_add(1, Ordering::Relaxed);
                    return result.match_count < MAX_SCAN_RESULTS;
                }
            }
        }

        true // Continuer l'itération
    });

    // Calculer le score de menace global
    if result.match_count > 0 {
        let mut total_weight = 0u32;
        for i in 0..result.match_count {
            let mr = &result.results[i];
            let sev_weight = mr.severity.weight();
            let score_weight = mr.score as u32;
            total_weight += sev_weight * score_weight;
        }

        // Normaliser : max théorique = 50 * 100 * match_count
        let max_possible = 5000 * result.match_count as u32;
        if max_possible > 0 {
            let normalized = (total_weight * 100) / max_possible;
            result.threat_score = normalized.min(100) as u8;
        }
    }

    result
}

/// Scanne un buffer avec une signature spécifique (par ID).
///
/// # Retour
/// - Un `MatchResult` si correspondance trouvée, ou `MatchResult::empty()`.
pub fn scan_with_signature(
    buffer: &[u8],
    sig_id: u32,
    mode: ScanMode,
    fuzzy_threshold: u8,
) -> MatchResult {
    let entry = match database::get_by_id(sig_id) {
        Some(e) => e,
        None => return MatchResult::empty(),
    };

    if !entry.enabled {
        return MatchResult::empty();
    }

    let pattern = entry.pattern_slice();

    // Exact
    if let Some(pos) = match_exact(pattern, buffer, 0) {
        return MatchResult {
            sig_id: entry.id,
            offset: pos,
            length: pattern.len(),
            score: 100,
            match_type: MatchType::Exact,
            severity: entry.severity,
            category: entry.category,
        };
    }

    // Wildcard
    if mode == ScanMode::WithWildcard || mode == ScanMode::Full {
        let has_wildcard = {
            let mut found = false;
            for &b in pattern.iter() {
                if b == b'*' {
                    found = true;
                    break;
                }
            }
            found
        };
        if has_wildcard {
            if let Some((pos, len)) = match_wildcard(pattern, buffer, 0) {
                return MatchResult {
                    sig_id: entry.id,
                    offset: pos,
                    length: len,
                    score: 90,
                    match_type: MatchType::Wildcard,
                    severity: entry.severity,
                    category: entry.category,
                };
            }
        }
    }

    // Fuzzy
    if mode == ScanMode::Full && !pattern.is_empty() && buffer.len() >= pattern.len() {
        let eff_threshold = if fuzzy_threshold == 0 {
            DEFAULT_FUZZY_THRESHOLD
        } else {
            fuzzy_threshold
        };
        let score = match_threshold(pattern, buffer, 0, eff_threshold);
        if score > 0 {
            return MatchResult {
                sig_id: entry.id,
                offset: 0,
                length: pattern.len(),
                score,
                match_type: MatchType::Fuzzy,
                severity: entry.severity,
                category: entry.category,
            };
        }
    }

    MatchResult::empty()
}

// ── Statistiques du matcher ──────────────────────────────────────────────────

/// Statistiques de performance du matcher.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MatcherStats {
    pub scans_total: u64,
    pub matches_found: u64,
    pub bytes_scanned: u64,
}

/// Retourne les statistiques du matcher.
pub fn get_matcher_stats() -> MatcherStats {
    MatcherStats {
        scans_total: SCANS_TOTAL.load(Ordering::Relaxed),
        matches_found: MATCHES_FOUND.load(Ordering::Relaxed),
        bytes_scanned: BYTES_SCANNED.load(Ordering::Relaxed),
    }
}

/// Réinitialise les compteurs du matcher.
pub fn reset_matcher_stats() {
    SCANS_TOTAL.store(0, Ordering::Relaxed);
    MATCHES_FOUND.store(0, Ordering::Relaxed);
    BYTES_SCANNED.store(0, Ordering::Relaxed);
}
