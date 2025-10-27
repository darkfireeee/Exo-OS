//! # Abstraction pour les Pilotes
//! 
//! Ce module fournit une interface unifiée pour les pilotes de périphériques.
//! Il permet d'abstraire les détails d'implémentation de chaque type de
//! périphérique tout en offrant des performances optimales.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::println;

/// Types de pilotes supportés
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverType {
    Block,
    Char,
    Network,
    USB,
    PCI,
    Unknown,
}

/// Interface commune pour tous les pilotes
pub trait Driver: Send + Sync {
    /// Retourne le type de pilote
    fn driver_type(&self) -> DriverType;
    
    /// Retourne le nom du pilote
    fn name(&self) -> &str;
    
    /// Initialise le pilote
    fn init(&mut self) -> Result<(), DriverError>;
    
    /// Arrête le pilote
    fn shutdown(&mut self) -> Result<(), DriverError>;
    
    /// Vérifie si le pilote est prêt
    fn is_ready(&self) -> bool;
}

/// Erreurs possibles pour les pilotes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    InitializationFailed,
    InvalidParameter,
    DeviceNotFound,
    OperationNotSupported,
    ResourceBusy,
    Timeout,
    HardwareError,
}

/// Gestionnaire de pilotes
pub struct DriverManager {
    drivers: BTreeMap<u32, Arc<Mutex<dyn Driver>>>,
    next_id: u32,
}

impl DriverManager {
    /// Crée un nouveau gestionnaire de pilotes
    pub fn new() -> Self {
        Self {
            drivers: BTreeMap::new(),
            next_id: 1,
        }
    }
    
    /// Enregistre un nouveau pilote
    pub fn register_driver(&mut self, driver: Arc<Mutex<dyn Driver>>) -> Result<u32, DriverError> {
        let id = self.next_id;
        self.next_id += 1;
        
        // Initialiser le pilote
        {
            let mut d = driver.lock();
            d.init()?;
        }
        
        self.drivers.insert(id, driver);
        Ok(id)
    }
    
    /// Désenregistre un pilote
    pub fn unregister_driver(&mut self, id: u32) -> Result<(), DriverError> {
        if let Some(driver) = self.drivers.remove(&id) {
            let mut d = driver.lock();
            d.shutdown()?;
            Ok(())
        } else {
            Err(DriverError::DeviceNotFound)
        }
    }
    
    /// Récupère un pilote par son ID
    pub fn get_driver(&self, id: u32) -> Option<Arc<Mutex<dyn Driver>>> {
        self.drivers.get(&id).cloned()
    }
    
    /// Récupère tous les pilotes d'un type spécifique
    pub fn get_drivers_by_type(&self, driver_type: DriverType) -> Vec<(u32, Arc<Mutex<dyn Driver>>)> {
        self.drivers
            .iter()
            .filter(|(_, driver)| {
                let d = driver.lock();
                d.driver_type() == driver_type
            })
            .map(|(id, driver)| (*id, driver.clone()))
            .collect()
    }
    
    /// Initialise tous les pilotes enregistrés
    pub fn init_all(&mut self) -> Result<(), DriverError> {
        for (_, driver) in &self.drivers {
            let mut d = driver.lock();
            d.init()?;
        }
        Ok(())
    }
    
    /// Arrête tous les pilotes enregistrés
    pub fn shutdown_all(&mut self) -> Result<(), DriverError> {
        for (_, driver) in &self.drivers {
            let mut d = driver.lock();
            d.shutdown()?;
        }
        Ok(())
    }
}

// Instance globale du gestionnaire de pilotes
lazy_static::lazy_static! {
    pub static ref DRIVER_MANAGER: Mutex<DriverManager> = Mutex::new(DriverManager::new());
}

/// Initialise le sous-système de pilotes
pub fn init() {
    println!("[DRIVERS] Initialisation du sous-système de pilotes...");
    
    // L'initialisation est différée jusqu'à ce que les pilotes spécifiques soient enregistrés
    
    println!("[DRIVERS] Sous-système de pilotes initialisé.");
}

// Inclure les implémentations des pilotes spécifiques
pub mod block;
pub mod serial;
pub mod vga_text;
