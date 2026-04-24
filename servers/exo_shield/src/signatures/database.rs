//! # Signature Database — Stockage statique des signatures de menaces
//!
//! Base de données no_std avec tableau statique de 256 entrées max.
//! Opérations : ajout, recherche par pattern, activation/désactivation, statistiques.
//!
//! ## Règles
//! - NS-01 : uniquement core::sync::atomic + spin, pas de heap
//! - CAP-01 : vérification de capacité avant toute modification
//! - Zéro stub, zéro TODO, zéro placeholder

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use spin::Mutex;

// ── Constantes ───────────────────────────────────────────────────────────────

/// Nombre maximum de signatures dans la base.
pub const MAX_SIGNATURES: usize = 256;

/// Taille maximale d'un pattern en octets.
pub const PATTERN_SIZE: usize = 32;

// ── Sévérité ─────────────────────────────────────────────────────────────────

/// Niveau de sévérité d'une signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Severity {
    Low = 0,
    Medium = 1,
    High = 2,
    Critical = 3,
}

impl Severity {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Severity::Low),
            1 => Some(Severity::Medium),
            2 => Some(Severity::High),
            3 => Some(Severity::Critical),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn weight(self) -> u32 {
        match self {
            Severity::Low => 1,
            Severity::Medium => 5,
            Severity::High => 15,
            Severity::Critical => 50,
        }
    }
}

// ── Catégorie ────────────────────────────────────────────────────────────────

/// Catégorie de menace associée à une signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Category {
    Malware = 0,
    Exploit = 1,
    Backdoor = 2,
    Ransomware = 3,
    Spyware = 4,
    Rootkit = 5,
    Network = 6,
    Filesystem = 7,
    Memory = 8,
    Custom = 9,
}

impl Category {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Category::Malware),
            1 => Some(Category::Exploit),
            2 => Some(Category::Backdoor),
            3 => Some(Category::Ransomware),
            4 => Some(Category::Spyware),
            5 => Some(Category::Rootkit),
            6 => Some(Category::Network),
            7 => Some(Category::Filesystem),
            8 => Some(Category::Memory),
            9 => Some(Category::Custom),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Nombre de catégories distinctes.
pub const CATEGORY_COUNT: usize = 10;

// ── Entrée de signature ──────────────────────────────────────────────────────

/// Une signature dans la base de données.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SignatureEntry {
    /// Identifiant unique de la signature.
    pub id: u32,
    /// Pattern binaire de la signature (rempli de zéros au-delà de pattern_len).
    pub pattern: [u8; PATTERN_SIZE],
    /// Longueur réelle du pattern (1..=32).
    pub pattern_len: u8,
    /// Niveau de sévérité.
    pub severity: Severity,
    /// Catégorie de menace.
    pub category: Category,
    /// La signature est-elle active ?
    pub enabled: bool,
    /// Réservé pour alignement.
    _reserved: [u8; 2],
}

impl SignatureEntry {
    /// Crée une entrée vide (désactivée, pattern nul).
    pub const fn empty() -> Self {
        Self {
            id: 0,
            pattern: [0u8; PATTERN_SIZE],
            pattern_len: 0,
            severity: Severity::Low,
            category: Category::Custom,
            enabled: false,
            _reserved: [0; 2],
        }
    }

    /// Crée une nouvelle entrée avec les paramètres donnés.
    pub fn new(id: u32, pattern: &[u8], severity: Severity, category: Category) -> Self {
        let len = pattern.len().min(PATTERN_SIZE);
        let mut pat = [0u8; PATTERN_SIZE];
        pat[..len].copy_from_slice(&pattern[..len]);
        Self {
            id,
            pattern: pat,
            pattern_len: len as u8,
            severity,
            category,
            enabled: true,
            _reserved: [0; 2],
        }
    }

    /// Retourne le slice du pattern réel.
    pub fn pattern_slice(&self) -> &[u8] {
        let len = self.pattern_len as usize;
        if len == 0 || len > PATTERN_SIZE {
            &[]
        } else {
            &self.pattern[..len]
        }
    }

    /// Vérifie si l'entrée est valide (pattern_len > 0 et id != 0).
    pub fn is_valid(&self) -> bool {
        self.id != 0 && self.pattern_len > 0 && (self.pattern_len as usize) <= PATTERN_SIZE
    }
}

// ── Statistiques ─────────────────────────────────────────────────────────────

/// Statistiques sur la base de signatures.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SignatureStats {
    /// Nombre total de signatures (valides).
    pub total: usize,
    /// Nombre de signatures actives (valides + enabled).
    pub active: usize,
    /// Répartition par sévérité (index = Severity as usize).
    pub by_severity: [usize; 4],
    /// Répartition par catégorie (index = Category as usize).
    pub by_category: [usize; CATEGORY_COUNT],
}

impl SignatureStats {
    pub const fn empty() -> Self {
        Self {
            total: 0,
            active: 0,
            by_severity: [0; 4],
            by_category: [0; CATEGORY_COUNT],
        }
    }
}

// ── Base de données ──────────────────────────────────────────────────────────

/// Base de données de signatures, stockée dans un tableau statique protégé par Mutex.
static SIG_DB: Mutex<SignatureDatabaseInner> = Mutex::new(SignatureDatabaseInner::new());

/// Compteur pour le prochain ID de signature.
static NEXT_SIG_ID: AtomicU32 = AtomicU32::new(1);

/// Nombre de signatures actives (cache atomique).
static ACTIVE_SIG_COUNT: AtomicU32 = AtomicU32::new(0);

/// Base de données interne (sans Mutex, pour accès via SIG_DB).
#[repr(C)]
struct SignatureDatabaseInner {
    entries: [SignatureEntry; MAX_SIGNATURES],
    count: usize,
}

impl SignatureDatabaseInner {
    const fn new() -> Self {
        Self {
            entries: [SignatureEntry::empty(); MAX_SIGNATURES],
            count: 0,
        }
    }
}

// ── API publique ─────────────────────────────────────────────────────────────

/// Ajoute une signature à la base de données.
///
/// # Retour
/// - L'ID de la signature si succès (non-zéro).
/// - 0 si la base est pleine ou le pattern est vide.
pub fn add_signature(pattern: &[u8], severity: Severity, category: Category) -> u32 {
    if pattern.is_empty() || pattern.len() > PATTERN_SIZE {
        return 0;
    }

    let mut db = SIG_DB.lock();

    // Vérifier si le pattern existe déjà (doublon)
    let plen = pattern.len();
    for i in 0..db.count {
        let entry = &db.entries[i];
        if entry.pattern_len as usize == plen {
            let mut match_found = true;
            for j in 0..plen {
                if entry.pattern[j] != pattern[j] {
                    match_found = false;
                    break;
                }
            }
            if match_found {
                return 0; // Doublon refusé
            }
        }
    }

    // Vérifier la capacité
    if db.count >= MAX_SIGNATURES {
        // Chercher un slot libéré (entrées avec id == 0)
        for i in 0..MAX_SIGNATURES {
            if db.entries[i].id == 0 {
                let id = NEXT_SIG_ID.fetch_add(1, Ordering::AcqRel);
                db.entries[i] = SignatureEntry::new(id, pattern, severity, category);
                ACTIVE_SIG_COUNT.fetch_add(1, Ordering::Relaxed);
                return id;
            }
        }
        return 0; // Vraiment plein
    }

    let id = NEXT_SIG_ID.fetch_add(1, Ordering::AcqRel);
    let idx = db.count;
    db.entries[idx] = SignatureEntry::new(id, pattern, severity, category);
    db.count += 1;
    ACTIVE_SIG_COUNT.fetch_add(1, Ordering::Relaxed);
    id
}

/// Ajoute une signature avec un ID spécifié (pour restauration/rollback).
///
/// # Retour
/// - L'ID si succès, 0 si échec.
pub fn add_signature_with_id(
    id: u32,
    pattern: &[u8],
    severity: Severity,
    category: Category,
    enabled: bool,
) -> u32 {
    if id == 0 || pattern.is_empty() || pattern.len() > PATTERN_SIZE {
        return 0;
    }

    let mut db = SIG_DB.lock();

    // Chercher un slot libre
    let slot = if db.count < MAX_SIGNATURES {
        db.count
    } else {
        // Chercher un slot avec id == 0
        let mut found = None;
        for i in 0..MAX_SIGNATURES {
            if db.entries[i].id == 0 {
                found = Some(i);
                break;
            }
        }
        match found {
            Some(s) => s,
            None => return 0,
        }
    };

    let mut entry = SignatureEntry::new(id, pattern, severity, category);
    entry.enabled = enabled;
    db.entries[slot] = entry;
    if slot == db.count {
        db.count += 1;
    }
    ACTIVE_SIG_COUNT.fetch_add(1, Ordering::Relaxed);

    // Ajuster le compteur NEXT_SIG_ID si nécessaire
    loop {
        let current = NEXT_SIG_ID.load(Ordering::Acquire);
        if id >= current {
            if NEXT_SIG_ID
                .compare_exchange(current, id + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        } else {
            break;
        }
    }

    id
}

/// Recherche une signature par pattern exact.
///
/// # Retour
/// - L'ID de la signature si trouvée, 0 sinon.
pub fn find_by_pattern(pattern: &[u8]) -> u32 {
    if pattern.is_empty() {
        return 0;
    }

    let db = SIG_DB.lock();
    let plen = pattern.len();

    for i in 0..db.count {
        let entry = &db.entries[i];
        if !entry.is_valid() {
            continue;
        }
        if entry.pattern_len as usize != plen {
            continue;
        }
        let mut match_found = true;
        for j in 0..plen {
            if entry.pattern[j] != pattern[j] {
                match_found = false;
                break;
            }
        }
        if match_found {
            return entry.id;
        }
    }

    0
}

/// Recherche une signature par son ID.
///
/// # Retour
/// - Some(SignatureEntry) si trouvée, None sinon.
pub fn get_by_id(id: u32) -> Option<SignatureEntry> {
    if id == 0 {
        return None;
    }

    let db = SIG_DB.lock();
    for i in 0..db.count {
        if db.entries[i].id == id {
            return Some(db.entries[i]);
        }
    }
    None
}

/// Active une signature par son ID.
///
/// # Retour
/// - true si la signature a été activée, false si non trouvée.
pub fn enable(id: u32) -> bool {
    let mut db = SIG_DB.lock();
    for i in 0..db.count {
        if db.entries[i].id == id {
            if !db.entries[i].enabled {
                db.entries[i].enabled = true;
                ACTIVE_SIG_COUNT.fetch_add(1, Ordering::Relaxed);
            }
            return true;
        }
    }
    false
}

/// Désactive une signature par son ID.
///
/// # Retour
/// - true si la signature a été désactivée, false si non trouvée.
pub fn disable(id: u32) -> bool {
    let mut db = SIG_DB.lock();
    for i in 0..db.count {
        if db.entries[i].id == id {
            if db.entries[i].enabled {
                db.entries[i].enabled = false;
                ACTIVE_SIG_COUNT.fetch_sub(1, Ordering::Relaxed);
            }
            return true;
        }
    }
    false
}

/// Supprime une signature par son ID (la marque comme vide).
///
/// # Retour
/// - true si la signature a été supprimée, false si non trouvée.
pub fn remove_signature(id: u32) -> bool {
    let mut db = SIG_DB.lock();
    for i in 0..db.count {
        if db.entries[i].id == id {
            if db.entries[i].enabled {
                ACTIVE_SIG_COUNT.fetch_sub(1, Ordering::Relaxed);
            }
            db.entries[i] = SignatureEntry::empty();
            return true;
        }
    }
    false
}

/// Retourne les statistiques de la base de signatures.
pub fn get_stats() -> SignatureStats {
    let db = SIG_DB.lock();
    let mut stats = SignatureStats::empty();

    for i in 0..db.count {
        let entry = &db.entries[i];
        if !entry.is_valid() {
            continue;
        }
        stats.total += 1;
        if entry.enabled {
            stats.active += 1;
        }
        let sev_idx = entry.severity.as_u8() as usize;
        if sev_idx < 4 {
            stats.by_severity[sev_idx] += 1;
        }
        let cat_idx = entry.category.as_u8() as usize;
        if cat_idx < CATEGORY_COUNT {
            stats.by_category[cat_idx] += 1;
        }
    }

    stats
}

/// Retourne le nombre de signatures actives.
pub fn active_count() -> u32 {
    ACTIVE_SIG_COUNT.load(Ordering::Relaxed)
}

/// Parcourt toutes les signatures actives et applique la fonction de rappel.
///
/// Le callback reçoit (index, &SignatureEntry). Si le callback retourne false,
/// l'itération s'arrête.
pub fn iter_active<F>(mut f: F)
where
    F: FnMut(usize, &SignatureEntry) -> bool,
{
    let db = SIG_DB.lock();
    let mut visited = 0usize;
    for i in 0..db.count {
        let entry = &db.entries[i];
        if entry.is_valid() && entry.enabled {
            if !f(i, entry) {
                return;
            }
            visited += 1;
        }
    }
    let _ = visited;
}

/// Parcourt TOUTES les signatures (actives ou non) et applique la fonction de rappel.
pub fn iter_all<F>(mut f: F)
where
    F: FnMut(usize, &SignatureEntry) -> bool,
{
    let db = SIG_DB.lock();
    for i in 0..db.count {
        let entry = &db.entries[i];
        if entry.is_valid() {
            if !f(i, entry) {
                return;
            }
        }
    }
}

/// Capture un instantané de toutes les signatures (pour rollback).
///
/// # Arguments
/// - `buffer` : tableau de destination (doit faire au moins MAX_SIGNATURES).
/// - `max_out` : nombre maximum de signatures à copier.
///
/// # Retour
/// Le nombre de signatures copiées.
pub fn snapshot(buffer: &mut [SignatureEntry], max_out: usize) -> usize {
    let db = SIG_DB.lock();
    let limit = max_out.min(buffer.len()).min(db.count);
    let mut copied = 0usize;
    for i in 0..limit {
        if db.entries[i].is_valid() {
            buffer[copied] = db.entries[i];
            copied += 1;
            if copied >= buffer.len() {
                break;
            }
        }
    }
    copied
}

/// Restaure un instantané (remplace toute la base).
///
/// # Retour
/// Le nombre de signatures restaurées.
pub fn restore(entries: &[SignatureEntry]) -> usize {
    let mut db = SIG_DB.lock();

    // Vider la base
    for i in 0..MAX_SIGNATURES {
        db.entries[i] = SignatureEntry::empty();
    }
    db.count = 0;

    let mut restored = 0usize;
    for entry in entries.iter() {
        if !entry.is_valid() {
            continue;
        }
        if db.count >= MAX_SIGNATURES {
            break;
        }
        let idx = db.count;
        db.entries[idx] = *entry;
        db.count += 1;
        restored += 1;
    }

    ACTIVE_SIG_COUNT.store(restored as u32, Ordering::Release);
    restored
}

/// Initialise la base de signatures (appelé au démarrage).
pub fn database_init() {
    let mut db = SIG_DB.lock();
    for i in 0..MAX_SIGNATURES {
        db.entries[i] = SignatureEntry::empty();
    }
    db.count = 0;
    NEXT_SIG_ID.store(1, Ordering::Release);
    ACTIVE_SIG_COUNT.store(0, Ordering::Release);
}
