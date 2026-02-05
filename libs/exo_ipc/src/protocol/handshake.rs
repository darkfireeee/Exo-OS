// libs/exo_ipc/src/protocol/handshake.rs
//! Protocole de handshake pour négociation de version

use crate::types::{Message, MessageType, MessageFlags, IpcError, IpcResult, PROTOCOL_VERSION};

/// Capabilities supportées par un endpoint
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities(u32);

impl Capabilities {
    /// Aucune capability
    pub const NONE: Self = Self(0);
    
    /// Support des messages zero-copy
    pub const ZERO_COPY: Self = Self(1 << 0);
    
    /// Support des messages fragmentés
    pub const FRAGMENTATION: Self = Self(1 << 1);
    
    /// Support de la compression
    pub const COMPRESSION: Self = Self(1 << 2);
    
    /// Support du chiffrement
    pub const ENCRYPTION: Self = Self(1 << 3);
    
    /// Support des checksums
    pub const CHECKSUMS: Self = Self(1 << 4);
    
    /// Support du flow control
    pub const FLOW_CONTROL: Self = Self(1 << 5);
    
    /// Support des priorités
    pub const PRIORITIES: Self = Self(1 << 6);
    
    /// Toutes les capabilities
    pub const ALL: Self = Self(0xFFFFFFFF);
    
    /// Capabilities de base (toujours supportées)
    pub const BASIC: Self = Self(
        Self::CHECKSUMS.0 | Self::FRAGMENTATION.0
    );
    
    /// Crée un ensemble de capabilities
    pub const fn new() -> Self {
        Self::NONE
    }
    
    /// Ajoute une capability
    pub const fn with(self, cap: Self) -> Self {
        Self(self.0 | cap.0)
    }
    
    /// Vérifie si une capability est supportée
    pub const fn has(&self, cap: Self) -> bool {
        (self.0 & cap.0) == cap.0
    }
    
    /// Intersection de capabilities (ce qui est supporté par les deux)
    pub const fn intersect(&self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// État d'une session de handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// Pas encore démarré
    NotStarted,
    
    /// HELLO envoyé, attente ACK
    HelloSent,
    
    /// HELLO reçu, ACK envoyé
    HelloReceived,
    
    /// Handshake complété
    Completed,
    
    /// Handshake échoué
    Failed,
}

/// Configuration négociée lors du handshake
#[derive(Debug, Clone, Copy)]
pub struct SessionConfig {
    /// Version du protocole négociée
    pub protocol_version: u16,
    
    /// Capabilities communes
    pub capabilities: Capabilities,
    
    /// Taille maximale de message
    pub max_message_size: u32,
    
    /// Taille du buffer de réception
    pub recv_buffer_size: u32,
}

impl SessionConfig {
    /// Configuration par défaut
    pub fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            capabilities: Capabilities::BASIC,
            max_message_size: 65536, // 64 KB
            recv_buffer_size: 1024 * 1024, // 1 MB
        }
    }
}

/// Gestionnaire de handshake
pub struct HandshakeManager {
    state: HandshakeState,
    local_config: SessionConfig,
    negotiated_config: Option<SessionConfig>,
}

impl HandshakeManager {
    /// Crée un nouveau gestionnaire
    pub fn new(local_config: SessionConfig) -> Self {
        Self {
            state: HandshakeState::NotStarted,
            local_config,
            negotiated_config: None,
        }
    }
    
    /// Crée un message HELLO pour initier le handshake
    pub fn create_hello(&mut self) -> IpcResult<Message> {
        if self.state != HandshakeState::NotStarted {
            return Err(IpcError::InvalidState);
        }
        
        let mut msg = Message::new(MessageType::Handshake);
        
        // Encoder les informations dans les données inline
        let data = [
            // Version (2 bytes)
            (self.local_config.protocol_version & 0xFF) as u8,
            (self.local_config.protocol_version >> 8) as u8,
            
            // Capabilities (4 bytes)
            (self.local_config.capabilities.0 & 0xFF) as u8,
            ((self.local_config.capabilities.0 >> 8) & 0xFF) as u8,
            ((self.local_config.capabilities.0 >> 16) & 0xFF) as u8,
            ((self.local_config.capabilities.0 >> 24) & 0xFF) as u8,
            
            // Max message size (4 bytes)
            (self.local_config.max_message_size & 0xFF) as u8,
            ((self.local_config.max_message_size >> 8) & 0xFF) as u8,
            ((self.local_config.max_message_size >> 16) & 0xFF) as u8,
            ((self.local_config.max_message_size >> 24) & 0xFF) as u8,
            
            // Recv buffer size (4 bytes)
            (self.local_config.recv_buffer_size & 0xFF) as u8,
            ((self.local_config.recv_buffer_size >> 8) & 0xFF) as u8,
            ((self.local_config.recv_buffer_size >> 16) & 0xFF) as u8,
            ((self.local_config.recv_buffer_size >> 24) & 0xFF) as u8,
        ];
        
        if let Some(inline_data) = msg.inline_data_mut() {
            inline_data[..data.len()].copy_from_slice(&data);
        }
        
        msg.header.data_size = data.len() as u16;
        msg.header.flags = msg.header.flags.with(MessageFlags::INLINE);
        
        self.state = HandshakeState::HelloSent;
        Ok(msg)
    }
    
    /// Traite un message HELLO reçu et crée un ACK
    pub fn process_hello(&mut self, hello_msg: &Message) -> IpcResult<Message> {
        if hello_msg.header.msg_type as u16 != MessageType::Handshake as u16 {
            return Err(IpcError::InvalidMessage);
        }
        
        let data = hello_msg.inline_data().ok_or(IpcError::InvalidMessage)?;
        if data.len() < 14 {
            return Err(IpcError::InvalidMessage);
        }
        
        // Décoder la configuration du peer
        let peer_version = u16::from_le_bytes([data[0], data[1]]);
        let peer_caps = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        let peer_max_msg = u32::from_le_bytes([data[6], data[7], data[8], data[9]]);
        let peer_recv_buf = u32::from_le_bytes([data[10], data[11], data[12], data[13]]);
        
        // Négocier la configuration
        let negotiated = SessionConfig {
            protocol_version: peer_version.min(self.local_config.protocol_version),
            capabilities: Capabilities(peer_caps).intersect(self.local_config.capabilities),
            max_message_size: peer_max_msg.min(self.local_config.max_message_size),
            recv_buffer_size: peer_recv_buf.min(self.local_config.recv_buffer_size),
        };
        
        // Vérifier la compatibilité
        if negotiated.protocol_version == 0 {
            self.state = HandshakeState::Failed;
            return Err(IpcError::IncompatibleVersion {
                local: self.local_config.protocol_version,
                remote: peer_version,
            });
        }
        
        self.negotiated_config = Some(negotiated);
        self.state = HandshakeState::HelloReceived;
        
        // Créer le ACK
        self.create_ack()
    }
    
    /// Crée un message ACK
    fn create_ack(&self) -> IpcResult<Message> {
        let config = self.negotiated_config.ok_or(IpcError::InvalidState)?;
        
        let mut msg = Message::new(MessageType::Ack);
        
        // Encoder la configuration négociée
        let data = [
            (config.protocol_version & 0xFF) as u8,
            (config.protocol_version >> 8) as u8,
            (config.capabilities.0 & 0xFF) as u8,
            ((config.capabilities.0 >> 8) & 0xFF) as u8,
            ((config.capabilities.0 >> 16) & 0xFF) as u8,
            ((config.capabilities.0 >> 24) & 0xFF) as u8,
        ];
        
        if let Some(inline_data) = msg.inline_data_mut() {
            inline_data[..data.len()].copy_from_slice(&data);
        }
        
        msg.header.data_size = data.len() as u16;
        msg.header.flags = msg.header.flags.with(MessageFlags::INLINE);
        
        Ok(msg)
    }
    
    /// Traite un ACK reçu
    pub fn process_ack(&mut self, ack_msg: &Message) -> IpcResult<()> {
        if ack_msg.header.msg_type as u16 != MessageType::Ack as u16 {
            return Err(IpcError::InvalidMessage);
        }
        
        if self.state != HandshakeState::HelloSent {
            return Err(IpcError::InvalidState);
        }
        
        let data = ack_msg.inline_data().ok_or(IpcError::InvalidMessage)?;
        if data.len() < 6 {
            return Err(IpcError::InvalidMessage);
        }
        
        let version = u16::from_le_bytes([data[0], data[1]]);
        let caps = u32::from_le_bytes([data[2], data[3], data[4], data[5]]);
        
        self.negotiated_config = Some(SessionConfig {
            protocol_version: version,
            capabilities: Capabilities(caps),
            max_message_size: self.local_config.max_message_size,
            recv_buffer_size: self.local_config.recv_buffer_size,
        });
        
        self.state = HandshakeState::Completed;
        Ok(())
    }
    
    /// Récupère l'état actuel
    pub fn state(&self) -> HandshakeState {
        self.state
    }
    
    /// Vérifie si le handshake est complété
    pub fn is_completed(&self) -> bool {
        self.state == HandshakeState::Completed
    }
    
    /// Récupère la configuration négociée
    pub fn negotiated_config(&self) -> Option<SessionConfig> {
        self.negotiated_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_capabilities() {
        let caps = Capabilities::BASIC
            .with(Capabilities::ZERO_COPY)
            .with(Capabilities::COMPRESSION);
        
        assert!(caps.has(Capabilities::CHECKSUMS));
        assert!(caps.has(Capabilities::ZERO_COPY));
        assert!(caps.has(Capabilities::COMPRESSION));
        assert!(!caps.has(Capabilities::ENCRYPTION));
    }
    
    #[test]
    fn test_handshake_flow() {
        let config = SessionConfig::default();
        
        let mut client = HandshakeManager::new(config);
        let mut server = HandshakeManager::new(config);
        
        // Client envoie HELLO
        let hello = client.create_hello().unwrap();
        assert_eq!(client.state(), HandshakeState::HelloSent);
        
        // Server traite HELLO et envoie ACK
        let ack = server.process_hello(&hello).unwrap();
        assert_eq!(server.state(), HandshakeState::HelloReceived);
        
        // Client traite ACK
        client.process_ack(&ack).unwrap();
        assert!(client.is_completed());
        
        // Vérifier que les configurations sont compatibles
        assert!(client.negotiated_config().is_some());
        assert!(server.negotiated_config().is_some());
    }
}
