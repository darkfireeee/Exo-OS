// kernel/src/fs/exofs/epoch/epoch_snapshot.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// Snapshots basés sur les Epochs — point de restauration immuable
// Ring 0 · no_std · Exo-OS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Un snapshot ExoFS est simplement un EpochPin sur un epoch passé + un
// enregistrement persisté dans l'objet de métadonnées snapshot.
// La lecture d'un snapshot utilise l'EpochRecord de l'epoch snapshotté
// pour retrouver l'EpochRoot correspondant.
//
// RÈGLE SECURITY-01 : RIGHT_SNAPSHOT_CREATE requis pour créer un snapshot.
// RÈGLE EPOCH-07    : le snapshot is read-only — aucun commit possible dessus.

use alloc::string::String;

use crate::fs::exofs::core::{
    ExofsError, ExofsResult, EpochId, ObjectId,
};
use crate::fs::exofs::core::rights::has_snapshot_create;
use crate::fs::exofs::epoch::epoch_pin::EpochPin;
use crate::security::capability::CapToken;

// ─────────────────────────────────────────────────────────────────────────────
// Descripteur de snapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Métadonnées in-memory d'un snapshot.
#[derive(Debug)]
pub struct SnapshotDescriptor {
    /// SnapshotId unique.
    pub snapshot_id:    u64,
    /// Epoch épinglé par ce snapshot.
    pub epoch_id:       EpochId,
    /// Nom du snapshot (max 64 caractères).
    pub name:           SnapshotName,
    /// Timestamp de création (TSC).
    pub created_at:     u64,
    /// ObjectId de l'objet racine snapshotté.
    pub root_object_id: ObjectId,
    /// Pin RAII — maintient l'epoch vivant tant que le snapshot existe.
    pin:                EpochPin,
}

/// Nom du snapshot — wraps [u8; 64] pour éviter String heap allocation.
#[derive(Copy, Clone, Debug)]
pub struct SnapshotName([u8; 64]);

impl SnapshotName {
    /// Crée un nom depuis une slice UTF-8. Tronque à 64 octets si nécessaire.
    pub fn from_bytes(src: &[u8]) -> Self {
        let mut buf = [0u8; 64];
        let len = src.len().min(64);
        buf[..len].copy_from_slice(&src[..len]);
        Self(buf)
    }

    /// Retourne le nom comme slice d'octets (sans nul terminateur).
    pub fn as_bytes(&self) -> &[u8] {
        let end = self.0.iter().position(|&b| b == 0).unwrap_or(64);
        &self.0[..end]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Création de snapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Crée un snapshot de l'epoch donné.
///
/// # Prérequis
/// - Le token doit posséder RIGHT_SNAPSHOT_CREATE (règle SECURITY-01).
/// - L'epoch donné doit être < epoch courant (on ne snapshot pas l'epoch actif).
///
/// # Paramètres
/// - `token`          : capability token du demandeur.
/// - `epoch_id`       : epoch à snapshotter.
/// - `root_object_id` : objet racine de la vue snapshot.
/// - `name`           : nom du snapshot (sera tronqué à 64 octets).
/// - `timestamp`      : horodatage fourni par l'appelant.
/// - `snapshot_id`    : identifiant unique alloué par le snapshot manager.
pub fn create_snapshot(
    token:          &CapToken,
    epoch_id:       EpochId,
    root_object_id: ObjectId,
    name:           &[u8],
    timestamp:      u64,
    snapshot_id:    u64,
) -> ExofsResult<SnapshotDescriptor> {
    // Vérification du droit de création de snapshot.
    if !has_snapshot_create(token.rights().bits()) {
        return Err(ExofsError::PermissionDenied);
    }

    // On épingle l'epoch avec l'owner = snapshot_id (tronqué à u32).
    let pin = EpochPin::acquire(epoch_id, snapshot_id as u32)?;

    Ok(SnapshotDescriptor {
        snapshot_id,
        epoch_id,
        name:           SnapshotName::from_bytes(name),
        created_at:     timestamp,
        root_object_id,
        pin,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Suppression de snapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Supprime un snapshot (décrément le pin et libère le descripteur).
///
/// Le drop de `descriptor` libère automatiquement l'EpochPin (RAII).
pub fn delete_snapshot(descriptor: SnapshotDescriptor) -> ExofsResult<()> {
    // Le drop ici libère l'EpochPin implicitement.
    drop(descriptor);
    Ok(())
}
