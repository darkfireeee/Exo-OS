// libs/exo_types/src/capability.rs
use alloc::string::{String, ToString};
use bitflags::bitflags;
use core::fmt;

bitflags! {
    /// Permissions pour les capabilities
    pub struct Rights: u32 {
        const NONE        = 0b0000_0000;
        const READ        = 0b0000_0001;
        const WRITE       = 0b0000_0010;
        const EXECUTE     = 0b0000_0100;
        const DELETE      = 0b0000_1000;
        const METADATA    = 0b0001_0000;
        const CREATE_FILE = 0b0010_0000;
        const CREATE_DIR  = 0b0100_0000;
        const NET_BIND    = 0b1000_0000;
        const NET_CONNECT = 0b0001_0000_0000;
        const IPC_SEND    = 0b0010_0000_0000;
        const IPC_RECV    = 0b0100_0000_0000;

        /// Tous les droits standard pour un fichier
        const FILE_STANDARD = Self::READ.bits | Self::WRITE.bits | Self::METADATA.bits;

        /// Tous les droits standard pour un répertoire
        const DIR_STANDARD = Self::FILE_STANDARD.bits | Self::CREATE_FILE.bits | Self::CREATE_DIR.bits;

        /// Tous les droits standard pour un socket réseau
        const SOCKET_STANDARD = Self::READ.bits | Self::WRITE.bits | Self::NET_BIND.bits | Self::NET_CONNECT.bits;
    }
}

/// Type de capability pour les objets système
#[derive(Debug, Clone)]
pub enum CapabilityType {
    File,
    Directory,
    Memory,
    Process,
    Thread,
    IpcChannel,
    NetworkSocket,
    Device,
    Key,
}

/// Structure représentant une capability
#[derive(Debug, Clone)]
pub struct Capability {
    /// Identifiant unique de la capability
    id: u64,

    /// Type d'objet référencé
    cap_type: CapabilityType,

    /// Permissions accordées
    rights: Rights,

    /// Méta-données supplémentaires
    metadata: Option<CapabilityMetadata>,
}

/// Métadonnées pour les capabilities
#[derive(Debug, Clone)]
pub struct CapabilityMetadata {
    /// Chemin ou nom pour les objets nommés
    path: Option<String>,

    /// Taille pour les objets mémoire
    size: Option<usize>,

    /// Permissions supplémentaires spécifiques au type
    extra_flags: u32,
}

impl Capability {
    /// Crée une nouvelle capability
    pub fn new(id: u64, cap_type: CapabilityType, rights: Rights) -> Self {
        Capability {
            id,
            cap_type,
            rights,
            metadata: None,
        }
    }

    /// Atténue les permissions d'une capability (droits uniquement réduits)
    pub fn attenuate(&self, new_rights: Rights) -> Self {
        let reduced_rights = self.rights & new_rights;

        Capability {
            id: self.id,
            cap_type: self.cap_type.clone(),
            rights: reduced_rights,
            metadata: self.metadata.clone(),
        }
    }

    /// Ajoute des métadonnées à la capability
    pub fn with_metadata(mut self, metadata: CapabilityMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Vérifie si la capability a les droits demandés
    pub fn has_rights(&self, required: Rights) -> bool {
        self.rights.contains(required)
    }

    /// Retourne le type de capability
    pub fn cap_type(&self) -> &CapabilityType {
        &self.cap_type
    }

    /// Retourne les permissions actuelles
    pub fn rights(&self) -> Rights {
        self.rights
    }
}

impl CapabilityMetadata {
    /// Crée des métadonnées pour un fichier
    pub fn for_file(path: &str, size: usize) -> Self {
        CapabilityMetadata {
            path: Some(path.to_string()),
            size: Some(size),
            extra_flags: 0,
        }
    }

    /// Crée des métadonnées pour un espace mémoire partagé
    pub fn for_memory(size: usize, executable: bool) -> Self {
        CapabilityMetadata {
            path: None,
            size: Some(size),
            extra_flags: if executable { 1 } else { 0 },
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Capability(id={}, type={:?}, rights={:?})",
            self.id, self.cap_type, self.rights
        )
    }
}

