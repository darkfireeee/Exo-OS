//! # Socket Buffer (skb) - Zero-Copy Packet Management
//! 
//! Structure similaire à sk_buff Linux mais moderne et optimisée.

use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Socket Buffer - structure centrale pour packets
pub struct SocketBuffer {
    /// Données du paquet (peut être DMA)
    pub data: Vec<u8>,
    
    /// Offsets dans data
    pub head: usize,      // Début du buffer
    pub data_start: usize, // Début des données
    pub tail: usize,      // Fin des données
    pub end: usize,       // Fin du buffer
    
    /// Metadata réseau
    pub network_header: Option<usize>,
    pub transport_header: Option<usize>,
    pub mac_header: Option<usize>,
    
    /// Interface
    pub iface: u32,
    
    /// Protocole
    pub protocol: u16,
    
    /// Timestamps
    pub timestamp: u64,
    
    /// Ref count pour zero-copy
    ref_count: Arc<AtomicU32>,
    
    /// Checksums
    pub ip_summed: bool,
    pub csum: u32,
}

impl SocketBuffer {
    /// Alloue un nouveau skb
    pub fn new(capacity: usize) -> Self {
        let data = vec![0u8; capacity];
        
        Self {
            head: 0,
            data_start: 0,
            tail: 0,
            end: capacity,
            network_header: None,
            transport_header: None,
            mac_header: None,
            iface: 0,
            protocol: 0,
            timestamp: 0,
            ref_count: Arc::new(AtomicU32::new(1)),
            data,
            ip_summed: false,
            csum: 0,
        }
    }
    
    /// Alloue avec headroom
    pub fn with_headroom(capacity: usize, headroom: usize) -> Self {
        let mut skb = Self::new(capacity);
        skb.data_start = headroom;
        skb.tail = headroom;
        skb
    }
    
    /// Clone (increment ref count)
    pub fn clone_ref(&self) -> Self {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
        
        Self {
            data: self.data.clone(),
            head: self.head,
            data_start: self.data_start,
            tail: self.tail,
            end: self.end,
            network_header: self.network_header,
            transport_header: self.transport_header,
            mac_header: self.mac_header,
            iface: self.iface,
            protocol: self.protocol,
            timestamp: self.timestamp,
            ref_count: self.ref_count.clone(),
            ip_summed: self.ip_summed,
            csum: self.csum,
        }
    }
    
    /// Longueur des données
    #[inline]
    pub fn len(&self) -> usize {
        self.tail - self.data_start
    }
    
    /// Headroom disponible
    #[inline]
    pub fn headroom(&self) -> usize {
        self.data_start - self.head
    }
    
    /// Tailroom disponible
    #[inline]
    pub fn tailroom(&self) -> usize {
        self.end - self.tail
    }
    
    /// Données du paquet
    #[inline]
    pub fn data_slice(&self) -> &[u8] {
        &self.data[self.data_start..self.tail]
    }
    
    /// Données mutables
    #[inline]
    pub fn data_slice_mut(&mut self) -> &mut [u8] {
        &mut self.data[self.data_start..self.tail]
    }
    
    /// Push data (ajoute à la fin)
    pub fn push(&mut self, data: &[u8]) -> Result<(), SkbError> {
        if self.tailroom() < data.len() {
            return Err(SkbError::NoSpace);
        }
        
        self.data[self.tail..self.tail + data.len()].copy_from_slice(data);
        self.tail += data.len();
        
        Ok(())
    }
    
    /// Put (ajoute au début - push header)
    pub fn put(&mut self, data: &[u8]) -> Result<(), SkbError> {
        if self.headroom() < data.len() {
            return Err(SkbError::NoSpace);
        }
        
        self.data_start -= data.len();
        self.data[self.data_start..self.data_start + data.len()].copy_from_slice(data);
        
        Ok(())
    }
    
    /// Pull (retire du début)
    pub fn pull(&mut self, len: usize) -> Result<(), SkbError> {
        if self.len() < len {
            return Err(SkbError::InvalidLength);
        }
        
        self.data_start += len;
        Ok(())
    }
    
    /// Reserve headroom
    pub fn reserve(&mut self, len: usize) {
        self.data_start += len;
        self.tail += len;
    }
    
    /// Reset pour réutilisation
    pub fn reset(&mut self) {
        self.data_start = 0;
        self.tail = 0;
        self.network_header = None;
        self.transport_header = None;
        self.mac_header = None;
        self.ip_summed = false;
        self.csum = 0;
    }
}

impl Drop for SocketBuffer {
    fn drop(&mut self) {
        let prev = self.ref_count.fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            // Dernier ref : free le buffer
            // (Rust le fait automatiquement)
        }
    }
}

/// Pool de skb pour allocation rapide
pub struct SkbPool {
    small: Vec<SocketBuffer>,  // 256 bytes
    medium: Vec<SocketBuffer>, // 2K
    large: Vec<SocketBuffer>,  // 64K
    allocated: AtomicUsize,
}

impl SkbPool {
    pub fn new() -> Self {
        Self {
            small: Vec::new(),
            medium: Vec::new(),
            large: Vec::new(),
            allocated: AtomicUsize::new(0),
        }
    }
    
    /// Alloue un skb depuis le pool
    pub fn alloc(&mut self, size: usize) -> SocketBuffer {
        self.allocated.fetch_add(1, Ordering::Relaxed);
        
        if size <= 256 {
            self.small.pop().unwrap_or_else(|| SocketBuffer::new(256))
        } else if size <= 2048 {
            self.medium.pop().unwrap_or_else(|| SocketBuffer::new(2048))
        } else {
            self.large.pop().unwrap_or_else(|| SocketBuffer::new(65536))
        }
    }
    
    /// Retourne un skb au pool
    pub fn free(&mut self, mut skb: SocketBuffer) {
        skb.reset();
        
        match skb.end {
            256 => self.small.push(skb),
            2048 => self.medium.push(skb),
            65536 => self.large.push(skb),
            _ => {} // Taille custom : drop
        }
    }
    
    pub fn stats(&self) -> SkbPoolStats {
        SkbPoolStats {
            allocated: self.allocated.load(Ordering::Relaxed),
            small_cached: self.small.len(),
            medium_cached: self.medium.len(),
            large_cached: self.large.len(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SkbPoolStats {
    pub allocated: usize,
    pub small_cached: usize,
    pub medium_cached: usize,
    pub large_cached: usize,
}

/// Erreurs skb
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkbError {
    NoSpace,
    InvalidLength,
    InvalidOffset,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_skb_push() {
        let mut skb = SocketBuffer::with_headroom(1024, 256);
        
        skb.push(&[1, 2, 3, 4]).unwrap();
        assert_eq!(skb.len(), 4);
        assert_eq!(skb.data_slice(), &[1, 2, 3, 4]);
    }
    
    #[test]
    fn test_skb_put() {
        let mut skb = SocketBuffer::with_headroom(1024, 256);
        
        skb.push(&[3, 4]).unwrap();
        skb.put(&[1, 2]).unwrap();
        
        assert_eq!(skb.len(), 4);
        assert_eq!(skb.data_slice(), &[1, 2, 3, 4]);
    }
    
    #[test]
    fn test_skb_pool() {
        let mut pool = SkbPool::new();
        
        let skb1 = pool.alloc(100);
        let skb2 = pool.alloc(1500);
        
        assert_eq!(skb1.end, 256);
        assert_eq!(skb2.end, 2048);
        
        pool.free(skb1);
        assert_eq!(pool.small.len(), 1);
    }
}
