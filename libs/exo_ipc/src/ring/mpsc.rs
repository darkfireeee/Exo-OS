// libs/exo_ipc/src/ring/mpsc.rs
//! Ring buffer lock-free Multi Producer Single Consumer (MPSC)
//!
//! Permet à plusieurs producteurs d'écrire dans le même buffer
//! avec un seul consommateur.

use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::types::Message;
use crate::util::cache::CachePadded;
use crate::util::atomic::Backoff;

/// Ring buffer MPSC lock-free
///
/// # Invariants:
/// - Plusieurs producteurs peuvent appeler `push()` simultanément
/// - Un seul consommateur peut appeler `pop()`
/// - Utilise CAS (Compare-And-Swap) pour la synchronisation multi-producteur
pub struct MpscRing {
    /// Buffer des messages
    buffer: *mut Message,
    
    /// Capacité (puissance de 2)
    capacity: usize,
    
    /// Masque pour wrapping
    mask: usize,
    
    /// Index de tête (producteurs écrivent ici)
    /// Nécessite CAS pour synchronisation multi-producteur
    head: CachePadded<AtomicUsize>,
    
    /// Index de queue (consommateur lit ici)
    tail: CachePadded<AtomicUsize>,
}

impl MpscRing {
    /// Crée un nouveau ring buffer MPSC
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if !capacity.is_power_of_two() {
            return Err("Capacité doit être une puissance de 2");
        }
        
        if capacity == 0 {
            return Err("Capacité doit être > 0");
        }
        
        let layout = alloc::alloc::Layout::array::<Message>(capacity)
            .map_err(|_| "Layout invalide")?;
        
        let buffer = unsafe {
            let ptr = alloc::alloc::alloc(layout) as *mut Message;
            if ptr.is_null() {
                return Err("Allocation échouée");
            }
            ptr
        };
        
        Ok(Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
        })
    }
    
    /// Tente d'ajouter un message (thread-safe pour multiple producteurs)
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        let mut backoff = Backoff::new();
        
        loop {
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            
            // Vérifier si plein
            if head.wrapping_sub(tail) >= self.capacity {
                return Err(msg);
            }
            
            // Tenter de réserver un slot avec CAS
            match self.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Slot réservé, écrire le message
                    unsafe {
                        let slot = self.buffer.add(head & self.mask);
                        ptr::write(slot, msg);
                    }
                    return Ok(());
                }
                Err(_) => {
                    // CAS a échoué, backoff et retry
                    backoff.spin();
                    continue;
                }
            }
        }
    }
    
    /// Retire un message (single consumer seulement)
    pub fn pop(&self) -> Option<Message> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        
        if tail == head {
            return None;
        }
        
        let msg = unsafe {
            let slot = self.buffer.add(tail & self.mask);
            ptr::read(slot)
        };
        
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        
        Some(msg)
    }
    
    /// Nombre d'éléments
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }
    
    /// Vérifie si vide
    pub fn is_empty(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }
    
    /// Vérifie si plein
    pub fn is_full(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail) >= self.capacity
    }
    
    /// Capacité
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Emplacements disponibles
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.len())
    }
}

impl Drop for MpscRing {
    fn drop(&mut self) {
        while self.pop().is_some() {}
        
        unsafe {
            let layout = alloc::alloc::Layout::array::<Message>(self.capacity)
                .expect("Layout doit être valide");
            alloc::alloc::dealloc(self.buffer as *mut u8, layout);
        }
    }
}

unsafe impl Send for MpscRing {}
unsafe impl Sync for MpscRing {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageType;
    
    #[test]
    fn test_mpsc_basic() {
        let ring = MpscRing::new(16).unwrap();
        
        let msg = Message::new(MessageType::Data);
        ring.push(msg).unwrap();
        
        let received = ring.pop().unwrap();
        assert_eq!(received.header.msg_type as u16, MessageType::Data as u16);
    }
    
    #[test]
    fn test_mpsc_multiple_producers() {
        use alloc::sync::Arc;
        
        let ring = Arc::new(MpscRing::new(64).unwrap());
        
        // Simuler plusieurs producteurs (dans les tests, on ne peut pas vraiment
        // créer plusieurs threads, mais on peut tester la logique)
        for i in 0..10 {
            let mut msg = Message::new(MessageType::Data);
            msg.header.sequence = i;
            ring.push(msg).unwrap();
        }
        
        // Vérifier qu'on peut récupérer tous les messages
        for _ in 0..10 {
            assert!(ring.pop().is_some());
        }
        
        assert!(ring.is_empty());
    }
}
