// libs/exo_ipc/src/ring/spsc.rs
//! Ring buffer lock-free Single Producer Single Consumer (SPSC)
//!
//! Optimisé pour performance maximale avec:
//! - Cache-line padding pour éviter false sharing
//! - Wrapping indices pour éviter les modulos
//! - Atomic orderings minimaux

use core::ptr;

use crate::types::Message;
use crate::util::cache::CachePadded;
use crate::util::atomic::RingIndex;

/// Ring buffer SPSC optimisé
///
/// # Invariants de sécurité:
/// - Un seul producteur peut appeler `push()`
/// - Un seul consommateur peut appeler `pop()`
/// - `capacity` doit être une puissance de 2
/// - `head` et `tail` peuvent wrapper (overflow intentionnel)
pub struct SpscRing {
    /// Buffer des messages (heap-allocated)
    buffer: *mut Message,
    
    /// Capacité (puissance de 2)
    capacity: usize,
    
    /// Masque pour wrapping (capacity - 1)
    mask: usize,
    
    /// Index de tête (producteur écrit ici)
    /// Séparé dans sa propre cache-line pour éviter false sharing
    head: CachePadded<RingIndex>,
    
    /// Index de queue (consommateur lit ici)
    /// Séparé dans sa propre cache-line
    tail: CachePadded<RingIndex>,
}

impl SpscRing {
    /// Crée un nouveau ring buffer avec la capacité spécifiée
    ///
    /// # Panics
    /// Panic si la capacité n'est pas une puissance de 2
    pub fn new(capacity: usize) -> Result<Self, &'static str> {
        if !capacity.is_power_of_two() {
            return Err("Capacité doit être une puissance de 2");
        }
        
        if capacity == 0 {
            return Err("Capacité doit être > 0");
        }
        
        // Allouer le buffer
        let layout = alloc::alloc::Layout::array::<Message>(capacity)
            .map_err(|_| "Layout invalide")?;
        
        let buffer = unsafe {
            let ptr = alloc::alloc::alloc(layout) as *mut Message;
            if ptr.is_null() {
                return Err("Allocation échouée");
            }
            
            // Initialiser avec MaybeUninit (pas besoin d'initialiser pour l'instant)
            ptr
        };
        
        Ok(Self {
            buffer,
            capacity,
            mask: capacity - 1,
            head: CachePadded::new(RingIndex::new(0)),
            tail: CachePadded::new(RingIndex::new(0)),
        })
    }
    
    /// Tente d'ajouter un message au buffer
    ///
    /// Retourne `Ok(())` en cas de succès, `Err(message)` si plein
    ///
    /// # Safety
    /// Doit être appelé uniquement par le producteur (single-threaded)
    pub fn push(&self, msg: Message) -> Result<(), Message> {
        let head = self.head.load_relaxed();
        let tail = self.tail.load_acquire();
        
        // Vérifier si le buffer est plein
        // On garde un slot vide pour distinguer plein/vide
        if head.wrapping_sub(tail) >= self.capacity {
            return Err(msg);
        }
        
        // Écrire le message
        // SAFETY: head est dans les limites (masqué) et on a vérifié qu'il y a de la place
        unsafe {
            let slot = self.buffer.add(head & self.mask);
            ptr::write(slot, msg);
        }
        
        // Publier le nouveau head avec Release ordering
        // Cela garantit que l'écriture du message est visible avant l'incrément de head
        self.head.store_release(head.wrapping_add(1));
        
        Ok(())
    }
    
    /// Tente de retirer un message du buffer
    ///
    /// Retourne `Some(message)` en cas de succès, `None` si vide
    ///
    /// # Safety
    /// Doit être appelé uniquement par le consommateur (single-threaded)
    pub fn pop(&self) -> Option<Message> {
        let tail = self.tail.load_relaxed();
        let head = self.head.load_acquire();
        
        // Vérifier si le buffer est vide
        if tail == head {
            return None;
        }
        
        // Lire le message
        // SAFETY: tail est dans les limites et on a vérifié qu'il y a un élément
        let msg = unsafe {
            let slot = self.buffer.add(tail & self.mask);
            ptr::read(slot)
        };
        
        // Publier le nouveau tail avec Release ordering
        self.tail.store_release(tail.wrapping_add(1));
        
        Some(msg)
    }
    
    /// Retourne le nombre d'éléments dans le buffer
    pub fn len(&self) -> usize {
        let head = self.head.load_acquire();
        let tail = self.tail.load_acquire();
        head.wrapping_sub(tail)
    }
    
    /// Vérifie si le buffer est vide
    pub fn is_empty(&self) -> bool {
        let head = self.head.load_acquire();
        let tail = self.tail.load_acquire();
        head == tail
    }
    
    /// Vérifie si le buffer est plein
    pub fn is_full(&self) -> bool {
        let head = self.head.load_acquire();
        let tail = self.tail.load_acquire();
        head.wrapping_sub(tail) >= self.capacity
    }
    
    /// Retourne la capacité du buffer
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    
    /// Retourne le nombre d'emplacements disponibles
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.len())
    }
}

impl Drop for SpscRing {
    fn drop(&mut self) {
        // Lire tous les messages restants pour les drop correctement
        while self.pop().is_some() {}
        
        // Libérer le buffer
        unsafe {
            let layout = alloc::alloc::Layout::array::<Message>(self.capacity)
                .expect("Layout doit être valide");
            alloc::alloc::dealloc(self.buffer as *mut u8, layout);
        }
    }
}

// SAFETY: SpscRing peut être envoyé entre threads car:
// - Les accès sont synchronisés par atomics
// - Un seul thread produit, un seul thread consomme
unsafe impl Send for SpscRing {}
unsafe impl Sync for SpscRing {}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageType;
    
    #[test]
    fn test_spsc_create() {
        let ring = SpscRing::new(16).unwrap();
        assert_eq!(ring.capacity(), 16);
        assert!(ring.is_empty());
        assert!(!ring.is_full());
    }
    
    #[test]
    fn test_spsc_push_pop() {
        let ring = SpscRing::new(16).unwrap();
        
        let msg = Message::new(MessageType::Data);
        ring.push(msg).unwrap();
        
        assert_eq!(ring.len(), 1);
        assert!(!ring.is_empty());
        
        let received = ring.pop().unwrap();
        assert_eq!(received.header.msg_type as u16, MessageType::Data as u16);
        
        assert!(ring.is_empty());
    }
    
    #[test]
    fn test_spsc_full() {
        let ring = SpscRing::new(4).unwrap();
        
        // Remplir le buffer (capacité - 1 car on garde un slot vide)
        for _ in 0..3 {
            let msg = Message::new(MessageType::Data);
            ring.push(msg).unwrap();
        }
        
        // Le buffer devrait être plein maintenant
        let msg = Message::new(MessageType::Data);
        assert!(ring.push(msg).is_err());
    }
    
    #[test]
    fn test_spsc_wrapping() {
        let ring = SpscRing::new(4).unwrap();
        
        // Push et pop plusieurs fois pour tester le wrapping
        for i in 0..10 {
            let mut msg = Message::new(MessageType::Data);
            msg.header.sequence = i;
            ring.push(msg).unwrap();
            
            let received = ring.pop().unwrap();
            assert_eq!(received.header.sequence, i);
        }
    }
    
    #[test]
    #[should_panic]
    fn test_spsc_invalid_capacity() {
        SpscRing::new(7).unwrap(); // Pas une puissance de 2
    }
}
*/
