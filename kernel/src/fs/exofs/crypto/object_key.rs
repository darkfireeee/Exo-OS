//! Clé d'objet ExoFS — chiffrement individuel de chaque blob.
//!
//! Chaque blob possède sa propre clé dérivée depuis la clé de volume.
//! Cette architecture permet de révoquer un objet (crypto-shredding) sans
//! avoir à déchiffrer / rechiffrer l'ensemble du volume.
//!
//! OOM-02 / ARITH-02 / RECUR-01 respectés.

use super::entropy::ENTROPY_POOL;
use super::key_derivation::KeyDerivation;
use super::volume_key::VolumeKey;
use crate::fs::exofs::core::{ExofsError, ExofsResult};
use alloc::vec::Vec;

// ─────────────────────────────────────────────────────────────────────────────
// Constantes
// ─────────────────────────────────────────────────────────────────────────────

/// Taille d'une clé d'objet.
pub const OBJECT_KEY_LEN: usize = 32;
/// Durée de vie maximale suggérée (nombre d'utilisations).
pub const OBJECT_KEY_MAX_USES: u64 = 1_000_000;

// ─────────────────────────────────────────────────────────────────────────────
// Identifiant de blob
// ─────────────────────────────────────────────────────────────────────────────

/// Identifiant unique d'un blob ExoFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobKeyId(pub u64);

impl BlobKeyId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

impl core::fmt::Display for BlobKeyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Blob({:#018x})", self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ObjectKey
// ─────────────────────────────────────────────────────────────────────────────

/// Clé d'objet (zeroize on drop).
pub struct ObjectKey {
    /// Matériel de clé (256 bits).
    key: [u8; OBJECT_KEY_LEN],
    /// Identifiant du blob associé.
    blob_id: BlobKeyId,
    /// Compteur d'utilisations pour la politique de rotation.
    use_count: u64,
    /// Indique si la clé a été révoquée.
    revoked: bool,
}

impl core::fmt::Debug for ObjectKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "ObjectKey {{ blob_id: {:?}, use_count: {}, revoked: {} }}",
            self.blob_id, self.use_count, self.revoked
        )
    }
}

impl Drop for ObjectKey {
    fn drop(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
        self.use_count = 0;
    }
}

/// Tweak pour séparer les usages d'une même clé d'objet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKeyTweak {
    /// Chiffrement des données.
    DataPlane,
    /// Chiffrement des métadonnées.
    Metadata,
    /// Chiffrement de l'en-tête.
    Header,
    /// MAC d'intégrité.
    Integrity,
}

impl ObjectKeyTweak {
    fn discriminant(self) -> u8 {
        match self {
            Self::DataPlane => 0,
            Self::Metadata => 1,
            Self::Header => 2,
            Self::Integrity => 3,
        }
    }
}

impl ObjectKey {
    // ── Constructeurs ─────────────────────────────────────────────────────────

    /// Dérive une clé d'objet depuis la clé de volume.
    pub fn derive(vk: &VolumeKey, blob_id: BlobKeyId) -> ExofsResult<Self> {
        let raw = vk.derive_object_key(blob_id.0)?;
        Ok(Self {
            key: raw,
            blob_id,
            use_count: 0,
            revoked: false,
        })
    }

    /// Génère une clé d'objet aléatoire (non liée à une VolumeKey — usage exceptionnel).
    pub fn generate_ephemeral(blob_id: BlobKeyId) -> ExofsResult<Self> {
        let raw_vec = ENTROPY_POOL.random_bytes(OBJECT_KEY_LEN)?;
        let mut key = [0u8; OBJECT_KEY_LEN];
        key.copy_from_slice(&raw_vec);
        Ok(Self {
            key,
            blob_id,
            use_count: 0,
            revoked: false,
        })
    }

    /// Construit depuis des bytes bruts (import sécurisé).
    pub fn from_bytes(bytes: [u8; OBJECT_KEY_LEN], blob_id: BlobKeyId) -> Self {
        Self {
            key: bytes,
            blob_id,
            use_count: 0,
            revoked: false,
        }
    }

    // ── Accesseurs ────────────────────────────────────────────────────────────

    pub fn blob_id(&self) -> BlobKeyId {
        self.blob_id
    }
    pub fn use_count(&self) -> u64 {
        self.use_count
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked
    }

    /// Retourne les bytes bruts (durée de vie très courte — ne pas stocker).
    pub fn raw_bytes(&self) -> &[u8; OBJECT_KEY_LEN] {
        &self.key
    }

    // ── Utilisation ───────────────────────────────────────────────────────────

    /// Incrémente le compteur d'utilisation.
    ///
    /// ARITH-02 : `saturating_add`.
    pub fn record_use(&mut self) -> ExofsResult<()> {
        if self.revoked {
            return Err(ExofsError::InternalError);
        }
        self.use_count = self.use_count.saturating_add(1);
        Ok(())
    }

    /// Retourne `true` si la clé a dépassé le nombre recommandé d'utilisations.
    pub fn requires_rotation(&self) -> bool {
        self.use_count >= OBJECT_KEY_MAX_USES
    }

    /// Révoque la clé (zeroize immédiat, use_count remis à zéro).
    pub fn revoke(&mut self) {
        self.key.iter_mut().for_each(|b| *b = 0);
        self.use_count = 0;
        self.revoked = true;
    }

    // ── Dérivation de sous-clés ───────────────────────────────────────────────

    /// Dérive une sous-clé pour un usage spécifique (tweak).
    ///
    /// Permet d'utiliser une seule clé de base pour chiffrement + intégrité
    /// avec séparation cryptographique garantie.
    pub fn derive_subkey(&self, tweak: ObjectKeyTweak) -> ExofsResult<[u8; 32]> {
        if self.revoked {
            return Err(ExofsError::InternalError);
        }
        let mut ctx: Vec<u8> = Vec::new();
        ctx.try_reserve(12).map_err(|_| ExofsError::NoMemory)?;
        ctx.extend_from_slice(b"exofs-sub-");
        ctx.push(tweak.discriminant());
        ctx.push(0u8); // padding
        let dk = KeyDerivation::derive_key(&self.key, b"", &ctx)?;
        Ok(*dk.as_bytes())
    }

    /// Dérive la sous-clé de chiffrement données.
    pub fn data_key(&self) -> ExofsResult<[u8; 32]> {
        self.derive_subkey(ObjectKeyTweak::DataPlane)
    }

    /// Dérive la sous-clé d'intégrité.
    pub fn integrity_key(&self) -> ExofsResult<[u8; 32]> {
        self.derive_subkey(ObjectKeyTweak::Integrity)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pool de clés d'objets
// ─────────────────────────────────────────────────────────────────────────────

/// Gestionnaire de clés d'objets pour un volume.
///
/// Maintient un ensemble de clés actives et révoquées.
pub struct ObjectKeyPool {
    /// Clés actives.
    active: Vec<ObjectKey>,
    /// Identifiants révoqués.
    revoked: Vec<BlobKeyId>,
}

impl ObjectKeyPool {
    /// Crée un pool vide.
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            revoked: Vec::new(),
        }
    }

    /// Retourne `true` si un blob_id est révoqué.
    pub fn is_revoked(&self, blob_id: BlobKeyId) -> bool {
        self.revoked.contains(&blob_id)
    }

    /// Ajoute une clé active.
    ///
    /// OOM-02.
    pub fn insert(&mut self, key: ObjectKey) -> ExofsResult<()> {
        self.active
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;
        self.active.push(key);
        Ok(())
    }

    /// Révoque un blob_id (ajoute à la liste de révocation).
    ///
    /// OOM-02.
    pub fn revoke(&mut self, blob_id: BlobKeyId) -> ExofsResult<()> {
        // Zeroize et retire la clé active.
        if let Some(pos) = self.active.iter().position(|k| k.blob_id() == blob_id) {
            self.active[pos].revoke();
            self.active.remove(pos);
        }
        if !self.revoked.contains(&blob_id) {
            self.revoked
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            self.revoked.push(blob_id);
        }
        Ok(())
    }

    /// Nombre de clés actives.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
    /// Nombre de révocations enregistrées.
    pub fn revoked_count(&self) -> usize {
        self.revoked.len()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::volume_key::{VolumeId, VolumeKey};
    use super::*;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn vk() -> VolumeKey {
        ok(VolumeKey::generate(VolumeId(1)))
    }

    #[test]
    fn test_derive_ok() {
        let vk = vk();
        let object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        assert_eq!(object_key.raw_bytes().len(), 32);
    }

    #[test]
    fn test_different_blobs_different_keys() {
        let vk = vk();
        let object_key_1 = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        let object_key_2 = ok(ObjectKey::derive(&vk, BlobKeyId(2)));
        assert_ne!(object_key_1.raw_bytes(), object_key_2.raw_bytes());
    }

    #[test]
    fn test_ephemeral_generation() {
        let object_key = ok(ObjectKey::generate_ephemeral(BlobKeyId(99)));
        assert!(!object_key.is_revoked());
    }

    #[test]
    fn test_record_use_increments() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        ok(object_key.record_use());
        ok(object_key.record_use());
        assert_eq!(object_key.use_count(), 2);
    }

    #[test]
    fn test_revoke_zeroes_key() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        object_key.revoke();
        assert!(object_key.is_revoked());
        assert_eq!(*object_key.raw_bytes(), [0u8; 32]);
    }

    #[test]
    fn test_record_use_after_revoke_fails() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        object_key.revoke();
        assert!(object_key.record_use().is_err());
    }

    #[test]
    fn test_subkey_data_ok() {
        let vk = vk();
        let object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        let data_key = ok(object_key.data_key());
        assert_eq!(data_key.len(), 32);
    }

    #[test]
    fn test_subkey_integrity_different() {
        let vk = vk();
        let object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        let data_key = ok(object_key.data_key());
        let integrity_key = ok(object_key.integrity_key());
        assert_ne!(data_key, integrity_key);
    }

    #[test]
    fn test_pool_insert_revoke() {
        let vk = vk();
        let mut pool = ObjectKeyPool::new();
        let object_key = ok(ObjectKey::derive(&vk, BlobKeyId(5)));
        ok(pool.insert(object_key));
        assert_eq!(pool.active_count(), 1);
        ok(pool.revoke(BlobKeyId(5)));
        assert_eq!(pool.active_count(), 0);
        assert_eq!(pool.revoked_count(), 1);
        assert!(pool.is_revoked(BlobKeyId(5)));
    }

    #[test]
    fn test_pool_double_revoke_ok() {
        let mut pool = ObjectKeyPool::new();
        ok(pool.revoke(BlobKeyId(99)));
        ok(pool.revoke(BlobKeyId(99))); // idempotent
        assert_eq!(pool.revoked_count(), 1);
    }

    #[test]
    fn test_blob_id_display() {
        let id = BlobKeyId(0x1234);
        assert!(format!("{id}").contains("Blob"));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Politique de rotation des clés d'objets
// ─────────────────────────────────────────────────────────────────────────────

/// Politique de rotation automatique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationPolicy {
    /// Jamais (clé permanente).
    Never,
    /// Après N utilisations.
    AfterNUses(u64),
    /// Toujours (une seule utilisation).
    Once,
}

impl RotationPolicy {
    /// Détermine si une clé doit être renouvelée selon la politique.
    pub fn should_rotate(self, key: &ObjectKey) -> bool {
        match self {
            Self::Never => false,
            Self::Once => key.use_count() >= 1,
            Self::AfterNUses(n) => key.use_count() >= n,
        }
    }
}

/// Gestionnaire de rotation automatique pour un ensemble de clés d'objets.
pub struct ObjectKeyRotator {
    policy: RotationPolicy,
    rotated: Vec<BlobKeyId>,
    pending: Vec<BlobKeyId>,
}

impl ObjectKeyRotator {
    /// Crée un rotateur avec la politique spécifiée.
    pub fn new(policy: RotationPolicy) -> Self {
        Self {
            policy,
            rotated: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Évalue si la clé doit être tournée, et l'enregistre si nécessaire.
    ///
    /// OOM-02.
    pub fn evaluate(&mut self, key: &ObjectKey) -> ExofsResult<bool> {
        if self.policy.should_rotate(key) {
            if !self.pending.contains(&key.blob_id()) {
                self.pending
                    .try_reserve(1)
                    .map_err(|_| ExofsError::NoMemory)?;
                self.pending.push(key.blob_id());
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Marque un blob_id comme ayant été tourné.
    pub fn mark_rotated(&mut self, blob_id: BlobKeyId) -> ExofsResult<()> {
        self.pending.retain(|&x| x != blob_id);
        if !self.rotated.contains(&blob_id) {
            self.rotated
                .try_reserve(1)
                .map_err(|_| ExofsError::NoMemory)?;
            self.rotated.push(blob_id);
        }
        Ok(())
    }

    /// Liste des blob_ids en attente de rotation.
    pub fn pending_rotation(&self) -> &[BlobKeyId] {
        &self.pending
    }
    /// Liste des blob_ids déjà tournés.
    pub fn rotated_ids(&self) -> &[BlobKeyId] {
        &self.rotated
    }
    /// Nombre de rotations en attente.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::super::volume_key::{VolumeId, VolumeKey};
    use super::*;

    fn ok<T, E: core::fmt::Debug>(res: Result<T, E>) -> T {
        match res {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err:?}"),
        }
    }

    fn vk() -> VolumeKey {
        ok(VolumeKey::generate(VolumeId(2)))
    }

    #[test]
    fn test_policy_never() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        ok(object_key.record_use());
        assert!(!RotationPolicy::Never.should_rotate(&object_key));
    }

    #[test]
    fn test_policy_once() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        assert!(!RotationPolicy::Once.should_rotate(&object_key));
        ok(object_key.record_use());
        assert!(RotationPolicy::Once.should_rotate(&object_key));
    }

    #[test]
    fn test_policy_after_n() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(1)));
        for _ in 0..4 {
            ok(object_key.record_use());
        }
        assert!(!RotationPolicy::AfterNUses(5).should_rotate(&object_key));
        ok(object_key.record_use());
        assert!(RotationPolicy::AfterNUses(5).should_rotate(&object_key));
    }

    #[test]
    fn test_rotator_evaluate_marks_pending() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(10)));
        ok(object_key.record_use());
        let mut rot = ObjectKeyRotator::new(RotationPolicy::Once);
        let needs = ok(rot.evaluate(&object_key));
        assert!(needs);
        assert_eq!(rot.pending_count(), 1);
    }

    #[test]
    fn test_rotator_mark_rotated() {
        let vk = vk();
        let mut object_key = ok(ObjectKey::derive(&vk, BlobKeyId(10)));
        ok(object_key.record_use());
        let mut rot = ObjectKeyRotator::new(RotationPolicy::Once);
        ok(rot.evaluate(&object_key));
        ok(rot.mark_rotated(BlobKeyId(10)));
        assert_eq!(rot.pending_count(), 0);
        assert_eq!(rot.rotated_ids().len(), 1);
    }
}
