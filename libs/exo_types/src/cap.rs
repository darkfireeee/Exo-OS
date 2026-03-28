// libs/exo-types/src/cap.rs
//
// Fichier : libs/exo_types/src/cap.rs
// Rôle    : CapToken, CapabilityType, Rights, verify_cap_token() — GI-01 Étape 4.
//
// INVARIANTS :
//   - CAP-01 : verify_cap_token() doit être la première instruction de main.rs
//              de chaque server Ring 1. panic! si token invalide.
//   - CORR-05 : enum #[repr(u16)] — AUCUNE variante avec données inline
//               (E0517 : illégal avec repr(C/u*)). Les paramètres (ex: BDF PCI)
//               sont encodés dans CapToken.object_id.
//   - CORR-52 : verify_cap_token() utilise subtle::ConstantTimeEq (constant-time).
//   - B3      : verify_cap_token() partagé ici — évite divergence entre servers.
//
// SOURCE DE VÉRITÉ :
//   ExoOS_Architecture_v7.md §1.3, ExoOS_Corrections_01 CORR-05,
//   ExoOS_Corrections_06 CORR-17, GI-01_Types_TCB_SSR.md §9

use crate::object_id::ObjectId;
use subtle::ConstantTimeEq;

// ─── Rights — Bitmask des droits accordés par un CapToken ────────────────────

/// Droits associés à un `CapToken`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Rights(pub u32);

impl Rights {
    /// Droit de lecture.
    pub const READ:    Self = Rights(0x01);
    /// Droit d'écriture.
    pub const WRITE:   Self = Rights(0x02);
    /// Droit d'exécution.
    pub const EXEC:    Self = Rights(0x04);
    /// Droit d'inspection (SYS_EXOFS_GET_CONTENT_HASH).
    pub const INSPECT: Self = Rights(0x08);
    /// Droit de délégation — transmettre ce token à un autre processus.
    pub const GRANT:   Self = Rights(0x10);
    /// Tous les droits (READ | WRITE | EXEC | INSPECT | GRANT).
    pub const ALL:     Self = Rights(0x1F);
    /// Aucun droit.
    pub const NONE:    Self = Rights(0x00);

    /// `true` si lecture autorisée.
    #[inline(always)]
    pub fn can_read(self)   -> bool { self.0 & Self::READ.0   != 0 }
    /// `true` si écriture autorisée.
    #[inline(always)]
    pub fn can_write(self)  -> bool { self.0 & Self::WRITE.0  != 0 }
    /// `true` si exécution autorisée.
    #[inline(always)]
    pub fn can_exec(self)   -> bool { self.0 & Self::EXEC.0   != 0 }
    /// `true` si inspection autorisée.
    #[inline(always)]
    pub fn can_inspect(self)-> bool { self.0 & Self::INSPECT.0!= 0 }

    /// `true` si `self` contient tous les droits de `other`.
    #[inline(always)]
    pub fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }
}

// ─── CapabilityType — Discriminant pur (CORR-05) ─────────────────────────────

/// Type de capability ExoOS.
///
/// **CORR-05** : `#[repr(u16)]` sans variantes à données.
/// Les paramètres (ex: BDF PCI pour `DriverPci`) sont encodés dans
/// `CapToken.object_id[0..3]` : `bus`, `device`, `function`.
///
/// ❌ ILLÉGAL (E0517 Rust) :
/// ```ignore
/// #[repr(C)]
/// pub enum CapabilityType {
///     Driver { pci_id: u16 },  // ← E0517 : variante avec données interdit avec repr(C)
/// }
/// ```
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityType {
    /// Connexion au directory service IPC.
    IpcBroker      = 1,
    /// Allocation mémoire via memory_server.
    MemoryServer   = 2,
    /// Driver PCI Ring 1. BDF encodé dans `CapToken.object_id[0..3]`.
    DriverPci      = 3,
    /// Administration système (SYS_PCI_CLAIM, SYS_PCI_SET_TOPOLOGY...).
    SysDeviceAdmin = 4,
    /// Accès ExoFS (read/write/stat...).
    ExoFsAccess    = 5,
    /// Communication avec crypto_server.
    CryptoServer   = 6,
    /// Interface ExoPhoenix — exo_shield uniquement.
    ExoPhoenix     = 7,
    /// Accès VFS (server vfs_server).
    VfsServer      = 8,
    /// Politique de scheduling (scheduller_server).
    SchedulerServer= 9,
}

impl CapabilityType {
    /// Convertit depuis le discriminant u16 — retourne `None` si inconnu.
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::IpcBroker),
            2 => Some(Self::MemoryServer),
            3 => Some(Self::DriverPci),
            4 => Some(Self::SysDeviceAdmin),
            5 => Some(Self::ExoFsAccess),
            6 => Some(Self::CryptoServer),
            7 => Some(Self::ExoPhoenix),
            8 => Some(Self::VfsServer),
            9 => Some(Self::SchedulerServer),
            _ => None,
        }
    }
}

// ─── CapToken — Token de capability ──────────────────────────────────────────

/// Token de capability ExoOS.
///
/// **Vérification** : via `verify_cap_token()` (constant-time — CORR-52).
/// **Révocation** : instantanée via incrément de `generation` (0 = révoqué).
///
/// **Encodage BDF** (pour `CapabilityType::DriverPci`) :
/// - `object_id.0[0]` = `bus`
/// - `object_id.0[1]` = `device`
/// - `object_id.0[2]` = `function`
/// - `object_id.0[3..32]` = zéro
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CapToken {
    /// Génération anti-replay — incrémentée à chaque émission. `0` = révoqué.
    pub generation: u64,
    /// Ressource cible. Pour `DriverPci` : bytes[0..3] = BDF compact.
    pub object_id:  ObjectId,
    /// Droits accordés sur la ressource.
    pub rights:     u32,
    /// Discriminant du type (`CapabilityType as u16`).
    pub type_id:    u16,
    /// Padding alignement — RÉSERVÉ, doit être zéro.
    pub _pad:       [u8; 2],
}

const _: () = assert!(core::mem::size_of::<CapToken>() == 48);

// ─── verify_cap_token — CAP-01 + CORR-52 ─────────────────────────────────────

/// Vérifie qu'un `CapToken` est valide et correspond au type attendu.
///
/// **CAP-01** : Doit être la **première instruction** de `main.rs` de chaque server.
/// **CORR-52** : Implémentation constant-time via `subtle` (protection timing attack).
///
/// # Comportement
/// - Retourne `true` si le token est valide.
/// - **`panic!`** si le token est invalide (CAP-01 : arrêt immédiat obligatoire).
///
/// ❌ ERREURS COMMUNES :
/// 1. Comparer `token.type_id == expected as u16` directement →
///    attaque de timing : branch dépend du secret.
/// 2. Retourner `bool` sans paniquer → le server peut démarrer sans capacité valide.
/// 3. Appeler APRÈS une opération → le token doit être vérifié EN PREMIER.
pub fn verify_cap_token(token: &CapToken, expected: CapabilityType) -> bool {
    // Comparaison constant-time du type_id (CORR-52)
    let type_match = token.type_id
        .to_le_bytes()
        .ct_eq(&(expected as u16).to_le_bytes());

    // generation != 0 → token non révoqué
    let gen_nonzero = !token.generation
        .to_le_bytes()
        .ct_eq(&[0u8; 8]);

    let result = bool::from(type_match & gen_nonzero);

    if !result {
        // CAP-01 : arrêt immédiat si token invalide — aucune exception
        panic!("SECURITY: CapToken invalide (type={:#x}, gen={}) — arrêt immédiat",
               token.type_id, token.generation);
    }

    result
}

/// Construit un `CapToken` pour un driver PCI avec BDF encodé.
///
/// Utilisé par le kernel lors de l'émission des capabilities aux drivers.
pub fn make_driver_pci_cap(generation: u64, bus: u8, device: u8, function: u8, rights: Rights) -> CapToken {
    let mut object_id = ObjectId([0u8; 32]);
    object_id.0[0] = bus;
    object_id.0[1] = device;
    object_id.0[2] = function;
    CapToken {
        generation,
        object_id,
        rights: rights.0,
        type_id: CapabilityType::DriverPci as u16,
        _pad: [0u8; 2],
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    fn make_valid_token(cap_type: CapabilityType) -> CapToken {
        CapToken {
            generation: 1,
            object_id:  ObjectId([0u8; 32]),
            rights:     Rights::READ.0,
            type_id:    cap_type as u16,
            _pad:       [0u8; 2],
        }
    }

    #[test]
    fn valid_token_passes() {
        let token = make_valid_token(CapabilityType::IpcBroker);
        assert!(verify_cap_token(&token, CapabilityType::IpcBroker));
    }

    #[test]
    #[should_panic(expected = "SECURITY: CapToken invalide")]
    fn wrong_type_panics() {
        let token = make_valid_token(CapabilityType::MemoryServer);
        verify_cap_token(&token, CapabilityType::IpcBroker);
    }

    #[test]
    #[should_panic(expected = "SECURITY: CapToken invalide")]
    fn zero_generation_panics() {
        let mut token = make_valid_token(CapabilityType::IpcBroker);
        token.generation = 0; // révoqué
        verify_cap_token(&token, CapabilityType::IpcBroker);
    }

    #[test]
    fn cap_token_size() {
        assert_eq!(core::mem::size_of::<CapToken>(), 48);
    }

    #[test]
    fn capability_type_roundtrip() {
        for v in [1u16, 2, 3, 4, 5, 6, 7, 8, 9] {
            assert!(CapabilityType::from_u16(v).is_some());
        }
        assert!(CapabilityType::from_u16(0).is_none());
        assert!(CapabilityType::from_u16(10).is_none());
    }
}
