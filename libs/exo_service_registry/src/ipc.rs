//! IPC server et client pour service registry
//!
//! Intégration complète avec exo_ipc pour communication inter-processus.
//!
//! Architecture:
//! ```text
//! IpcClient → [MPSC Channel] → IpcServer → RegistryDaemon → Registry
//! ```
//!
//! ## Usage
//!
//! ### Serveur:
//! ```ignore
//! use exo_service_registry::ipc::IpcServer;
//! use exo_service_registry::Registry;
//!
//! let registry = Box::new(Registry::new());
//! let mut server = IpcServer::new(registry)?;
//! server.run()?; // Bloque et traite les requêtes
//! ```
//!
//! ### Client:
//! ```ignore
//! use exo_service_registry::ipc::IpcClient;
//! use exo_service_registry::{ServiceName, ServiceInfo};
//!
//! let mut client = IpcClient::new()?;
//! let name = ServiceName::new("my_service")?;
//! let info = ServiceInfo::new("/tmp/my_service.sock");
//! client.register(name, info)?;
//! ```

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use exo_ipc::{
    channel::mpsc,
    Message, MessageType, IpcError, IpcResult,
    RecvError, SenderMpsc, ReceiverMpsc,
};

use crate::daemon::RegistryDaemon;
use crate::protocol::{RegistryRequest, RegistryResponse, ResponseType};
use crate::serialize::{BinarySerialize, SerializeError};
use crate::types::{ServiceName, ServiceInfo, RegistryError, RegistryResult};

/// Convertit SerializeError en IpcError
impl From<SerializeError> for IpcError {
    fn from(err: SerializeError) -> Self {
        match err {
            SerializeError::BufferTooSmall => IpcError::MessageTooLarge {
                size: 0,
                max_size: exo_ipc::MAX_INLINE_SIZE,
            },
            SerializeError::InvalidVersion(_)
            | SerializeError::InvalidType(_)
            | SerializeError::CorruptedData
            | SerializeError::Overflow => IpcError::DeserializationError,
        }
    }
}

/// Serveur IPC pour registry
///
/// Écoute les requêtes sur un canal MPSC et délègue au daemon.
pub struct IpcServer {
    /// Daemon gérant le registry
    daemon: RegistryDaemon,

    /// Récepteur de requêtes
    receiver: ReceiverMpsc,

    /// Émetteurs de réponses (un par client)
    response_senders: Vec<SenderMpsc>,

    /// Flag d'exécution
    running: AtomicBool,
}

impl IpcServer {
    /// Crée un nouveau serveur IPC
    ///
    /// # Arguments
    /// * `daemon` - Daemon registry à utiliser
    /// * `capacity` - Capacité du canal (doit être puissance de 2)
    pub fn new(daemon: RegistryDaemon, capacity: usize) -> IpcResult<Self> {
        let (_tx, rx) = mpsc(capacity)
            .map_err(|_| IpcError::InvalidCapacity(capacity))?;

        Ok(Self {
            daemon,
            receiver: rx,
            response_senders: Vec::new(),
            running: AtomicBool::new(false),
        })
    }

    /// Ajoute un sender de réponse pour un client
    pub fn add_client(&mut self, sender: SenderMpsc) {
        self.response_senders.push(sender);
    }

    /// Lance la boucle d'écoute (bloquant)
    ///
    /// Traite les requêtes jusqu'à ce que shutdown() soit appelé.
    pub fn run(&mut self) -> IpcResult<()> {
        self.running.store(true, Ordering::Release);

        while self.running.load(Ordering::Acquire) {
            match self.receiver.recv() {
                Ok(msg) => {
                    if let Err(_e) = self.handle_message(msg) {
                        // Log l'erreur mais continue
                        // TODO: Intégrer avec exo_logger
                        continue;
                    }
                }
                Err(RecvError::Disconnected) => {
                    // Canal fermé, on arrête
                    break;
                }
                Err(RecvError::Empty) => {
                    // Pas de message, on continue
                    continue;
                }
                Err(RecvError::Timeout) => {
                    // Timeout, on continue
                    continue;
                }
            }
        }

        Ok(())
    }

    /// Arrête le serveur
    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
    }

    /// Traite un message IPC
    fn handle_message(&mut self, msg: Message) -> IpcResult<()> {
        // Vérifie le type de message
        if msg.header.msg_type != MessageType::Data {
            return Err(IpcError::InvalidMessage);
        }

        // Récupère les données inline
        let data = msg.inline_data().ok_or(IpcError::InvalidMessage)?;

        // Désérialise la requête
        let request = RegistryRequest::deserialize_from(data)?;

        // Traite via le daemon
        let response = self.daemon.handle_request(request);

        // Sérialise la réponse
        let mut response_buf = Vec::new();
        response.serialize_into(&mut response_buf)
            .map_err(|_| IpcError::SerializationError)?;

        // Crée le message de réponse
        let response_msg = Message::with_inline_data(&response_buf, MessageType::Data)
            .map_err(|_| IpcError::MessageTooLarge {
                size: response_buf.len(),
                max_size: exo_ipc::MAX_INLINE_SIZE,
            })?;

        // Envoie aux clients (broadcast pour l'instant)
        // TODO: Routing intelligent basé sur message_id
        for sender in &self.response_senders {
            let _ = sender.send(response_msg.clone());
        }

        Ok(())
    }

    /// Retourne le nombre de requêtes traitées
    pub fn requests_processed(&self) -> u64 {
        self.daemon.requests_processed()
    }

    /// Retourne une référence au daemon
    pub fn daemon(&self) -> &RegistryDaemon {
        &self.daemon
    }

    /// Retourne une référence mutable au daemon
    pub fn daemon_mut(&mut self) -> &mut RegistryDaemon {
        &mut self.daemon
    }
}

/// Client IPC pour registry
///
/// Envoie des requêtes au serveur registry via IPC.
pub struct IpcClient {
    /// Émetteur de requêtes
    sender: SenderMpsc,

    /// Récepteur de réponses
    receiver: ReceiverMpsc,

    /// Timeout en millisecondes
    timeout_ms: u64,
}

impl IpcClient {
    /// Crée un nouveau client IPC
    ///
    /// # Arguments
    /// * `capacity` - Capacité du canal (doit être puissance de 2)
    pub fn new(capacity: usize) -> IpcResult<Self> {
        let (tx, rx) = mpsc(capacity)
            .map_err(|_| IpcError::InvalidCapacity(capacity))?;

        Ok(Self {
            sender: tx,
            receiver: rx,
            timeout_ms: 5000, // 5 secondes par défaut
        })
    }

    /// Définit le timeout
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Envoie une requête et attend la réponse
    fn send_request(&mut self, request: RegistryRequest) -> IpcResult<RegistryResponse> {
        // Sérialise la requête
        let mut buf = Vec::new();
        request.serialize_into(&mut buf)
            .map_err(|_| IpcError::SerializationError)?;

        // Crée le message IPC
        let msg = Message::with_inline_data(&buf, MessageType::Data)
            .map_err(|_| IpcError::MessageTooLarge {
                size: buf.len(),
                max_size: exo_ipc::MAX_INLINE_SIZE,
            })?;

        // Envoie
        self.sender.send(msg)
            .map_err(|_| IpcError::Disconnected)?;

        // Attend la réponse
        let response_msg = self.receiver.recv()
            .map_err(|e| match e {
                RecvError::Disconnected => IpcError::Disconnected,
                RecvError::Empty => IpcError::WouldBlock,
                RecvError::Timeout => IpcError::Timeout,
            })?;

        // Désérialise
        let response_data = response_msg.inline_data()
            .ok_or(IpcError::InvalidMessage)?;

        RegistryResponse::deserialize_from(response_data)
            .map_err(|_| IpcError::DeserializationError)
    }

    /// Enregistre un service
    pub fn register(&mut self, name: ServiceName, info: ServiceInfo) -> RegistryResult<()> {
        let request = RegistryRequest::register(name, info);
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::Ok => Ok(()),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Recherche un service
    pub fn lookup(&mut self, name: &ServiceName) -> RegistryResult<Option<ServiceInfo>> {
        let request = RegistryRequest::lookup(name.clone());
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::Found => Ok(response.service_info),
            ResponseType::NotFound => Ok(None),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Désenregistre un service
    pub fn unregister(&mut self, name: &ServiceName) -> RegistryResult<()> {
        let request = RegistryRequest::unregister(name.clone());
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::Ok => Ok(()),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Envoie un heartbeat
    pub fn heartbeat(&mut self, name: &ServiceName) -> RegistryResult<()> {
        let request = RegistryRequest::heartbeat(name.clone());
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::Ok => Ok(()),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Liste tous les services
    pub fn list(&mut self) -> RegistryResult<Vec<(ServiceName, ServiceInfo)>> {
        let request = RegistryRequest::list();
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::List => Ok(response.services),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Ping le serveur (health check)
    pub fn ping(&mut self) -> RegistryResult<()> {
        let request = RegistryRequest::ping();
        let response = self.send_request(request)
            .map_err(|_| RegistryError::StorageError("IPC error".into()))?;

        match response.response_type {
            ResponseType::Pong => Ok(()),
            ResponseType::Error => {
                let msg = response.error_message.unwrap_or_else(|| "Unknown error".into());
                Err(RegistryError::StorageError(msg))
            }
            _ => Err(RegistryError::StorageError("Unexpected response".into())),
        }
    }

    /// Retourne une référence au sender (pour passer au serveur)
    pub fn sender(&self) -> &SenderMpsc {
        &self.sender
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Registry;

    #[test]
    fn test_ipc_server_creation() {
        let registry = Box::new(Registry::new());
        let daemon = RegistryDaemon::with_registry(registry);
        let server = IpcServer::new(daemon, 64);
        assert!(server.is_ok());
    }

    #[test]
    fn test_ipc_client_creation() {
        let client = IpcClient::new(64);
        assert!(client.is_ok());
    }

    #[test]
    fn test_serialize_error_conversion() {
        let err = SerializeError::BufferTooSmall;
        let ipc_err: IpcError = err.into();
        match ipc_err {
            IpcError::MessageTooLarge { .. } => (),
            _ => panic!("Wrong conversion"),
        }

        let err = SerializeError::CorruptedData;
        let ipc_err: IpcError = err.into();
        match ipc_err {
            IpcError::DeserializationError => (),
            _ => panic!("Wrong conversion"),
        }
    }
}
