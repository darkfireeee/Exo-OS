// libs/exo_ipc/src/types/endpoint.rs
//! Endpoints IPC pour identification et routage

use core::fmt;
use super::capability::CapabilityId;

/// Identifiant d'endpoint unique
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct EndpointId(pub u64);

impl EndpointId {
    /// Crée un nouvel endpoint ID
    pub const fn new(id: u64) -> Self {
        Self(id)
    }
    
    /// Endpoint invalide
    pub const INVALID: Self = Self(0);
    
    /// Endpoint broadcast (tous les endpoints)
    pub const BROADCAST: Self = Self(0xFFFFFFFFFFFFFFFF);
    
    /// Vérifie si l'endpoint est valide
    pub const fn is_valid(&self) -> bool {
        self.0 != 0
    }
    
    /// Vérifie si c'est un broadcast
    pub const fn is_broadcast(&self) -> bool {
        self.0 == Self::BROADCAST.0
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_broadcast() {
            write!(f, "Endpoint(BROADCAST)")
        } else {
            write!(f, "Endpoint({})", self.0)
        }
    }
}

/// Type d'endpoint
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EndpointType {
    /// Processus utilisateur
    Process = 0,
    
    /// Thread dans un processus
    Thread = 1,
    
    /// Service système
    Service = 2,
    
    /// Driver matériel
    Driver = 3,
    
    /// Endpoint virtuel
    Virtual = 4,
}

/// Endpoint IPC complet
#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    /// Identifiant unique
    pub id: EndpointId,
    
    /// Type d'endpoint
    pub endpoint_type: EndpointType,
    
    /// Capability associée
    pub capability: CapabilityId,
    
    /// PID du processus propriétaire
    pub owner_pid: u32,
    
    /// Flags d'état
    pub flags: u32,
}

impl Endpoint {
    /// Crée un nouvel endpoint
    pub const fn new(
        id: EndpointId,
        endpoint_type: EndpointType,
        capability: CapabilityId,
        owner_pid: u32,
    ) -> Self {
        Self {
            id,
            endpoint_type,
            capability,
            owner_pid,
            flags: 0,
        }
    }
    
    /// Endpoint pour un processus
    pub const fn process(id: EndpointId, pid: u32, capability: CapabilityId) -> Self {
        Self::new(id, EndpointType::Process, capability, pid)
    }
    
    /// Endpoint pour un service
    pub const fn service(id: EndpointId, capability: CapabilityId) -> Self {
        Self::new(id, EndpointType::Service, capability, 0)
    }
    
    /// Vérifie si l'endpoint est valide
    pub const fn is_valid(&self) -> bool {
        self.id.is_valid()
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Endpoint {{ id: {}, type: {:?}, cap: {}, owner: {} }}",
            self.id, self.endpoint_type, self.capability, self.owner_pid
        )
    }
}

/// Adresse IPC complète (endpoint + metadata)
#[derive(Debug, Clone, Copy)]
pub struct IpcAddress {
    /// Endpoint source
    pub source: EndpointId,
    
    /// Endpoint destination
    pub destination: EndpointId,
    
    /// ID de session (pour multiplexage)
    pub session_id: u32,
    
    /// Numéro de séquence
    pub sequence: u32,
}

impl IpcAddress {
    /// Crée une nouvelle adresse
    pub const fn new(source: EndpointId, destination: EndpointId) -> Self {
        Self {
            source,
            destination,
            session_id: 0,
            sequence: 0,
        }
    }
    
    /// Avec session ID
    pub const fn with_session(mut self, session_id: u32) -> Self {
        self.session_id = session_id;
        self
    }
    
    /// Avec numéro de séquence
    pub const fn with_sequence(mut self, sequence: u32) -> Self {
        self.sequence = sequence;
        self
    }
    
    /// Inverse source et destination (pour réponses)
    pub const fn reverse(&self) -> Self {
        Self {
            source: self.destination,
            destination: self.source,
            session_id: self.session_id,
            sequence: self.sequence,
        }
    }
}
