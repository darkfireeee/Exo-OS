// kernel/src/security/capability/access_control/object_types.rs
//
// ObjectKind — catalogue des types d'objets protégés (v6)
//
// Chaque variante documente les droits attendus pour les opérations typiques.
// Utilisé par checker::check_access() pour l'audit et les messages d'erreur.

use crate::security::capability::Rights;

/// Type d'objet protégé par le système de capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectKind {
    /// Canal IPC (nommé ou anonyme).
    /// Droits : `READ` (recv) | `WRITE` (send)
    IpcChannel  = 0,
    /// Endpoint IPC (point de connexion de service).
    /// Droits : `EXEC` (connect) | `WRITE` (publish)
    IpcEndpoint = 1,
    /// Région de mémoire partagée.
    /// Droits : `READ` | `WRITE`
    ShmRegion   = 2,
    /// Fichier régulier.
    /// Droits : `READ` | `WRITE` | `EXEC`
    File        = 3,
    /// Répertoire.
    /// Droits : `READ` (list) | `WRITE` (create/delete)
    Directory   = 4,
    /// Processus ou thread.
    /// Droits : `WRITE` (signal) | `EXEC` (ptrace/debug)
    Process     = 5,
    /// Clé cryptographique.
    /// Droits : `EXEC` (utilisation de la clé)
    CryptoKey   = 6,
}

impl ObjectKind {
    /// Droits minimaux suggérés pour les opérations typiques sur ce type.
    /// Utilisé dans les messages d'erreur audit ; n'est PAS un whitelist.
    pub const fn typical_rights(self) -> Rights {
        match self {
            Self::IpcChannel   => Rights::IPC_BASIC,
            Self::IpcEndpoint  => Rights::EXEC,
            Self::ShmRegion    => Rights::READ_WRITE,
            Self::File         => Rights::READ,
            Self::Directory    => Rights::READ,
            Self::Process      => Rights::WRITE,
            Self::CryptoKey    => Rights::EXEC,
        }
    }

    /// Nom lisible pour les logs d'audit.
    pub const fn name(self) -> &'static str {
        match self {
            Self::IpcChannel   => "IpcChannel",
            Self::IpcEndpoint  => "IpcEndpoint",
            Self::ShmRegion    => "ShmRegion",
            Self::File         => "File",
            Self::Directory    => "Directory",
            Self::Process      => "Process",
            Self::CryptoKey    => "CryptoKey",
        }
    }
}

impl core::fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}
