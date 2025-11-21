//! # Service Registry - Enregistrement et Gestion des Services
//!
//! Permet aux services de s'enregistrer auprès d'init et de communiquer
//! leur état.

use alloc::string::String;
use exo_types::{ErrorCode, ExoError, Result};
use log::{debug, info};

use crate::service::Service;

/// Handle retourné après enregistrement d'un service
#[derive(Debug, Clone, Copy)]
pub struct ServiceHandle {
    /// ID unique du service
    pub id: u64,
}

/// Registry central des services
pub struct ServiceRegistry;

impl ServiceRegistry {
    /// Enregistre un service auprès du gestionnaire (init)
    ///
    /// # Arguments
    /// - `service` - Service à enregistrer (doit implémenter Service trait)
    ///
    /// # Returns
    /// Handle permettant les interactions futures avec le gestionnaire
    pub fn register<S: Service>(service: S) -> Result<ServiceHandle> {
        let name = service.name();
        info!("Registering service: {}", name);
        
        // TODO: Envoyer message IPC à init pour enregistrement
        // Format du message:
        // RegisterService {
        //     name: String,
        //     capabilities: ServiceCapabilities,
        //     dependencies: Vec<String>,
        // }
        
        debug!("Service {} registered successfully", name);
        
        // Pour l'instant, retourner un handle factice
        Ok(ServiceHandle { id: 1 })
    }
    
    /// Notifie le gestionnaire que le service est prêt
    ///
    /// Appelé après que start() ait réussi
    pub fn notify_ready(handle: ServiceHandle) -> Result<()> {
        debug!("Service {} ready", handle.id);
        
        // TODO: Envoyer message IPC à init
        // ReadyNotification { service_id: u64 }
        
        Ok(())
    }
    
    /// Envoie un heartbeat au gestionnaire
    ///
    /// Permet au gestionnaire de savoir que le service est toujours actif
    pub fn heartbeat(handle: ServiceHandle) -> Result<()> {
        debug!("Heartbeat from service {}", handle.id);
        
        // TODO: Envoyer message IPC à init
        // Heartbeat { service_id: u64, timestamp: u64 }
        
        Ok(())
    }
    
    /// Notifie le gestionnaire d'un changement d'état
    ///
    /// Utilisé quand le service détecte un problème (devient Degraded ou Unhealthy)
    pub fn notify_status_change(handle: ServiceHandle, status: crate::service::HealthStatus) -> Result<()> {
        info!("Service {} status changed to {:?}", handle.id, status);
        
        // TODO: Envoyer message IPC à init
        // StatusChange { service_id: u64, new_status: HealthStatus }
        
        Ok(())
    }
}
