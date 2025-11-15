//! # Adaptive Block Driver - Driver Disque Adaptatif
//! 
//! Implémentation d'un driver de disque utilisant le système AdaptiveDriver
//! pour optimiser automatiquement entre polling et interrupts.

use super::adaptive_driver::*;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;
use alloc::collections::VecDeque;

/// Taille d'un bloc (512 bytes standard)
const BLOCK_SIZE: usize = 512;

/// Nombre max de requêtes dans le batch
const MAX_BATCH_SIZE: usize = 32;

/// Requête de lecture/écriture
#[derive(Debug, Clone)]
pub struct BlockRequest {
    /// Numéro du bloc
    pub block_number: u64,
    
    /// Lecture (true) ou écriture (false)
    pub is_read: bool,
    
    /// Buffer de données
    pub buffer: [u8; BLOCK_SIZE],
    
    /// Timestamp de la requête (pour mesure latence)
    pub timestamp_tsc: u64,
}

impl BlockRequest {
    pub fn new_read(block_number: u64) -> Self {
        Self {
            block_number,
            is_read: true,
            buffer: [0u8; BLOCK_SIZE],
            timestamp_tsc: rdtsc(),
        }
    }
    
    pub fn new_write(block_number: u64, data: [u8; BLOCK_SIZE]) -> Self {
        Self {
            block_number,
            is_read: false,
            buffer: data,
            timestamp_tsc: rdtsc(),
        }
    }
}

/// Driver de disque adaptatif
pub struct AdaptiveBlockDriver {
    /// Nom du driver
    name: &'static str,
    
    /// Controller adaptatif
    controller: Mutex<AdaptiveController>,
    
    /// Queue de requêtes en attente (pour mode batch)
    request_queue: Mutex<VecDeque<BlockRequest>>,
    
    /// Flag indiquant si le hardware a des données prêtes
    hardware_ready: AtomicBool,
    
    /// Statistiques
    stats: DriverStats,
    
    /// Compteur de requêtes servies
    requests_served: AtomicUsize,
}

impl AdaptiveBlockDriver {
    /// Crée un nouveau driver
    pub fn new(name: &'static str, tsc_frequency_mhz: u64) -> Self {
        Self {
            name,
            controller: Mutex::new(AdaptiveController::new(tsc_frequency_mhz)),
            request_queue: Mutex::new(VecDeque::with_capacity(MAX_BATCH_SIZE)),
            hardware_ready: AtomicBool::new(false),
            stats: DriverStats::default(),
            requests_served: AtomicUsize::new(0),
        }
    }
    
    /// Soumet une requête
    pub fn submit_request(&mut self, request: BlockRequest) -> Result<(), &'static str> {
        let start_tsc = rdtsc();
        
        // Enregistrer l'opération dans le controller
        let mode = {
            let mut controller = self.controller.lock();
            controller.record_operation()
        };
        
        // Traiter selon le mode
        let result = match mode {
            DriverMode::Interrupt => {
                self.submit_interrupt_mode(request)
            }
            DriverMode::Polling => {
                self.submit_polling_mode(request)
            }
            DriverMode::Hybrid => {
                self.submit_hybrid_mode(request)
            }
            DriverMode::Batch => {
                self.submit_batch_mode(request)
            }
        };
        
        // Mesurer cycles
        let elapsed = rdtsc() - start_tsc;
        {
            let mut controller = self.controller.lock();
            controller.record_cycles(elapsed);
        }
        
        if result.is_ok() {
            self.requests_served.fetch_add(1, Ordering::Relaxed);
        }
        
        result
    }
    
    /// Traite une requête en mode Interrupt
    fn submit_interrupt_mode(&mut self, request: BlockRequest) -> Result<(), &'static str> {
        // Envoyer commande au hardware
        self.send_to_hardware(&request)?;
        
        // Attendre interrupt
        self.wait_interrupt()?;
        
        Ok(())
    }
    
    /// Traite une requête en mode Polling
    fn submit_polling_mode(&mut self, request: BlockRequest) -> Result<(), &'static str> {
        // Envoyer commande au hardware
        self.send_to_hardware(&request)?;
        
        // Polling jusqu'à completion
        loop {
            if self.poll_status()? {
                break;
            }
        }
        
        Ok(())
    }
    
    /// Traite une requête en mode Hybrid
    fn submit_hybrid_mode(&mut self, request: BlockRequest) -> Result<(), &'static str> {
        // Envoyer commande au hardware
        self.send_to_hardware(&request)?;
        
        // Poll court puis interrupt
        self.hybrid_wait()
    }
    
    /// Traite une requête en mode Batch
    fn submit_batch_mode(&mut self, request: BlockRequest) -> Result<(), &'static str> {
        // Ajouter à la queue
        let mut queue = self.request_queue.lock();
        queue.push_back(request);
        
        // Si queue pleine ou timeout, flush
        if queue.len() >= MAX_BATCH_SIZE {
            drop(queue);
            self.flush_batch()?;
        }
        
        Ok(())
    }
    
    /// Flush le batch de requêtes
    fn flush_batch(&mut self) -> Result<(), &'static str> {
        let batch_size = {
            let mut queue = self.request_queue.lock();
            let size = queue.len();
            
            if size == 0 {
                return Ok(());
            }
            
            // Coalescence: trier par numéro de bloc pour accès séquentiel
            let mut batch: alloc::vec::Vec<_> = queue.drain(..).collect();
            batch.sort_by_key(|req| req.block_number);
            
            // Envoyer toutes les requêtes
            for request in batch.iter() {
                self.send_to_hardware(request)?;
            }
            
            size
        }; // Le MutexGuard est dropped ici
        
        // Attendre completion de toutes
        for _ in 0..batch_size {
            self.wait_interrupt()?;
        }
        
        Ok(())
    }
    
    /// Simule l'envoi au hardware
    fn send_to_hardware(&self, _request: &BlockRequest) -> Result<(), &'static str> {
        // Simulation: marquer hardware comme occupé
        self.hardware_ready.store(false, Ordering::Release);
        
        // Dans un vrai driver:
        // - Écrire registres hardware (commande, adresse, etc.)
        // - Démarrer DMA si disponible
        
        Ok(())
    }
    
    /// Simule la completion hardware (normalement appelé par interrupt handler)
    pub fn simulate_completion(&self) {
        self.hardware_ready.store(true, Ordering::Release);
    }
    
    /// Retourne le nombre de requêtes servies
    pub fn requests_served(&self) -> usize {
        self.requests_served.load(Ordering::Relaxed)
    }
    
    /// Retourne les statistiques du controller
    pub fn controller_stats(&self) -> DriverStats {
        self.controller.lock().stats().clone()
    }
}

impl AdaptiveDriver for AdaptiveBlockDriver {
    fn name(&self) -> &str {
        self.name
    }
    
    fn wait_interrupt(&mut self) -> Result<(), &'static str> {
        // Dans un vrai kernel:
        // - Activer IRQ pour le disque
        // - Bloquer thread courant
        // - Réveillé par interrupt handler
        
        // Simulation: busy wait jusqu'à hardware_ready
        while !self.hardware_ready.load(Ordering::Acquire) {
            // En vrai: hlt ou équivalent
            core::hint::spin_loop();
        }
        
        Ok(())
    }
    
    fn poll_status(&mut self) -> Result<bool, &'static str> {
        // Vérifier statut hardware
        Ok(self.hardware_ready.load(Ordering::Acquire))
    }
    
    fn batch_operation(&mut self, batch_size: usize) -> Result<usize, &'static str> {
        let queue = self.request_queue.lock();
        let actual_size = queue.len().min(batch_size);
        drop(queue);
        
        if actual_size > 0 {
            self.flush_batch()?;
        }
        
        Ok(actual_size)
    }
    
    fn current_mode(&self) -> DriverMode {
        self.controller.lock().current_mode()
    }
    
    fn set_mode(&mut self, mode: DriverMode) {
        self.controller.lock().force_mode(mode);
    }
    
    fn stats(&self) -> &DriverStats {
        &self.stats
    }
}

/// Lit le compteur TSC
#[cfg(all(target_arch = "x86_64", not(target_os = "windows")))]
fn rdtsc() -> u64 {
    unsafe {
        let mut low: u32;
        let mut high: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nostack, nomem)
        );
        ((high as u64) << 32) | (low as u64)
    }
}

#[cfg(not(all(target_arch = "x86_64", not(target_os = "windows"))))]
fn rdtsc() -> u64 {
    static mut COUNTER: u64 = 0;
    unsafe {
        COUNTER += 100;
        COUNTER
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_block_request_new() {
        let req = BlockRequest::new_read(42);
        assert_eq!(req.block_number, 42);
        assert!(req.is_read);
        assert_eq!(req.buffer.len(), BLOCK_SIZE);
    }
    
    #[test]
    fn test_adaptive_block_driver_init() {
        let driver = AdaptiveBlockDriver::new("test_disk", 2000);
        assert_eq!(driver.name(), "test_disk");
        assert_eq!(driver.current_mode(), DriverMode::Interrupt);
    }
    
    #[test]
    fn test_submit_polling_mode() {
        let mut driver = AdaptiveBlockDriver::new("test_disk", 2000);
        driver.set_mode(DriverMode::Polling);
        
        let request = BlockRequest::new_read(0);
        
        // Simuler completion dans un autre "thread"
        driver.simulate_completion();
        
        let result = driver.submit_request(request);
        assert!(result.is_ok());
        assert_eq!(driver.requests_served(), 1);
    }
    
    #[test]
    fn test_batch_accumulation() {
        let mut driver = AdaptiveBlockDriver::new("test_disk", 2000);
        driver.set_mode(DriverMode::Batch);
        
        // Soumettre plusieurs requêtes
        for i in 0..10 {
            let request = BlockRequest::new_read(i);
            driver.submit_request(request).ok();
        }
        
        // Vérifier que la queue contient les requêtes
        let queue_len = driver.request_queue.lock().len();
        assert_eq!(queue_len, 10);
    }
    
    #[test]
    fn test_batch_flush_on_full() {
        let mut driver = AdaptiveBlockDriver::new("test_disk", 2000);
        driver.set_mode(DriverMode::Batch);
        
        // Soumettre MAX_BATCH_SIZE requêtes
        for i in 0..MAX_BATCH_SIZE {
            let request = BlockRequest::new_read(i as u64);
            driver.simulate_completion(); // Simuler completion pour chaque
            driver.submit_request(request).ok();
        }
        
        // Queue devrait être vidée (flush automatique)
        let queue_len = driver.request_queue.lock().len();
        assert_eq!(queue_len, 0);
    }
}
