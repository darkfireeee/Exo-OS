// kernel/src/fs/exofs/core/object_id.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// ObjectId — constructeurs, compteur Class2, détection, sérialisation, validation
// ObjectIdPool (allocation batch), ObjectIdBuilder, utilitaires de format
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLES :
//   LOBJ-05 : comparaison ct_eq() UNIQUEMENT (timing-safe, pas d'opérateur ==).
//   LOBJ-06 : ObjectId Class1 = Blake3(blob_id || owner_cap) — calculé UNE SEULE FOIS.
//   LOBJ-07 : ObjectId Class2 — stable à vie, JAMAIS modifié après émission.
//   CLASS-02 : ObjectId Class2 = [counter_u64_le | zéros | 0x02].
//   ONDISK-01 : Format on-disk little-endian validé avant utilisation.
//
// FORMAT ON-DISK ObjectId :
//   Class1 : bytes[0..32] = Blake3(blob_id[0..32] || owner_cap[0..32])
//            → byte[31] ≠ 0x02 (vrai dans l'immense majorité des cas hachés)
//
//   Class2 : bytes[0..8]  = counter u64 LE
//            bytes[8..31] = 0x00 × 23
//            bytes[31]    = 0x02  ← marqueur de classe
//
//   INVALID : bytes[0..32] = 0xFF × 32 (sentinel ObjectId::INVALID dans types.rs)
//   ZERO    : bytes[0..32] = 0x00 × 32 (jamais alloué, erreur on-disk)

use crate::fs::exofs::core::error::ExofsError;
use crate::fs::exofs::core::object_class::ObjectClass;
use crate::fs::exofs::core::types::{BlobId, ObjectId};
use core::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Marqueur de classe dans byte[31] d'un ObjectId Class2.
pub const CLASS2_MARKER: u8 = 0x02;
/// Valeur minimale valide du compteur Class2 (0 = invalide).
pub const CLASS2_COUNTER_MIN: u64 = 1;
/// Index du byte portant le marqueur de classe dans un ObjectId.
pub const CLASS_MARKER_BYTE_IDX: usize = 31;
/// Taille d'un ObjectId en représentation hexadécimale ASCII.
pub const OBJECT_ID_HEX_LEN: usize = 64;
/// Longueur de l'aperçu court (16 hex chars = 8 bytes) pour les logs.
pub const OBJECT_ID_SHORT_HEX_LEN: usize = 16;

// ─────────────────────────────────────────────────────────────────────────────
// Compteur monotone global pour les ObjectId Class2
// ─────────────────────────────────────────────────────────────────────────────

/// Compteur monotone global pour les ObjectId de Classe 2.
///
/// Démarre à 1 (0 = invalide selon CLASS2_COUNTER_MIN).
/// Persisté sur disque à chaque commit d'epoch pour le recovery.
static CLASS2_COUNTER: AtomicU64 = AtomicU64::new(1);

// ─────────────────────────────────────────────────────────────────────────────
// Constructeurs principaux
// ─────────────────────────────────────────────────────────────────────────────

/// Crée un ObjectId de Classe 1 : Blake3(blob_id || owner_cap_bytes).
///
/// L'ObjectId est calculé UNE SEULE FOIS à la création (règle LOBJ-06).
/// Il est immuable pour toute la durée de vie de l'objet.
///
/// # Paramètres
/// - `blob_id`         : hash du contenu brut du blob (avant compression).
/// - `owner_cap_bytes` : 32 octets du CapToken propriétaire.
///
/// # Retour
/// ObjectId Class1 dérivé de manière déterministe du couple (blob, cap).
pub fn new_class1(blob_id: &BlobId, owner_cap_bytes: &[u8; 32]) -> ObjectId {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(&blob_id.0);
    input[32..].copy_from_slice(owner_cap_bytes);
    let hash = crate::fs::exofs::core::blob_id::blake3_hash(&input);
    ObjectId(hash)
}

/// Crée un ObjectId de Classe 2 avec le prochain compteur disponible.
///
/// Format — bytes :
///   [0..8]  = counter u64 LE
///   [8..31] = 0x00 × 23
///   [31]    = CLASS2_MARKER (0x02)
///
/// L'ObjectId est STABLE À VIE (règle LOBJ-07) : aucune mutation future.
pub fn new_class2() -> ObjectId {
    let counter = CLASS2_COUNTER.fetch_add(1, Ordering::Relaxed);
    build_class2_id(counter)
}

/// Crée un ObjectId de Classe 2 avec un compteur explicite (recovery / replay).
///
/// Utilisé lors du replay d'époch pour reconstruire les ObjectIds historiques.
/// NE modifie PAS le compteur global CLASS2_COUNTER.
#[inline]
pub fn new_class2_with_counter(counter: u64) -> ObjectId {
    build_class2_id(counter)
}

/// Construit l'ObjectId Class2 depuis un compteur (helper interne).
#[inline]
fn build_class2_id(counter: u64) -> ObjectId {
    let mut bytes = [0u8; 32];
    let c = counter.to_le_bytes();
    bytes[0] = c[0];
    bytes[1] = c[1];
    bytes[2] = c[2];
    bytes[3] = c[3];
    bytes[4] = c[4];
    bytes[5] = c[5];
    bytes[6] = c[6];
    bytes[7] = c[7];
    bytes[CLASS_MARKER_BYTE_IDX] = CLASS2_MARKER;
    ObjectId(bytes)
}

// ─────────────────────────────────────────────────────────────────────────────
// Gestion du compteur Class2
// ─────────────────────────────────────────────────────────────────────────────

/// Restaure le compteur Class2 depuis la valeur persistée sur disque au boot.
///
/// Garantit que les nouveaux ObjectIds ne collisionneront pas avec ceux
/// existant sur disque. Cette fonction est idempotente et thread-safe.
///
/// # Règle
/// Si `last_counter` ≥ compteur actuel, le compteur est mis à jour à
/// `last_counter + 1`.
pub fn restore_class2_counter(last_counter: u64) {
    let next = last_counter.saturating_add(1).max(CLASS2_COUNTER_MIN);
    loop {
        let cur = CLASS2_COUNTER.load(Ordering::Relaxed);
        if next <= cur {
            break;
        }
        if CLASS2_COUNTER
            .compare_exchange(cur, next, Ordering::SeqCst, Ordering::Relaxed)
            .is_ok()
        {
            break;
        }
    }
}

/// Retourne la valeur courante du compteur Class2 pour persistance.
///
/// Appelé lors de chaque epoch commit pour persister le watermark.
#[inline]
pub fn current_class2_counter() -> u64 {
    CLASS2_COUNTER.load(Ordering::Relaxed)
}

/// Détecte un éventuel gap dans les compteurs Class2 (recovery).
///
/// Un gap est détecté si `last_persisted` < `actual_max` − 1.
/// Cela peut indiquer une perte de données ou un crash mid-commit.
///
/// Retourne (expected_next, found_next, gap_detected).
pub fn detect_class2_counter_gap(last_persisted: u64, actual_max_on_disk: u64) -> (u64, u64, bool) {
    let expected = last_persisted.saturating_add(1);
    let gap = actual_max_on_disk > last_persisted.saturating_add(1);
    (expected, actual_max_on_disk, gap)
}

// ─────────────────────────────────────────────────────────────────────────────
// Détection de classe
// ─────────────────────────────────────────────────────────────────────────────

/// Vrai si l'ObjectId semble être de Classe 2 (heuristique byte[31]).
///
/// ATTENTION : non garanti sur des données corrompues.
/// Utiliser `detect_class()` pour la logique métier.
#[inline]
pub fn is_class2_heuristic(oid: &ObjectId) -> bool {
    oid.0[CLASS_MARKER_BYTE_IDX] == CLASS2_MARKER
}

/// Vrai si l'ObjectId semble être de Classe 1 (heuristique).
#[inline]
pub fn is_class1_heuristic(oid: &ObjectId) -> bool {
    !is_class2_heuristic(oid)
}

/// Retourne la classe probable depuis le marqueur on-disk de l'ObjectId.
///
/// Utilisé lors du chargement d'ObjectHeaders pour vérifier la cohérence.
/// Résultat à confirmer via `validate_class_invariants()` avec le Kind.
#[inline]
pub fn detect_class(oid: &ObjectId) -> ObjectClass {
    if is_class2_heuristic(oid) {
        ObjectClass::Class2
    } else {
        ObjectClass::Class1
    }
}

/// Extrait le compteur u64 d'un ObjectId Class2.
///
/// Retourne None si l'ObjectId n'est pas Class2 (heuristique).
/// Utilisé au recovery pour restaurer CLASS2_COUNTER.
#[inline]
pub fn extract_class2_counter(oid: &ObjectId) -> Option<u64> {
    if !is_class2_heuristic(oid) {
        return None;
    }
    Some(u64::from_le_bytes([
        oid.0[0], oid.0[1], oid.0[2], oid.0[3], oid.0[4], oid.0[5], oid.0[6], oid.0[7],
    ]))
}

// ─────────────────────────────────────────────────────────────────────────────
// Sérialisation / formatage
// ─────────────────────────────────────────────────────────────────────────────

/// Formate un ObjectId en représentation hex ASCII de 64 caractères.
///
/// Retourne un tableau de 64 octets ASCII. Pas d'allocation (no_std safe).
/// Utilisé pour les messages de log kernel.
pub fn object_id_to_hex(oid: &ObjectId) -> [u8; OBJECT_ID_HEX_LEN] {
    let mut out = [0u8; OBJECT_ID_HEX_LEN];
    const D: &[u8] = b"0123456789abcdef";
    for (i, &b) in oid.0.iter().enumerate() {
        out[i * 2] = D[(b >> 4) as usize];
        out[i * 2 + 1] = D[(b & 0xf) as usize];
    }
    out
}

/// Aperçu court d'un ObjectId (16 hex chars = 8 premiers octets).
///
/// Format suffisant pour les logs kernel ou la déduplication rapide.
pub fn object_id_short_hex(oid: &ObjectId) -> [u8; OBJECT_ID_SHORT_HEX_LEN] {
    let mut out = [0u8; OBJECT_ID_SHORT_HEX_LEN];
    const D: &[u8] = b"0123456789abcdef";
    for (i, &b) in oid.0[..8].iter().enumerate() {
        out[i * 2] = D[(b >> 4) as usize];
        out[i * 2 + 1] = D[(b & 0xf) as usize];
    }
    out
}

/// Parse un ObjectId depuis une chaîne hex de 64 caractères.
///
/// Retourne `ExofsError::InvalidArgument` si la chaîne n'est pas valide.
pub fn object_id_from_hex(hex: &[u8; OBJECT_ID_HEX_LEN]) -> Result<ObjectId, ExofsError> {
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        let hi = hex_nibble(hex[i * 2])?;
        let lo = hex_nibble(hex[i * 2 + 1])?;
        bytes[i] = (hi << 4) | lo;
    }
    Ok(ObjectId(bytes))
}

/// Convertit un nibble ASCII en valeur hexadécimale.
#[inline]
fn hex_nibble(c: u8) -> Result<u8, ExofsError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(ExofsError::InvalidArgument),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Validation on-disk
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un ObjectId on-disk n'est pas manifestement corrompu.
///
/// Critères de validité :
///   - Non-INVALID (0xFF × 32).
///   - Non-ZERO (0x00 × 32 = jamais alloué).
///   - Si Class2 : compteur > 0.
pub fn validate_object_id_ondisk(oid: &ObjectId) -> Result<(), ExofsError> {
    if oid.is_invalid() {
        return Err(ExofsError::CorruptedStructure);
    }
    if oid.0 == [0u8; 32] {
        return Err(ExofsError::CorruptedStructure);
    }
    if is_class2_heuristic(oid) {
        if extract_class2_counter(oid).unwrap_or(0) == 0 {
            return Err(ExofsError::CorruptedStructure);
        }
    }
    Ok(())
}

/// Alias rétro-compatible (ancienne orthographe sans double-d).
#[inline]
pub fn validate_object_id_ondi(oid: &ObjectId) -> Result<(), ExofsError> {
    validate_object_id_ondisk(oid)
}

/// Valide un lot d'ObjectIds en stoppant au premier invalide.
///
/// Retourne l'index du premier ObjectId invalide, ou None si tous sont valides.
pub fn validate_batch(oids: &[ObjectId]) -> Option<usize> {
    for (i, oid) in oids.iter().enumerate() {
        if validate_object_id_ondisk(oid).is_err() {
            return Some(i);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectIdBuilder — construction fluente d'un ObjectId
// ─────────────────────────────────────────────────────────────────────────────

/// Builder pour la création d'ObjectIds avec validation explicite des paramètres.
///
/// Garantit que les invariants LOBJ-05/06/07 sont respectés avant émission.
#[derive(Default)]
pub struct ObjectIdBuilder {
    blob_id_set: bool,
    blob_id: [u8; 32],
    owner_cap_set: bool,
    owner_cap: [u8; 32],
    target_class2: bool,
    force_counter: Option<u64>,
}

impl ObjectIdBuilder {
    /// Nouveau builder (commence vide).
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure le BlobId pour un ObjectId Class1.
    pub fn blob_id(mut self, bid: &BlobId) -> Self {
        self.blob_id.copy_from_slice(&bid.0);
        self.blob_id_set = true;
        self
    }

    /// Configure le CapToken pour un ObjectId Class1.
    pub fn owner_cap(mut self, cap: &[u8; 32]) -> Self {
        self.owner_cap.copy_from_slice(cap);
        self.owner_cap_set = true;
        self
    }

    /// Sélectionne la classe 2 (compteur monotone).
    pub fn class2(mut self) -> Self {
        self.target_class2 = true;
        self
    }

    /// Fixe un compteur Class2 spécifique (recovery uniquement).
    pub fn with_counter(mut self, c: u64) -> Self {
        self.force_counter = Some(c);
        self.target_class2 = true;
        self
    }

    /// Construit l'ObjectId selon les paramètres fournis.
    ///
    /// Retourne une erreur si les paramètres sont incohérents.
    pub fn build(self) -> Result<ObjectId, ExofsError> {
        if self.target_class2 {
            let counter = match self.force_counter {
                Some(c) => {
                    if c == 0 {
                        return Err(ExofsError::InvalidArgument);
                    }
                    c
                }
                None => CLASS2_COUNTER.fetch_add(1, Ordering::Relaxed),
            };
            return Ok(build_class2_id(counter));
        }
        // Class1 : blob_id et owner_cap requis.
        if !self.blob_id_set {
            return Err(ExofsError::InvalidArgument);
        }
        if !self.owner_cap_set {
            return Err(ExofsError::InvalidArgument);
        }
        let mut input = [0u8; 64];
        input[..32].copy_from_slice(&self.blob_id);
        input[32..].copy_from_slice(&self.owner_cap);
        let hash = crate::fs::exofs::core::blob_id::blake3_hash(&input);
        Ok(ObjectId(hash))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectIdPool — allocation batch sans alloc (capacité fixe)
// ─────────────────────────────────────────────────────────────────────────────

/// Taille maximale d'un ObjectIdPool statique (pas d'allocation heap).
pub const OBJECT_ID_POOL_CAP: usize = 64;

/// Pool d'ObjectIds Class2 pré-alloués, pour les commits d'epoch en lot.
///
/// Évite les appels atomiques répétés dans les chemins chauds.
/// Le pool est rempli en une seule opération fetch_add, réduisant
/// la pression sur le cache partagé.
pub struct ObjectIdPool {
    ids: [ObjectId; OBJECT_ID_POOL_CAP],
    count: usize,
    next: usize,
}

impl ObjectIdPool {
    /// Crée un pool vide.
    pub const fn empty() -> Self {
        Self {
            ids: [ObjectId::INVALID; OBJECT_ID_POOL_CAP],
            count: 0,
            next: 0,
        }
    }

    /// Remplit le pool avec `n` nouveaux ObjectIds Class2.
    ///
    /// Une seule opération atomique fetch_add pour `n` IDs.
    /// Retourne une erreur si `n` > OBJECT_ID_POOL_CAP.
    pub fn fill(&mut self, n: usize) -> Result<(), ExofsError> {
        if n == 0 || n > OBJECT_ID_POOL_CAP {
            return Err(ExofsError::InvalidArgument);
        }
        // Allocation batch : une seule opération atomique.
        let base = CLASS2_COUNTER.fetch_add(n as u64, Ordering::Relaxed);
        for i in 0..n {
            self.ids[i] = build_class2_id(base + i as u64);
        }
        self.count = n;
        self.next = 0;
        Ok(())
    }

    /// Prend le prochain ObjectId disponible dans le pool.
    ///
    /// Retourne None si le pool est épuisé.
    pub fn take(&mut self) -> Option<ObjectId> {
        if self.next >= self.count {
            return None;
        }
        let id = self.ids[self.next];
        self.next += 1;
        Some(id)
    }

    /// Nombre d'ObjectIds restants dans le pool.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.count.saturating_sub(self.next)
    }

    /// Vide le pool (tous les IDs non-pris sont libérés / perdus).
    ///
    /// À appeler lors d'un rollback d'epoch pour libérer les IDs réservés.
    /// Note : les compteurs atomiques ne sont pas restaurés (monotone strict).
    pub fn flush(&mut self) {
        self.count = 0;
        self.next = 0;
    }

    /// Vrai si le pool est épuisé.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de comparaison
// ─────────────────────────────────────────────────────────────────────────────

/// Comparaison timing-safe de deux ObjectIds (règle LOBJ-05).
///
/// NE PAS utiliser == pour comparer des ObjectIds. Retourne toujours en
/// temps constant indépendamment du contenu pour éviter les fuites de timing.
///
/// Utiliser crate::fs::exofs::core::types::ObjectId::ct_eq() en priorité.
/// Cette fonction est un alias de commodité.
#[inline]
pub fn object_id_ct_eq(a: &ObjectId, b: &ObjectId) -> bool {
    a.ct_eq(b)
}

/// Trie un slice d'ObjectIds in-place par ordre lexicographique.
///
/// Utilisé pour construire des structures de déduplication déterministes.
pub fn sort_object_ids(ids: &mut [ObjectId]) {
    // Insertion sort — adapté aux petits slices (< 100 éléments) en Ring 0.
    let n = ids.len();
    for i in 1..n {
        let key = ids[i];
        let mut j = i;
        while j > 0 && ids[j - 1].0 > key.0 {
            ids[j] = ids[j - 1];
            j -= 1;
        }
        ids[j] = key;
    }
}

/// Cherche un ObjectId dans un slice trié (recherche binaire).
///
/// Retourne Some(index) si trouvé, None sinon.
pub fn search_sorted_object_ids(ids: &[ObjectId], target: &ObjectId) -> Option<usize> {
    let mut lo = 0usize;
    let mut hi = ids.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if ids[mid].0 == target.0 {
            return Some(mid);
        } else if ids[mid].0 < target.0 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Utilitaires de namespace
// ─────────────────────────────────────────────────────────────────────────────

/// Construit un ObjectId Class1 de namespace (sans BlobId, avec namespace tag).
///
/// Utilisé pour les PathIndex racine d'un volume.
/// Format : Blake3(namespace_tag[0..16] || pad[0..16] || owner_cap[0..32])
pub fn new_namespace_id(namespace_tag: &[u8; 16], owner_cap: &[u8; 32]) -> ObjectId {
    let mut blob_input = [0u8; 32];
    blob_input[..16].copy_from_slice(namespace_tag);
    // bytes[16..32] = 0 (padding de namespace)
    let pseudo_blob = BlobId(crate::fs::exofs::core::blob_id::blake3_hash(&blob_input));
    new_class1(&pseudo_blob, owner_cap)
}

/// Extrait le préfixe de namespace (16 premiers octets) d'un ObjectId.
///
/// Uniquement significatif pour les ObjectIds générés par `new_namespace_id`.
pub fn extract_namespace_prefix(oid: &ObjectId) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&oid.0[..8]);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Réinitialise le compteur Class2 pour les tests.
///
/// # Safety
/// À appeler UNIQUEMENT avant tout accès concurrent.
/// Aucune synchronisation supplémentaire après l'appel.
#[cfg(test)]
pub unsafe fn reset_class2_counter_for_test() {
    CLASS2_COUNTER.store(1, Ordering::SeqCst);
}
