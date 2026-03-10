// kernel/src/fs/exofs/epoch/epoch_snapshot.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Snapshots basés sur les Epochs — points de restauration immuables
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un snapshot ExoFS est un EpochPin sur un epoch passé associé à un
// descripteur de métadonnées. La lecture d'un snapshot utilise l'EpochRecord
// de l'epoch snapshotté pour retrouver l'EpochRoot correspondant.
//
// RÈGLE EPOCH-07 : le snapshot est read-only — aucun commit possible dessus.
// RÈGLE DAG-01   : pas d'import storage/ — callbacks injectés.
// RÈGLE OOM-02   : try_reserve avant push.
// RÈGLE ARITH-02 : checked_add / saturating_* pour toute arithmétique.
// RÈGLE RECUR-01 : pas de récursion.
//
// Design :
//   - SnapshotRegistry est un registre global protégé par SpinLock.
//   - Capacité maximale : MAX_SNAPSHOTS = 64.
//   - Chaque snapshot maintient un EpochPin RAII.
//   - Les droits sont vérifiés via un bitmask u64 (rights_bits).

use alloc::vec::Vec;
use core::fmt;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, ObjectId,
};
use crate::fs::exofs::epoch::epoch_pin::{EpochPin, PinReason, oldest_pinned_epoch};
use crate::fs::exofs::epoch::epoch_stats::EPOCH_STATS;
use crate::scheduler::sync::spinlock::SpinLock;

// =============================================================================
// Droits de snapshot (bitmask interne, DAG-01 compliant)
// =============================================================================

/// Bit droit : création de snapshot autorisée.
pub const RIGHT_SNAPSHOT_CREATE: u64 = 1 << 0;
/// Bit droit : suppression de snapshot autorisée.
pub const RIGHT_SNAPSHOT_DELETE: u64 = 1 << 1;
/// Bit droit : listage des snapshots autorisé.
pub const RIGHT_SNAPSHOT_LIST:   u64 = 1 << 2;
/// Bit droit : lecture depuis un snapshot autorisée.
pub const RIGHT_SNAPSHOT_READ:   u64 = 1 << 3;

/// Vérifie si le bitmask de droits autorise la création de snapshot.
#[inline]
pub fn has_snapshot_create(rights_bits: u64) -> bool {
    rights_bits & RIGHT_SNAPSHOT_CREATE != 0
}

/// Vérifie si le bitmask de droits autorise la suppression de snapshot.
#[inline]
pub fn has_snapshot_delete(rights_bits: u64) -> bool {
    rights_bits & RIGHT_SNAPSHOT_DELETE != 0
}

/// Vérifie si le bitmask de droits autorise le listage des snapshots.
#[inline]
pub fn has_snapshot_list(rights_bits: u64) -> bool {
    rights_bits & RIGHT_SNAPSHOT_LIST != 0
}

// =============================================================================
// Nom de snapshot
// =============================================================================

/// Nom du snapshot — wraps [u8; 64] pour éviter heap allocation.
#[derive(Copy, Clone)]
pub struct SnapshotName([u8; 64]);

impl SnapshotName {
    /// Crée un nom vide (zéros).
    pub const fn empty() -> Self {
        Self([0u8; 64])
    }

    /// Crée un nom depuis une slice UTF-8/bytes. Tronque à 64 octets.
    pub fn from_bytes(src: &[u8]) -> Self {
        let mut buf = [0u8; 64];
        let len = src.len().min(64);
        buf[..len].copy_from_slice(&src[..len]);
        Self(buf)
    }

    /// Retourne le nom comme slice d'octets (jusqu'au premier nul ou 64).
    pub fn as_bytes(&self) -> &[u8] {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(64);
        &self.0[..end]
    }

    /// Longueur effective du nom (octets avant le premier nul).
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Retourne `true` si le nom est vide.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for SnapshotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Affiche la partie ASCII valide ou les octets bruts si non-ASCII.
        let bytes = self.as_bytes();
        if core::str::from_utf8(bytes).is_ok() {
            write!(f, "SnapshotName({:?})", core::str::from_utf8(bytes).unwrap_or("?"))
        } else {
            write!(f, "SnapshotName({:?})", bytes)
        }
    }
}

impl fmt::Display for SnapshotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.as_bytes();
        match core::str::from_utf8(bytes) {
            Ok(s)  => write!(f, "{}", s),
            Err(_) => write!(f, "<non-utf8 name>"),
        }
    }
}

// =============================================================================
// Descripteur de snapshot
// =============================================================================

/// Métadonnées in-memory d'un snapshot.
pub struct SnapshotDescriptor {
    /// SnapshotId unique (alloué par le registre).
    pub snapshot_id:    u64,
    /// Epoch épinglé par ce snapshot.
    pub epoch_id:       EpochId,
    /// Nom du snapshot (max 64 octets).
    pub name:           SnapshotName,
    /// Timestamp de création (TSC cycles).
    pub created_at:     u64,
    /// Timestamp d'expiration (0 = pas d'expiration).
    pub expires_at:     u64,
    /// ObjectId de l'objet racine snapshotté.
    pub root_object_id: ObjectId,
    /// Pin RAII — maintient l'epoch vivant tant que le snapshot existe.
    #[allow(dead_code)]
    pin:                EpochPin,
}

impl SnapshotDescriptor {
    /// Retourne l'epoch_id épinglé par ce snapshot.
    #[inline]
    pub fn pinned_epoch(&self) -> EpochId {
        self.epoch_id
    }

    /// Retourne `true` si le snapshot a expiré (expires_at != 0 et tsc_now >= expires_at).
    #[inline]
    pub fn is_expired(&self, tsc_now: u64) -> bool {
        self.expires_at != 0 && tsc_now >= self.expires_at
    }

    /// Retourne `true` si ce snapshot est permanent (pas d'expiration).
    #[inline]
    pub fn is_permanent(&self) -> bool {
        self.expires_at == 0
    }
}

impl fmt::Debug for SnapshotDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotDescriptor")
            .field("snapshot_id", &self.snapshot_id)
            .field("epoch_id", &self.epoch_id.0)
            .field("name", &self.name)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

// =============================================================================
// Registre de snapshots
// =============================================================================

/// Capacité maximale du registre de snapshots.
pub const MAX_SNAPSHOTS: usize = 64;

/// État interne du registre de snapshots.
struct SnapshotRegistryInner {
    /// Liste des snapshots actifs.
    snapshots:    Vec<SnapshotDescriptor>,
    /// Compteur d'allocation d'IDs (jamais réutilisé).
    next_id:      u64,
    /// Nombre total de snapshots créés (statistique).
    total_created: u64,
    /// Nombre total de snapshots supprimés (statistique).
    total_deleted: u64,
}

impl SnapshotRegistryInner {
    const fn new_uninit() -> Self {
        Self {
            snapshots:     Vec::new(),
            next_id:       1,
            total_created: 0,
            total_deleted: 0,
        }
    }
}

/// Registre global des snapshots, protégé par SpinLock.
pub struct SnapshotRegistry {
    inner: SpinLock<SnapshotRegistryInner>,
}

impl SnapshotRegistry {
    /// Crée un registre vide.
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(SnapshotRegistryInner::new_uninit()),
        }
    }

    /// Crée un snapshot et l'enregistre.
    ///
    /// # Paramètres
    /// - `rights_bits`    : bitmask de droits du demandeur.
    /// - `epoch_id`       : epoch à épingler.
    /// - `root_object_id` : objet racine snapshotté.
    /// - `name`           : nom du snapshot (tronqué à 64 octets).
    /// - `timestamp`      : horodatage TSC fourni par l'appelant.
    /// - `expires_at`     : TSC d'expiration (0 = permanent).
    ///
    /// # Retour
    /// - `Ok(snapshot_id)` : ID unique du nouveau snapshot.
    /// - `Err(ExofsError::PermissionDenied)` : droit manquant.
    /// - `Err(ExofsError::QuotaExceeded)` : MAX_SNAPSHOTS atteint.
    pub fn create(
        &self,
        rights_bits:    u64,
        epoch_id:       EpochId,
        root_object_id: ObjectId,
        name:           &[u8],
        timestamp:      u64,
        expires_at:     u64,
    ) -> ExofsResult<u64> {
        if !has_snapshot_create(rights_bits) {
            return Err(ExofsError::PermissionDenied);
        }

        let mut inner = self.inner.lock();

        if inner.snapshots.len() >= MAX_SNAPSHOTS {
            return Err(ExofsError::QuotaExceeded);
        }

        // Alloue un ID.
        let snapshot_id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1);

        // Crée le pin RAII (propriétaire = snapshot_id tronqué sur 32 bits).
        let pin = EpochPin::acquire_with_reason(
            epoch_id,
            snapshot_id as u32,
            PinReason::Snapshot,
            0,
        )?;

        // Pré-réserve pour éviter panic OOM (OOM-02).
        inner
            .snapshots
            .try_reserve(1)
            .map_err(|_| ExofsError::NoMemory)?;

        inner.snapshots.push(SnapshotDescriptor {
            snapshot_id,
            epoch_id,
            name:           SnapshotName::from_bytes(name),
            created_at:     timestamp,
            expires_at,
            root_object_id,
            pin,
        });

        inner.total_created = inner.total_created.saturating_add(1);
        EPOCH_STATS.inc_snapshots_created();

        Ok(snapshot_id)
    }

    /// Supprime un snapshot par ID.
    ///
    /// # Paramètres
    /// - `rights_bits`  : bitmask de droits du demandeur.
    /// - `snapshot_id`  : ID du snapshot à supprimer.
    ///
    /// # Retour
    /// - `Ok(())` : snapshot supprimé.
    /// - `Err(ExofsError::PermissionDenied)` : droit manquant.
    /// - `Err(ExofsError::NotFound)` : snapshot introuvable.
    pub fn delete(&self, rights_bits: u64, snapshot_id: u64) -> ExofsResult<()> {
        if !has_snapshot_delete(rights_bits) {
            return Err(ExofsError::PermissionDenied);
        }

        let mut inner = self.inner.lock();
        let pos = inner
            .snapshots
            .iter()
            .position(|s| s.snapshot_id == snapshot_id)
            .ok_or(ExofsError::NotFound)?;

        // Le drop du SnapshotDescriptor libère l'EpochPin (RAII).
        inner.snapshots.swap_remove(pos);
        inner.total_deleted = inner.total_deleted.saturating_add(1);
        EPOCH_STATS.inc_snapshots_deleted();

        Ok(())
    }

    /// Recherche un snapshot par ID.
    ///
    /// Exécute `f` avec une référence au descripteur si trouvé.
    /// Retourne `None` si absent.
    pub fn with_snapshot<R, F>(&self, snapshot_id: u64, f: F) -> Option<R>
    where
        F: FnOnce(&SnapshotDescriptor) -> R,
    {
        let inner = self.inner.lock();
        inner
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == snapshot_id)
            .map(f)
    }

    /// Recherche un snapshot par epoch_id.
    ///
    /// Retourne le snapshot_id du premier snapshot épinglant cet epoch.
    pub fn find_by_epoch(&self, epoch_id: EpochId) -> Option<u64> {
        let inner = self.inner.lock();
        inner
            .snapshots
            .iter()
            .find(|s| s.epoch_id.0 == epoch_id.0)
            .map(|s| s.snapshot_id)
    }

    /// Retourne le nombre de snapshots actuellement actifs.
    pub fn active_count(&self) -> usize {
        self.inner.lock().snapshots.len()
    }

    /// Retourne `true` si le registre est plein.
    pub fn is_full(&self) -> bool {
        self.inner.lock().snapshots.len() >= MAX_SNAPSHOTS
    }

    /// Expire et supprime les snapshots dont `expires_at <= tsc_now`.
    ///
    /// Retourne le nombre de snapshots expirés supprimés.
    /// RECUR-01 : itération linéaire, pas de récursion.
    pub fn expire_snapshots(&self, tsc_now: u64) -> usize {
        let mut inner = self.inner.lock();
        let before = inner.snapshots.len();

        // Collecte les indices à supprimer (ordre décroissant pour swap_remove stable).
        let mut to_remove: Vec<usize> = Vec::new();
        for (i, s) in inner.snapshots.iter().enumerate() {
            if s.is_expired(tsc_now) {
                // try_reserve non requis : len <= MAX_SNAPSHOTS = 64.
                let _ = to_remove.try_reserve(1);
                to_remove.push(i);
            }
        }

        // Suppression en ordre décroissant (swap_remove préserve les autres indices).
        for &idx in to_remove.iter().rev() {
            inner.snapshots.swap_remove(idx);
            inner.total_deleted = inner.total_deleted.saturating_add(1);
        }

        let removed = before.saturating_sub(inner.snapshots.len());
        removed
    }

    /// Collecte un snapshot des statistiques du registre.
    pub fn stats(&self) -> SnapshotRegistryStats {
        let inner = self.inner.lock();
        SnapshotRegistryStats {
            active_count:   inner.snapshots.len() as u32,
            total_created:  inner.total_created,
            total_deleted:  inner.total_deleted,
            oldest_epoch:   oldest_pinned_epoch(),
        }
    }

    /// Liste tous les snapshot_id actifs dans le Vec fourni.
    ///
    /// # Paramètres
    /// - `out`  : Vec de sortie (pré-alloué par l'appelant).
    /// - `rights_bits` : doit avoir RIGHT_SNAPSHOT_LIST.
    pub fn list_ids(&self, out: &mut Vec<u64>, rights_bits: u64) -> ExofsResult<()> {
        if !has_snapshot_list(rights_bits) {
            return Err(ExofsError::PermissionDenied);
        }
        let inner = self.inner.lock();
        out.try_reserve(inner.snapshots.len())
            .map_err(|_| ExofsError::NoMemory)?;
        for s in &inner.snapshots {
            out.push(s.snapshot_id);
        }
        Ok(())
    }
}

/// Registre global des snapshots.
pub static SNAPSHOT_REGISTRY: SnapshotRegistry = SnapshotRegistry::new();

// =============================================================================
// Statistiques du registre
// =============================================================================

/// Snapshot immutable des statistiques du registre de snapshots.
#[derive(Debug, Copy, Clone)]
pub struct SnapshotRegistryStats {
    /// Nombre de snapshots actuellement actifs.
    pub active_count:  u32,
    /// Total des snapshots créés depuis le démarrage.
    pub total_created: u64,
    /// Total des snapshots supprimés (ou expirés) depuis le démarrage.
    pub total_deleted: u64,
    /// Epoch le plus ancien épinglé par un snapshot (None si aucun).
    pub oldest_epoch:  Option<EpochId>,
}

impl fmt::Display for SnapshotRegistryStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SnapshotRegistry {{ active={}, created={}, deleted={}, oldest_epoch={} }}",
            self.active_count,
            self.total_created,
            self.total_deleted,
            self.oldest_epoch.map_or(0, |e| e.0),
        )
    }
}

// =============================================================================
// API de convenance (fonctions libres)
// =============================================================================

/// Crée un snapshot dans le registre global.
///
/// Voir `SnapshotRegistry::create` pour la documentation complète.
pub fn create_snapshot(
    rights_bits:    u64,
    epoch_id:       EpochId,
    root_object_id: ObjectId,
    name:           &[u8],
    timestamp:      u64,
) -> ExofsResult<u64> {
    SNAPSHOT_REGISTRY.create(rights_bits, epoch_id, root_object_id, name, timestamp, 0)
}

/// Crée un snapshot avec expiration dans le registre global.
pub fn create_snapshot_with_expiry(
    rights_bits:    u64,
    epoch_id:       EpochId,
    root_object_id: ObjectId,
    name:           &[u8],
    timestamp:      u64,
    expires_at:     u64,
) -> ExofsResult<u64> {
    SNAPSHOT_REGISTRY.create(
        rights_bits,
        epoch_id,
        root_object_id,
        name,
        timestamp,
        expires_at,
    )
}

/// Supprime un snapshot du registre global.
pub fn delete_snapshot(rights_bits: u64, snapshot_id: u64) -> ExofsResult<()> {
    SNAPSHOT_REGISTRY.delete(rights_bits, snapshot_id)
}

/// Retourne l'epoch_id épinglé par un snapshot donné.
///
/// Retourne `None` si le snapshot n'existe pas.
pub fn snapshot_epoch_id(snapshot_id: u64) -> Option<EpochId> {
    SNAPSHOT_REGISTRY.with_snapshot(snapshot_id, |s| s.epoch_id)
}

/// Retourne `true` si l'epoch donné est encore référencé par un snapshot.
pub fn epoch_has_snapshot(epoch_id: EpochId) -> bool {
    SNAPSHOT_REGISTRY.find_by_epoch(epoch_id).is_some()
}

/// Collecte les statistiques du registre global.
pub fn snapshot_registry_stats() -> SnapshotRegistryStats {
    SNAPSHOT_REGISTRY.stats()
}

/// Expire les snapshots dont le TSC a dépassé expires_at.
///
/// Retourne le nombre de snapshots supprimés.
pub fn expire_snapshots(tsc_now: u64) -> usize {
    SNAPSHOT_REGISTRY.expire_snapshots(tsc_now)
}
