//! # Backend Management
//! 
//! Backend server pool management

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::SpinLock;
use super::algorithms::Backend;

/// Backend pool
pub struct BackendPool {
    backends: SpinLock<Vec<Backend>>,
    next_id: core::sync::atomic::AtomicU32,
}

impl BackendPool {
    pub fn new() -> Self {
        Self {
            backends: SpinLock::new(Vec::new()),
            next_id: core::sync::atomic::AtomicU32::new(1),
        }
    }
    
    /// Add backend
    pub fn add(&self, address: [u8; 4], port: u16, weight: u32) -> u32 {
        let id = self.next_id.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let backend = Backend::new(id, address, port, weight);
        
        let mut backends = self.backends.lock();
        backends.push(backend);
        
        id
    }
    
    /// Remove backend
    pub fn remove(&self, id: u32) -> bool {
        let mut backends = self.backends.lock();
        if let Some(pos) = backends.iter().position(|b| b.id == id) {
            backends.remove(pos);
            true
        } else {
            false
        }
    }
    
    /// Get backend
    pub fn get(&self, id: u32) -> Option<Backend> {
        let backends = self.backends.lock();
        backends.iter().find(|b| b.id == id).cloned()
    }
    
    /// List all backends
    pub fn list(&self) -> Vec<Backend> {
        self.backends.lock().clone()
    }
    
    /// Mark backend healthy/unhealthy
    pub fn set_health(&self, id: u32, healthy: bool) -> bool {
        let mut backends = self.backends.lock();
        if let Some(backend) = backends.iter_mut().find(|b| b.id == id) {
            backend.healthy = healthy;
            true
        } else {
            false
        }
    }
}
