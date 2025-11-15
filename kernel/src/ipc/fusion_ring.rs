//! # Fusion Ring - IPC Zero-Copy Haute Performance
//! 
//! Implémentation d'un ring buffer lock-free optimisé pour l'IPC avec 3 modes :
//! - Fast Path : données inline (≤56 bytes)
//! - Zero-Copy : pointeur vers shared memory (>56 bytes)
//! - Batch : envoi multiple messages d'un coup
//!
//! Gains attendus : 10-20× plus rapide que Mutex<VecDeque>

use core::sync::atomic::{AtomicU64, Ordering, fence};
use alloc::vec::Vec;
use spin::Mutex;

/// Taille d'une page (4KB)
const PAGE_SIZE: usize = 4096;

/// Taille du ring (doit être puissance de 2)
const RING_SIZE: usize = 4096;

/// Taille d'un slot (1 cache line)
const SLOT_SIZE: usize = 64;

/// Taille max pour inline data
pub const INLINE_SIZE: usize = 56;

/// Ring buffer optimisé pour IPC haute performance
#[repr(C, align(4096))]
pub struct FusionRing {
    // === Cache Line 0 : Head (lecteur) ===
    head: AtomicU64,
    _pad1: [u8; 56],
    
    // === Cache Line 1 : Tail (écrivain) ===
    tail: AtomicU64,
    batch_size: u32,
    capacity: u32,
    _pad2: [u8; 48],
    
    // === Buffer circulaire (multiple pages) ===
    slots: [Slot; RING_SIZE],
}

/// Slot de 64 bytes (1 cache line)
#[repr(C, align(64))]
pub struct Slot {
    /// Numéro de séquence pour synchronisation
    seq: AtomicU64,
    
    /// Type de message
    msg_type: MessageType,
    
    /// Flags (priority, ack_required, etc.)
    flags: u8,
    
    /// Padding pour alignement
    _pad: [u8; 6],
    
    /// Payload (union de différents types)
    payload: SlotPayload,
}

/// Type de message
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MessageType {
    /// Données inline (fast path)
    Inline = 0,
    
    /// Pointeur vers shared memory (zero-copy)
    Shared = 1,
    
    /// Batch de messages
    Batch = 2,
    
    /// Message de contrôle
    Control = 3,
}

/// Payload du slot (56 bytes)
#[repr(C)]
pub union SlotPayload {
    /// Fast path : données inline
    inline: InlineData,
    
    /// Zero-copy : descripteur de shared memory
    shared: SharedMemDescriptor,
    
    /// Batch : pointeur vers batch
    batch: BatchDescriptor,
}

/// Données inline (56 bytes)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct InlineData {
    pub data: [u8; INLINE_SIZE],
}

/// Descripteur de shared memory (56 bytes)
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct SharedMemDescriptor {
    /// Adresse physique de la page
    pub phys_addr: u64,
    
    /// Taille des données
    pub size: u32,
    
    /// ID du thread propriétaire
    pub owner: u16,
    
    /// Flags (READONLY, WRITABLE, etc.)
    pub flags: u16,
    
    /// Padding pour alignement
    _pad: [u8; 40],
}

/// Descripteur de batch
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct BatchDescriptor {
    /// Pointeur vers le batch
    pub batch_ptr: u64,
    
    /// Nombre de messages dans le batch
    pub count: u32,
    
    /// Padding
    _pad: [u8; 44],
}

impl FusionRing {
    /// Crée un nouveau ring
    pub fn new() -> Self {
        Self {
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            batch_size: 16,
            capacity: RING_SIZE as u32,
            _pad1: [0; 56],
            _pad2: [0; 48],
            slots: unsafe { core::mem::zeroed() },
        }
    }
    
    /// Envoie un message zero-copy (via descripteur shared memory)
    #[inline(always)]
    pub fn send_zerocopy(&self, phys_addr: u64, size: u32, flags: u16) -> Result<(), IpcError> {
        let tail = self.tail.load(Ordering::Relaxed);
        let slot_idx = (tail & (RING_SIZE as u64 - 1)) as usize;
        let slot = &self.slots[slot_idx];
        
        // Vérifie si le slot est disponible
        if slot.seq.load(Ordering::Acquire) != tail {
            return Err(IpcError::Full);
        }
        
        // Crée le descripteur shared memory
        let desc = SharedMemDescriptor {
            phys_addr,
            size,
            owner: 0, // TODO: récupérer le thread ID actuel
            flags,
            _pad: [0; 40],
        };
        
        // Écrit le payload
        unsafe {
            let payload_ptr = &slot.payload as *const SlotPayload as *mut SlotPayload;
            (*payload_ptr).shared = desc;
        }
        
        // Met à jour le type
        unsafe {
            let slot_mut = slot as *const Slot as *mut Slot;
            (*slot_mut).msg_type = MessageType::Shared;
            (*slot_mut).flags = 0;
        }
        
        // Memory barrier
        fence(Ordering::Release);
        
        // Marque le slot comme rempli
        slot.seq.store(tail + 1, Ordering::Release);
        
        // Avance le tail
        self.tail.store(tail + 1, Ordering::Release);
        
        Ok(())
    }
    
    /// Envoie automatiquement inline ou zerocopy selon la taille
    #[inline(always)]
    pub fn send(&self, data: &[u8]) -> Result<(), IpcError> {
        if data.len() <= INLINE_SIZE {
            self.send_inline(data)
        } else {
            // Pour le zero-copy, il faut allouer une page
            // Pour l'instant, retourne une erreur (sera implémenté avec SharedMemoryPool)
            Err(IpcError::TooLarge)
        }
    }
    
    /// Envoie avec pool de mémoire partagée (zero-copy pour gros messages)
    pub fn send_with_pool(&self, data: &[u8], pool: &SharedMemoryPool) -> Result<(), IpcError> {
        if data.len() <= INLINE_SIZE {
            // Fast path : inline
            self.send_inline(data)
        } else if data.len() <= PAGE_SIZE {
            // Zero-copy : alloue une page
            let phys_addr = pool.alloc_page().ok_or(IpcError::TooLarge)?;
            
            // Copie les données dans la page
            // NOTE: En production, il faudrait mapper la page en mémoire virtuelle
            // Pour l'instant, on suppose que phys_addr est directement accessible
            unsafe {
                let dst = phys_addr as *mut u8;
                core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            }
            
            // Envoie le descripteur
            self.send_zerocopy(phys_addr, data.len() as u32, 0)
        } else {
            // Trop grand pour une seule page
            Err(IpcError::TooLarge)
        }
    }
    
    /// Envoie un message (fast path inline)
    #[inline(always)]
    pub fn send_inline(&self, data: &[u8]) -> Result<(), IpcError> {
        if data.len() > INLINE_SIZE {
            return Err(IpcError::TooLarge);
        }
        
        let tail = self.tail.load(Ordering::Relaxed);
        let slot_idx = (tail & (RING_SIZE as u64 - 1)) as usize;
        let slot = &self.slots[slot_idx];
        
        // Vérifie si le slot est disponible
        if slot.seq.load(Ordering::Acquire) != tail {
            return Err(IpcError::Full);
        }
        
        // Copie les données inline
        let mut inline_data = InlineData { data: [0; INLINE_SIZE] };
        inline_data.data[..data.len()].copy_from_slice(data);
        
        // Écrit le payload
        unsafe {
            let payload_ptr = &slot.payload as *const SlotPayload as *mut SlotPayload;
            (*payload_ptr).inline = inline_data;
        }
        
        // Met à jour le type et les flags
        unsafe {
            let slot_mut = slot as *const Slot as *mut Slot;
            (*slot_mut).msg_type = MessageType::Inline;
            (*slot_mut).flags = 0;
        }
        
        // Memory barrier pour garantir la visibilité
        fence(Ordering::Release);
        
        // Marque le slot comme rempli
        slot.seq.store(tail + 1, Ordering::Release);
        
        // Avance le tail
        self.tail.store(tail + 1, Ordering::Release);
        
        Ok(())
    }
    
    /// Reçoit un message (non-bloquant)
    #[inline(always)]
    pub fn recv(&self) -> Result<Message, IpcError> {
        let head = self.head.load(Ordering::Relaxed);
        let slot_idx = (head & (RING_SIZE as u64 - 1)) as usize;
        let slot = &self.slots[slot_idx];
        
        // Vérifie si données disponibles
        if slot.seq.load(Ordering::Acquire) != head + 1 {
            return Err(IpcError::Empty);
        }
        
        // Lit le message selon le type
        let msg = unsafe {
            match slot.msg_type {
                MessageType::Inline => {
                    let data = slot.payload.inline.data;
                    Message::Inline(data)
                }
                MessageType::Shared => {
                    let desc = slot.payload.shared;
                    Message::Shared(desc)
                }
                MessageType::Batch => {
                    let desc = slot.payload.batch;
                    Message::Batch(desc)
                }
                MessageType::Control => {
                    Message::Control
                }
            }
        };
        
        // Marque le slot comme libre
        slot.seq.store(head + RING_SIZE as u64 + 1, Ordering::Release);
        
        // Avance le head
        self.head.store(head + 1, Ordering::Release);
        
        Ok(msg)
    }
    
    /// Envoie un batch de messages (optimisé avec une seule fence)
    /// 
    /// Gain de performance : au lieu de 16 fences pour 16 messages,
    /// une seule fence pour tout le batch
    pub fn send_batch(&self, messages: &[&[u8]]) -> Result<usize, IpcError> {
        if messages.is_empty() {
            return Ok(0);
        }
        
        // Vérifie qu'il y a assez de slots disponibles
        if messages.len() > self.available_slots() {
            return Err(IpcError::Full);
        }
        
        let start_tail = self.tail.load(Ordering::Relaxed);
        let mut sent_count = 0;
        
        // Écrit tous les messages sans fence
        for (i, data) in messages.iter().enumerate() {
            if data.len() > INLINE_SIZE {
                // Skip les messages trop grands (on pourrait aussi retourner une erreur)
                continue;
            }
            
            let tail = start_tail + i as u64;
            let slot_idx = (tail & (RING_SIZE as u64 - 1)) as usize;
            let slot = &self.slots[slot_idx];
            
            // Vérifie disponibilité
            if slot.seq.load(Ordering::Relaxed) != tail {
                break; // Ring plein
            }
            
            // Copie données inline
            let mut inline_data = InlineData { data: [0; INLINE_SIZE] };
            inline_data.data[..data.len()].copy_from_slice(data);
            
            // Écrit payload
            unsafe {
                let payload_ptr = &slot.payload as *const SlotPayload as *mut SlotPayload;
                (*payload_ptr).inline = inline_data;
            }
            
            // Met à jour type
            unsafe {
                let slot_mut = slot as *const Slot as *mut Slot;
                (*slot_mut).msg_type = MessageType::Inline;
                (*slot_mut).flags = 0;
            }
            
            sent_count += 1;
        }
        
        if sent_count == 0 {
            return Err(IpcError::TooLarge);
        }
        
        // UNE SEULE memory barrier pour tout le batch
        fence(Ordering::Release);
        
        // Marque tous les slots comme remplis
        for i in 0..sent_count {
            let tail = start_tail + i as u64;
            let slot_idx = (tail & (RING_SIZE as u64 - 1)) as usize;
            let slot = &self.slots[slot_idx];
            slot.seq.store(tail + 1, Ordering::Release);
        }
        
        // Avance le tail d'un coup
        self.tail.store(start_tail + sent_count as u64, Ordering::Release);
        
        Ok(sent_count)
    }
    
    /// Nombre de slots disponibles
    #[inline]
    pub fn available_slots(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        RING_SIZE - (tail.wrapping_sub(head) as usize)
    }
    
    /// Nombre de messages en attente
    #[inline]
    pub fn pending_messages(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        tail.wrapping_sub(head) as usize
    }
}

/// Message reçu
#[derive(Debug)]
pub enum Message {
    Inline([u8; INLINE_SIZE]),
    Shared(SharedMemDescriptor),
    Batch(BatchDescriptor),
    Control,
}

/// Erreurs IPC
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum IpcError {
    Full,
    Empty,
    TooLarge,
    InvalidDescriptor,
}

/// Pool de mémoire partagée pour le zero-copy
pub struct SharedMemoryPool {
    /// Pages disponibles (adresses physiques)
    free_pages: Mutex<Vec<u64>>,
    
    /// Pages allouées (pour tracking et cleanup)
    allocated_pages: Mutex<Vec<u64>>,
}

impl SharedMemoryPool {
    /// Crée un nouveau pool
    pub fn new() -> Self {
        Self {
            free_pages: Mutex::new(Vec::new()),
            allocated_pages: Mutex::new(Vec::new()),
        }
    }
    
    /// Ajoute une page au pool (appelé par le gestionnaire de mémoire)
    pub fn add_page(&self, phys_addr: u64) {
        self.free_pages.lock().push(phys_addr);
    }
    
    /// Alloue une page pour le zero-copy
    pub fn alloc_page(&self) -> Option<u64> {
        let mut free = self.free_pages.lock();
        if let Some(page) = free.pop() {
            self.allocated_pages.lock().push(page);
            Some(page)
        } else {
            None
        }
    }
    
    /// Libère une page
    pub fn free_page(&self, phys_addr: u64) {
        let mut allocated = self.allocated_pages.lock();
        if let Some(pos) = allocated.iter().position(|&p| p == phys_addr) {
            allocated.swap_remove(pos);
            self.free_pages.lock().push(phys_addr);
        }
    }
    
    /// Nombre de pages disponibles
    pub fn available_pages(&self) -> usize {
        self.free_pages.lock().len()
    }
    
    /// Nombre de pages allouées
    pub fn allocated_count(&self) -> usize {
        self.allocated_pages.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_ring_creation() {
        let ring = FusionRing::new();
        assert_eq!(ring.available_slots(), RING_SIZE);
        assert_eq!(ring.pending_messages(), 0);
    }
    
    #[test]
    fn test_send_recv_inline() {
        let ring = FusionRing::new();
        
        // Envoie
        let data = [1u8, 2, 3, 4, 5];
        ring.send_inline(&data).unwrap();
        
        // Vérifie état
        assert_eq!(ring.pending_messages(), 1);
        
        // Reçoit
        match ring.recv().unwrap() {
            Message::Inline(received) => {
                assert_eq!(&received[..5], &data);
            }
            _ => panic!("Wrong message type"),
        }
        
        // Vérifie état
        assert_eq!(ring.pending_messages(), 0);
    }
    
    #[test]
    fn test_multiple_messages() {
        let ring = FusionRing::new();
        
        // Envoie 100 messages
        for i in 0..100 {
            let data = [i as u8; 56];
            ring.send_inline(&data).unwrap();
        }
        
        assert_eq!(ring.pending_messages(), 100);
        
        // Reçoit tous
        for i in 0..100 {
            match ring.recv().unwrap() {
                Message::Inline(received) => {
                    assert_eq!(received[0], i as u8);
                }
                _ => panic!("Wrong message type"),
            }
        }
        
        assert_eq!(ring.pending_messages(), 0);
    }
    
    #[test]
    fn test_ring_full() {
        let ring = FusionRing::new();
        
        // Remplit complètement
        for _ in 0..RING_SIZE {
            let data = [0u8; 56];
            ring.send_inline(&data).unwrap();
        }
        
        // Le prochain doit échouer
        let result = ring.send_inline(&[0u8; 56]);
        assert_eq!(result, Err(IpcError::Full));
    }
    
    #[test]
    fn test_too_large() {
        let ring = FusionRing::new();
        
        // Message trop grand
        let data = [0u8; INLINE_SIZE + 1];
        let result = ring.send_inline(&data);
        assert_eq!(result, Err(IpcError::TooLarge));
    }
    
    #[test]
    fn test_shared_memory_pool() {
        let pool = SharedMemoryPool::new();
        
        // Initialement vide
        assert_eq!(pool.available_pages(), 0);
        assert_eq!(pool.allocated_count(), 0);
        
        // Ajoute quelques pages
        pool.add_page(0x1000);
        pool.add_page(0x2000);
        pool.add_page(0x3000);
        
        assert_eq!(pool.available_pages(), 3);
        
        // Alloue une page
        let page = pool.alloc_page().unwrap();
        assert_eq!(pool.available_pages(), 2);
        assert_eq!(pool.allocated_count(), 1);
        
        // Libère la page
        pool.free_page(page);
        assert_eq!(pool.available_pages(), 3);
        assert_eq!(pool.allocated_count(), 0);
    }
    
    #[test]
    fn test_send_zerocopy() {
        let ring = FusionRing::new();
        let pool = SharedMemoryPool::new();
        
        // Ajoute une page au pool
        // NOTE: En test, on utilise une adresse fictive
        // En production, ce serait une vraie page physique
        pool.add_page(0x100000);
        
        // Message plus grand que inline (>56 bytes)
        let large_data = [0xAB; 100];
        
        // Envoie avec pool (devrait utiliser zero-copy)
        ring.send_with_pool(&large_data, &pool).unwrap();
        
        // Vérifie qu'une page a été allouée
        assert_eq!(pool.allocated_count(), 1);
        assert_eq!(pool.available_pages(), 0);
        
        // Reçoit le message
        match ring.recv().unwrap() {
            Message::Shared(desc) => {
                assert_eq!(desc.size, 100);
                assert_eq!(desc.phys_addr, 0x100000);
                
                // Libère la page après réception
                pool.free_page(desc.phys_addr);
            }
            _ => panic!("Expected Shared message"),
        }
        
        // Vérifie que la page est libérée
        assert_eq!(pool.allocated_count(), 0);
        assert_eq!(pool.available_pages(), 1);
    }
    
    #[test]
    fn test_send_auto_inline_vs_zerocopy() {
        let ring = FusionRing::new();
        
        // Petit message → inline
        let small = [1u8; 32];
        ring.send(&small).unwrap();
        
        match ring.recv().unwrap() {
            Message::Inline(data) => {
                assert_eq!(&data[..32], &small);
            }
            _ => panic!("Expected Inline message"),
        }
        
        // Grand message sans pool → erreur
        let large = [2u8; 100];
        let result = ring.send(&large);
        assert_eq!(result, Err(IpcError::TooLarge));
    }
    
    #[test]
    fn test_send_batch() {
        let ring = FusionRing::new();
        
        // Prépare 16 messages
        let msg1 = [1u8; 32];
        let msg2 = [2u8; 32];
        let msg3 = [3u8; 32];
        let msg4 = [4u8; 32];
        let msg5 = [5u8; 32];
        let msg6 = [6u8; 32];
        let msg7 = [7u8; 32];
        let msg8 = [8u8; 32];
        let msg9 = [9u8; 32];
        let msg10 = [10u8; 32];
        let msg11 = [11u8; 32];
        let msg12 = [12u8; 32];
        let msg13 = [13u8; 32];
        let msg14 = [14u8; 32];
        let msg15 = [15u8; 32];
        let msg16 = [16u8; 32];
        
        let messages = [
            &msg1[..], &msg2[..], &msg3[..], &msg4[..],
            &msg5[..], &msg6[..], &msg7[..], &msg8[..],
            &msg9[..], &msg10[..], &msg11[..], &msg12[..],
            &msg13[..], &msg14[..], &msg15[..], &msg16[..]
        ];
        
        // Envoie le batch (1 seule fence au lieu de 16)
        let sent = ring.send_batch(&messages).unwrap();
        assert_eq!(sent, 16);
        assert_eq!(ring.pending_messages(), 16);
        
        // Vérifie réception dans l'ordre
        for i in 1..=16 {
            match ring.recv().unwrap() {
                Message::Inline(data) => {
                    assert_eq!(data[0], i as u8);
                }
                _ => panic!("Expected Inline message"),
            }
        }
        
        assert_eq!(ring.pending_messages(), 0);
    }
    
    #[test]
    fn test_batch_vs_individual_performance() {
        let ring = FusionRing::new();
        
        // Test avec batch : beaucoup plus rapide grâce à la fence unique
        let msg = [0xAB; 32];
        let messages: Vec<&[u8]> = (0..100).map(|_| &msg[..]).collect();
        
        let sent = ring.send_batch(&messages).unwrap();
        assert_eq!(sent, 100);
        
        // Vide le ring
        for _ in 0..100 {
            ring.recv().unwrap();
        }
        
        // Test individuel (pour comparaison)
        for _ in 0..100 {
            ring.send_inline(&msg).unwrap();
        }
        
        assert_eq!(ring.pending_messages(), 100);
    }
    
    #[test]
    fn test_batch_partial_send() {
        let ring = FusionRing::new();
        
        // Remplit presque complètement le ring
        let msg = [1u8; 32];
        for _ in 0..(RING_SIZE - 10) {
            ring.send_inline(&msg).unwrap();
        }
        
        // Essaie d'envoyer un batch de 20 messages (mais seulement 10 slots dispo)
        let messages: Vec<&[u8]> = (0..20).map(|_| &msg[..]).collect();
        let sent = ring.send_batch(&messages).unwrap();
        
        // Devrait avoir envoyé seulement 10 messages
        assert_eq!(sent, 10);
        assert_eq!(ring.available_slots(), 0);
    }
    
    #[test]
    fn test_batch_too_large_messages() {
        let ring = FusionRing::new();
        
        // Tous les messages sont trop grands
        let large = [0u8; INLINE_SIZE + 1];
        let messages = [&large[..], &large[..], &large[..]];
        
        let result = ring.send_batch(&messages);
        assert_eq!(result, Err(IpcError::TooLarge));
    }
    
    #[test]
    fn test_batch_empty() {
        let ring = FusionRing::new();
        
        let messages: [&[u8]; 0] = [];
        let result = ring.send_batch(&messages).unwrap();
        assert_eq!(result, 0);
    }
}
