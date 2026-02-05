// libs/exo_ipc/src/shm/pool.rs
//! Pool de messages pour réduire les allocations

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::types::Message;
use crate::util::cache::CachePadded;

/// Pool de messages pour réutilisation
///
/// Réduit les allocations en recyclant les messages
pub struct MessagePool {
    /// Pool de messages disponibles
    pool: Vec<Message>,
    
    /// Capacité maximale du pool
    max_capacity: usize,
    
    /// Statistiques: allocations
    allocations: CachePadded<AtomicUsize>,
    
    /// Statistiques: recyclages
    recycled: CachePadded<AtomicUsize>,
}

impl MessagePool {
    /// Crée un nouveau pool
    ///
    /// # Arguments
    /// * `initial_size` - Nombre de messages à pré-allouer
    /// * `max_capacity` - Capacité maximale du pool
    pub fn new(initial_size: usize, max_capacity: usize) -> Self {
        let mut pool = Vec::with_capacity(max_capacity);
        
        // Pré-allouer les messages
        for _ in 0..initial_size {
            pool.push(Message::new(crate::types::MessageType::Data));
        }
        
        Self {
            pool,
            max_capacity,
            allocations: CachePadded::new(AtomicUsize::new(0)),
            recycled: CachePadded::new(AtomicUsize::new(0)),
        }
    }
    
    /// Acquiert un message du pool ou en alloue un nouveau
    pub fn acquire(&mut self) -> Message {
        match self.pool.pop() {
            Some(msg) => {
                self.recycled.fetch_add(1, Ordering::Relaxed);
                msg
            }
            None => {
                self.allocations.fetch_add(1, Ordering::Relaxed);
                Message::new(crate::types::MessageType::Data)
            }
        }
    }
    
    /// Retourne un message au pool
    pub fn release(&mut self, msg: Message) {
        if self.pool.len() < self.max_capacity {
            self.pool.push(msg);
        }
        // Sinon, le message est droppé
    }
    
    /// Nombre de messages disponibles dans le pool
    pub fn available(&self) -> usize {
        self.pool.len()
    }
    
    /// Nombre total d'allocations
    pub fn allocation_count(&self) -> usize {
        self.allocations.load(Ordering::Relaxed)
    }
    
    /// Nombre total de recyclages
    pub fn recycle_count(&self) -> usize {
        self.recycled.load(Ordering::Relaxed)
    }
    
    /// Taux de recyclage (0.0 - 1.0)
    pub fn recycle_rate(&self) -> f32 {
        let allocs = self.allocation_count() as f32;
        let recyc = self.recycle_count() as f32;
        let total = allocs + recyc;
        
        if total > 0.0 {
            recyc / total
        } else {
            0.0
        }
    }
    
    /// Pré-remplit le pool avec des messages
    pub fn prefill(&mut self, count: usize) {
        let to_add = count.min(self.max_capacity - self.pool.len());
        
        for _ in 0..to_add {
            self.pool.push(Message::new(crate::types::MessageType::Data));
        }
    }
    
    /// Vide le pool
    pub fn clear(&mut self) {
        self.pool.clear();
    }
    
    /// Shrink le pool à une taille donnée
    pub fn shrink_to(&mut self, size: usize) {
        if self.pool.len() > size {
            self.pool.truncate(size);
            self.pool.shrink_to_fit();
        }
    }
}

impl Default for MessagePool {
    fn default() -> Self {
        Self::new(16, 256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageType;
    
    #[test]
    fn test_pool_acquire_release() {
        let mut pool = MessagePool::new(0, 16);
        
        let msg = pool.acquire();
        assert_eq!(pool.allocation_count(), 1);
        
        pool.release(msg);
        assert_eq!(pool.available(), 1);
        
        let msg2 = pool.acquire();
        assert_eq!(pool.recycle_count(), 1);
        
        pool.release(msg2);
    }
    
    #[test]
    fn test_pool_prefill() {
        let mut pool = MessagePool::new(0, 32);
        assert_eq!(pool.available(), 0);
        
        pool.prefill(10);
        assert_eq!(pool.available(), 10);
    }
    
    #[test]
    fn test_pool_max_capacity() {
        let mut pool = MessagePool::new(0, 4);
        
        for _ in 0..10 {
            let msg = pool.acquire();
            pool.release(msg);
        }
        
        // Ne devrait pas dépasser la capacité max
        assert!(pool.available() <= 4);
    }
    
    #[test]
    fn test_pool_recycle_rate() {
        let mut pool = MessagePool::new(2, 16);
        
        // Acquérir depuis le pool (recyclage)
        let msg1 = pool.acquire();
        let msg2 = pool.acquire();
        
        // Acquérir en allouant
        let msg3 = pool.acquire();
        
        assert_eq!(pool.allocation_count(), 1);
        assert_eq!(pool.recycle_count(), 2);
        assert!(pool.recycle_rate() > 0.6); // 2/3 = 0.666...
    }
}
