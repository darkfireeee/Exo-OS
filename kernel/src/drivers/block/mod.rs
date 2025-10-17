//! # Interface pour les Périphériques Bloc
//! 
//! Ce module définit l'interface pour les périphériques de stockage bloc
//! comme les disques durs, SSD, etc. Il offre des opérations de lecture/écriture
//! optimisées avec support pour les opérations asynchrones.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::Mutex;
use crate::drivers::{Driver, DriverError, DriverType};

/// Taille d'un secteur standard (en octets)
pub const SECTOR_SIZE: u64 = 512;

/// Types de périphériques bloc
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceType {
    HardDisk,
    SSD,
    Floppy,
    Optical,
    RAMDisk,
    Virtual,
}

/// Opérations possibles sur un périphérique bloc
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOperation {
    Read,
    Write,
    Flush,
    Trim,
}

/// Requête d'opération sur un périphérique bloc
#[derive(Debug)]
pub struct BlockRequest {
    /// Type d'opération
    pub operation: BlockOperation,
    /// Numéro du secteur de départ
    pub sector: u64,
    /// Nombre de secteurs
    pub count: u64,
    /// Pointeur vers les données
    pub data: *mut u8,
    /// Fonction de rappel pour la complétion asynchrone
    pub callback: Option<Box<dyn FnOnce(Result<(), BlockError>) + Send>>,
}

/// Erreurs spécifiques aux périphériques bloc
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    InvalidSector,
    InvalidCount,
    DeviceError,
    Timeout,
    WriteProtected,
    MediaError,
}

/// Interface pour les périphériques bloc
pub trait BlockDevice: Driver {
    /// Retourne le type de périphérique bloc
    fn device_type(&self) -> BlockDeviceType;
    
    /// Retourne la taille du périphérique en secteurs
    fn size_in_sectors(&self) -> u64;
    
    /// Retourne la taille du secteur en octets
    fn sector_size(&self) -> u64;
    
    /// Vérifie si le périphérique supporte les opérations asynchrones
    fn supports_async(&self) -> bool;
    
    /// Lit des secteurs depuis le périphérique
    fn read_sectors(&mut self, sector: u64, count: u64, data: *mut u8) -> Result<(), BlockError>;
    
    /// Écrit des secteurs vers le périphérique
    fn write_sectors(&mut self, sector: u64, count: u64, data: *const u8) -> Result<(), BlockError>;
    
    /// Vide les caches du périphérique
    fn flush(&mut self) -> Result<(), BlockError>;
    
    /// Soumet une requête asynchrone
    fn submit_async_request(&mut self, request: BlockRequest) -> Result<(), BlockError>;
    
    /// Traite les requêtes asynchrones en attente
    fn process_async_requests(&mut self);
}

/// Pilote de périphérique bloc générique
pub struct GenericBlockDevice {
    /// Nom du périphérique
    name: &'static str,
    /// Type de périphérique
    device_type: BlockDeviceType,
    /// Taille en secteurs
    size_sectors: u64,
    /// Taille des secteurs
    sector_size: u64,
    /// Support des opérations asynchrones
    async_support: bool,
    /// File d'attente des requêtes asynchrones
    async_queue: Mutex<VecDeque<BlockRequest>>,
    /// État d'initialisation
    initialized: bool,
}

impl GenericBlockDevice {
    /// Crée un nouveau périphérique bloc générique
    pub fn new(
        name: &'static str,
        device_type: BlockDeviceType,
        size_sectors: u64,
        sector_size: u64,
        async_support: bool,
    ) -> Self {
        Self {
            name,
            device_type,
            size_sectors,
            sector_size,
            async_support,
            async_queue: Mutex::new(VecDeque::new()),
            initialized: false,
        }
    }
}

impl Driver for GenericBlockDevice {
    fn driver_type(&self) -> DriverType {
        DriverType::Block
    }
    
    fn name(&self) -> &str {
        self.name
    }
    
    fn init(&mut self) -> Result<(), DriverError> {
        if self.initialized {
            return Ok(());
        }
        
        // Initialisation spécifique au périphérique
        // Pour un périphérique générique, il n'y a rien de spécial à faire
        
        self.initialized = true;
        Ok(())
    }
    
    fn shutdown(&mut self) -> Result<(), DriverError> {
        if !self.initialized {
            return Ok(());
        }
        
        // Arrêt spécifique au périphérique
        // Pour un périphérique générique, il n'y a rien de spécial à faire
        
        self.initialized = false;
        Ok(())
    }
    
    fn is_ready(&self) -> bool {
        self.initialized
    }
}

impl BlockDevice for GenericBlockDevice {
    fn device_type(&self) -> BlockDeviceType {
        self.device_type
    }
    
    fn size_in_sectors(&self) -> u64 {
        self.size_sectors
    }
    
    fn sector_size(&self) -> u64 {
        self.sector_size
    }
    
    fn supports_async(&self) -> bool {
        self.async_support
    }
    
    fn read_sectors(&mut self, sector: u64, count: u64, data: *mut u8) -> Result<(), BlockError> {
        if !self.initialized {
            return Err(BlockError::DeviceError);
        }
        
        if sector >= self.size_sectors {
            return Err(BlockError::InvalidSector);
        }
        
        if sector + count > self.size_sectors {
            return Err(BlockError::InvalidCount);
        }
        
        // Pour un périphérique générique, nous ne faisons rien
        // Dans une implémentation réelle, nous interagirions avec le matériel
        
        Ok(())
    }
    
    fn write_sectors(&mut self, sector: u64, count: u64, data: *const u8) -> Result<(), BlockError> {
        if !self.initialized {
            return Err(BlockError::DeviceError);
        }
        
        if sector >= self.size_sectors {
            return Err(BlockError::InvalidSector);
        }
        
        if sector + count > self.size_sectors {
            return Err(BlockError::InvalidCount);
        }
        
        // Pour un périphérique générique, nous ne faisons rien
        // Dans une implémentation réelle, nous interagirions avec le matériel
        
        Ok(())
    }
    
    fn flush(&mut self) -> Result<(), BlockError> {
        if !self.initialized {
            return Err(BlockError::DeviceError);
        }
        
        // Pour un périphérique générique, il n'y a rien à faire
        // Dans une implémentation réelle, nous viderions les caches
        
        Ok(())
    }
    
    fn submit_async_request(&mut self, request: BlockRequest) -> Result<(), BlockError> {
        if !self.async_support {
            return Err(BlockError::DeviceError);
        }
        
        if !self.initialized {
            return Err(BlockError::DeviceError);
        }
        
        // Ajouter la requête à la file d'attente
        let mut queue = self.async_queue.lock();
        queue.push_back(request);
        
        Ok(())
    }
    
    fn process_async_requests(&mut self) {
        if !self.async_support || !self.initialized {
            return;
        }
        
        // Traiter les requêtes en attente
        let mut queue = self.async_queue.lock();
        while let Some(request) = queue.pop_front() {
            let result = match request.operation {
                BlockOperation::Read => {
                    self.read_sectors(request.sector, request.count, request.data)
                }
                BlockOperation::Write => {
                    self.write_sectors(request.sector, request.count, request.data)
                }
                BlockOperation::Flush => {
                    self.flush()
                }
                BlockOperation::Trim => {
                    // Non implémenté pour un périphérique générique
                    Err(BlockError::OperationNotSupported)
                }
            };
            
            // Appeler la fonction de rappel si elle existe
            if let Some(callback) = request.callback {
                callback(result);
            }
        }
    }
}

/// Enregistre un nouveau périphérique bloc
pub fn register_block_device(device: Arc<Mutex<dyn BlockDevice>>) -> Result<u32, DriverError> {
    crate::drivers::DRIVER_MANAGER.lock().register_driver(device)
}

/// Récupère un périphérique bloc par son ID
pub fn get_block_device(id: u32) -> Option<Arc<Mutex<dyn BlockDevice>>> {
    if let Some(driver) = crate::drivers::DRIVER_MANAGER.lock().get_driver(id) {
        // Vérifier que le pilote est bien un périphérique bloc
        let d = driver.lock();
        if d.driver_type() == DriverType::Block {
            // Le downcast n'est pas possible directement avec les traits objets,
            // donc nous retournons le pilote générique
            // Le code appelant devra faire la vérification appropriée
            Some(driver)
        } else {
            None
        }
    } else {
        None
    }
}

/// Récupère tous les périphériques bloc
pub fn get_all_block_devices() -> Vec<(u32, Arc<Mutex<dyn BlockDevice>>)> {
    let mut result = Vec::new();
    
    let drivers = crate::drivers::DRIVER_MANAGER.lock();
    let block_drivers = drivers.get_drivers_by_type(DriverType::Block);
    
    for (id, driver) in block_drivers {
        // Même remarque que pour get_block_device
        result.push((id, driver));
    }
    
    result
}

/// Traite les requêtes asynchrones pour tous les périphériques bloc
pub fn process_all_async_requests() {
    let drivers = crate::drivers::DRIVER_MANAGER.lock();
    let block_drivers = drivers.get_drivers_by_type(DriverType::Block);
    
    for (_, driver) in block_drivers {
        // Tenter de traiter les requêtes asynchrones
        // Note: Le downcast n'est pas possible avec les traits objets,
        // donc cette fonction est surtout indicative
        // Dans une implémentation réelle, nous aurions besoin d'un mécanisme
        // pour identifier les périphériques bloc spécifiquement
    }
}