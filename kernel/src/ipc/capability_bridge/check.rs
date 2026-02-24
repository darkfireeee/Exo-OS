// kernel/src/ipc/capability_bridge/check.rs
//
// ═══════════════════════════════════════════════════════════════════════════════
// CHECK — Vérification de capabilities pour accès IPC
// (Exo-OS · IPC Couche 2a · Shim ~50 lignes)
// ═══════════════════════════════════════════════════════════════════════════════
//
// RÈGLE ABSOLUE (DOC1, Correction C2) :
//   Ce fichier ne contient PAS de logique de capability.
//   Il ne fait que déléguer à security::capability::verify().
//   Toute modification de la logique d'autorisation va dans security/*.
//
// PÉRIMÈTRE : wrapper de mapping IPC-endpoint → ObjectId + appel verify().
// ═══════════════════════════════════════════════════════════════════════════════

#![allow(dead_code)]

use crate::ipc::core::types::{EndpointId, ChannelId, IpcCapError, IpcError};

// ─────────────────────────────────────────────────────────────────────────────
// Types mirrors — security/capability/*.rs est vide pour l'instant.
// Ces types correspondent EXACTEMENT à ce que security/capability/ exposera.
// Quand security/ sera implémenté, supprimer ces mirrors et importer direct.
// ─────────────────────────────────────────────────────────────────────────────

/// Token de capability opaque — miroir de security::capability::CapToken.
/// 128 bits logiques, inforgeable (18 bytes selon ABI Rust + pad).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(C)]
pub struct CapToken {
    /// Identifiant de l'objet protégé.
    pub object_id: u64,
    /// Génération du token (invalide si ≠ génération courante de l'objet).
    pub generation: u32,
    /// Droits accordés (bitmask Rights).
    pub rights: u16,
    /// Padding (2 bytes).
    pub _pad: u16,
}

/// Droits de capability — miroir de security::capability::Rights.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct Rights(pub u16);

impl Rights {
    pub const READ:     Self = Self(1 << 0);
    pub const WRITE:    Self = Self(1 << 1);
    pub const EXECUTE:  Self = Self(1 << 2);
    pub const SEND:     Self = Self(1 << 3);
    pub const RECEIVE:  Self = Self(1 << 4);
    pub const DELEGATE: Self = Self(1 << 5);
    pub const CONNECT:  Self = Self(1 << 6);
    pub const LISTEN:   Self = Self(1 << 7);

    #[inline(always)]
    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Table de capabilities — référence vers la table du processus courant.
/// Type opaque — la logique est dans security/capability/table.rs.
pub struct CapTable {
    // En production : pointeur vers la CapTable du PCB du processus.
    // Ici : stub jusqu'à ce que security/ soit pleinement implémenté.
    _opaque: u64,
}

impl CapTable {
    /// Crée une table de capabilities de test (toutes les permissions accordées).
    /// ATTENTION : uniquement pour le bootstrap et les tests — JAMAIS en production.
    pub const fn trusted() -> Self {
        Self { _opaque: 0xDEAD_BEEF_0000_0001 }
    }

    /// Retourne vrai si c'est la table de confiance (bootstrap).
    #[inline(always)]
    fn is_trusted(&self) -> bool {
        self._opaque == 0xDEAD_BEEF_0000_0001
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// verify() — DÉLÉGATION à security::capability::verify()
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie qu'un token donne les droits `required` sur l'objet `object_id`.
///
/// DÉLÈGUE à security::capability::verify() — toute la logique est là-bas.
/// Ce wrapper ne fait que le mapping IPC-spécifique (EndpointId → ObjectId).
///
/// Quand security/capability/ sera implémenté :
///   Remplacer ce corps par :
///   ```
///   crate::security::capability::verify(table, token, required_rights)
///       .map_err(IpcCapError::from)
///   ```
#[inline]
fn verify_raw(
    table:          &CapTable,
    token:          CapToken,
    object_id:      u64,
    required_rights: Rights,
) -> Result<(), IpcCapError> {
    // ── Implémentation de stub jusqu'à security/capability/ ─────────────
    // En production, ce bloc est remplacé par l'appel à security::capability::verify().
    if table.is_trusted() {
        return Ok(()); // table bootstrap — tous droits accordés
    }
    if token.object_id != object_id {
        return Err(IpcCapError::ObjectNotFound);
    }
    if token.rights & required_rights.0 != required_rights.0 {
        return Err(IpcCapError::InsufficientRights);
    }
    // Vérification génération — O(1), jamais de parcours.
    // En production : table.get(object_id).ok_or(ObjectNotFound)?
    //                 entry.generation == token.generation → ok
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Fonctions publiques du bridge
// ─────────────────────────────────────────────────────────────────────────────

/// Vérifie l'accès IPC à un endpoint via son EndpointId.
///
/// Mappe EndpointId → ObjectId (les 32 bits bas avec tag 0x01 dans les bits hauts).
pub fn verify_endpoint_access(
    table:    &CapTable,
    token:    CapToken,
    endpoint: EndpointId,
    rights:   Rights,
) -> Result<(), IpcError> {
    let object_id = endpoint.to_object_id();
    verify_raw(table, token, object_id, rights)
        .map_err(IpcError::from)
}

/// Vérifie l'accès IPC à un canal via son ChannelId.
///
/// Tag 0x02 dans les bits hauts pour distinguer des endpoints.
pub fn verify_channel_access(
    table:   &CapTable,
    token:   CapToken,
    channel: ChannelId,
    rights:  Rights,
) -> Result<(), IpcError> {
    let object_id = (0x02u64 << 32) | (channel.get() & 0xFFFF_FFFF);
    verify_raw(table, token, object_id, rights)
        .map_err(IpcError::from)
}

// ─────────────────────────────────────────────────────────────────────────────
// IpcCapBridge trait — interface pour les objets IPC nécessitant vérification
// ─────────────────────────────────────────────────────────────────────────────

/// Trait implémenté par les objets IPC (endpoints, canaux) qui doivent
/// vérifier les capabilities avant toute opération.
pub trait IpcCapBridge {
    /// Vérifie que l'appelant a les droits requis pour cette opération.
    fn check_access(&self, table: &CapTable, token: CapToken, rights: Rights)
        -> Result<(), IpcError>;
}

/// Implémentation générique pour les endpoints.
pub struct EndpointCapBridge {
    pub endpoint_id: EndpointId,
}

impl IpcCapBridge for EndpointCapBridge {
    #[inline]
    fn check_access(&self, table: &CapTable, token: CapToken, rights: Rights)
        -> Result<(), IpcError>
    {
        verify_endpoint_access(table, token, self.endpoint_id, rights)
    }
}

// Alias publique pour la compatibilité avec le code appelant.
pub use verify_endpoint_access as verify_ipc_access;
