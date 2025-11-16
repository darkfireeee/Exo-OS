//! # Fast Channel - Wrapper IPC utilisant Fusion Ring
//! 
//! Wrapper de compatibilit\u00e9 pour utiliser FusionRing sur les canaux critiques
//! avec mesure de latence int\u00e9gr\u00e9e.

use crate::ipc::fusion_ring::{FusionRing, Message, IpcError, INLINE_SIZE};
use crate::perf_counters::{rdtsc, PERF_MANAGER, Component};

/// Canal rapide utilisant FusionRing en arri\u00e8re-plan
pub struct FastChannel {
    /// Ring buffer sous-jacent
    ring: FusionRing,
    
    /// Nom du canal (pour debug)
    name: &'static str,
    
    /// Nombre de messages envoy\u00e9s
    sent_count: u64,
    
    /// Nombre de messages re\u00e7us
    received_count: u64,
}

impl FastChannel {
    /// Cr\u00e9e un nouveau canal rapide
    pub fn new(name: &'static str) -> Self {
        let ch = Self {
            ring: FusionRing::new(),
            name,
            sent_count: 0,
            received_count: 0,
        };
        crate::println!("[FAST] new FastChannel '{}' ring_ptr={:p}", name, &ch.ring as *const FusionRing);
        ch
    }
    
    /// Envoie un message avec mesure de latence
    pub fn send(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let start = rdtsc();
        
        let result = if data.len() <= INLINE_SIZE {
            // Fast path inline
            self.ring.send_inline(data).map_err(|e| match e {
                IpcError::Full => "Canal plein",
                IpcError::TooLarge => "Message trop grand",
                _ => "Erreur IPC",
            })
        } else {
            // Zero-copy via pool (si disponible)
            Err("Zero-copy non implémenté dans cette version")
        };
        
        let end = rdtsc();
        let cycles = end - start;
        
        if result.is_ok() {
            self.sent_count += 1;
            PERF_MANAGER.record(Component::Ipc, cycles);
        }
        
        result
    }
    
    /// Re\u00e7oit un message avec mesure de latence
    pub fn receive(&mut self) -> Result<alloc::vec::Vec<u8>, &'static str> {
        let start = rdtsc();
        
        let result = self.ring.recv().map(|msg| {
            match msg {
                Message::Inline(data) => {
                    // Trouver la longueur r\u00e9elle (jusqu'au premier 0 ou fin)
                    let len = data.iter().position(|&b| b == 0).unwrap_or(INLINE_SIZE);
                    data[..len].to_vec()
                }
                _ => alloc::vec::Vec::new(),
            }
        }).map_err(|e| match e {
            IpcError::Empty => "Aucun message",
            _ => "Erreur IPC",
        });
        
        let end = rdtsc();
        let cycles = end - start;
        
        if result.is_ok() {
            self.received_count += 1;
            PERF_MANAGER.record(Component::Ipc, cycles);
        }
        
        result
    }
    
    /// Retourne le nombre de messages en attente
    pub fn pending_count(&self) -> usize {
        self.ring.pending_messages()
    }
    
    /// Retourne les statistiques du canal
    pub fn stats(&self) -> (u64, u64) {
        (self.sent_count, self.received_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_fast_channel_create() {
        let channel = FastChannel::new("test");
        assert_eq!(channel.pending_count(), 0);
        assert_eq!(channel.stats(), (0, 0));
    }
    
    #[test]
    fn test_fast_channel_send_receive() {
        let mut channel = FastChannel::new("test");
        
        // Envoie un petit message
        let msg = b"Hello";
        channel.send(msg).unwrap();
        
        assert_eq!(channel.pending_count(), 1);
        
        // Re\u00e7oit le message
        let received = channel.receive().unwrap();
        assert_eq!(&received[..], msg);
        
        assert_eq!(channel.pending_count(), 0);
        assert_eq!(channel.stats(), (1, 1));
    }
    
    #[test]
    fn test_fast_channel_multiple_messages() {
        let mut channel = FastChannel::new("test");
        
        // Envoie plusieurs messages
        for i in 0..10 {
            let msg = alloc::format!("Message {}", i);
            channel.send(msg.as_bytes()).unwrap();
        }
        
        assert_eq!(channel.pending_count(), 10);
        
        // Re\u00e7oit tous les messages
        for i in 0..10 {
            let received = channel.receive().unwrap();
            let expected = alloc::format!("Message {}", i);
            assert_eq!(&received[..], expected.as_bytes());
        }
        
        assert_eq!(channel.pending_count(), 0);
    }
    
    #[test]
    fn test_fast_channel_empty() {
        let mut channel = FastChannel::new("test");
        
        // Essaie de recevoir sur un canal vide
        let result = channel.receive();
        assert!(result.is_err());
    }
}
