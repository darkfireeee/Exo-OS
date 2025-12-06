//! # IP Fragmentation & Reassembly
//! 
//! Gestion de la fragmentation IPv4/IPv6 (RFC 815, RFC 8200)

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::SpinLock;

/// Fragment d'un paquet IP
#[derive(Debug, Clone)]
pub struct IpFragment {
    /// Offset dans le paquet original (en octets)
    pub offset: u16,
    
    /// Longueur du fragment
    pub length: u16,
    
    /// Données
    pub data: Vec<u8>,
    
    /// Timestamp de réception
    pub timestamp: u64,
    
    /// More Fragments flag
    pub more_fragments: bool,
}

/// Clé unique pour identifier un paquet fragmenté
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FragmentKey {
    /// Adresse source
    pub src: [u8; 16], // IPv6 format (IPv4-mapped pour IPv4)
    
    /// Adresse dest
    pub dst: [u8; 16],
    
    /// ID du paquet
    pub id: u32,
    
    /// Protocole (6=TCP, 17=UDP, etc.)
    pub protocol: u8,
}

/// Cache de fragments en cours de réassemblage
pub struct FragmentCache {
    /// Fragments groupés par clé
    fragments: BTreeMap<FragmentKey, Vec<IpFragment>>,
    
    /// Timeout (60s par défaut - RFC 791)
    timeout: u64,
    
    /// Stats
    stats: FragmentStats,
}

impl FragmentCache {
    const DEFAULT_TIMEOUT: u64 = 60_000_000; // 60 secondes
    
    pub fn new() -> Self {
        Self {
            fragments: BTreeMap::new(),
            timeout: Self::DEFAULT_TIMEOUT,
            stats: FragmentStats::new(),
        }
    }
    
    /// Ajoute un fragment
    pub fn add_fragment(
        &mut self,
        key: FragmentKey,
        fragment: IpFragment,
        now: u64,
    ) -> FragmentResult {
        // Ajoute au cache
        let frags = self.fragments.entry(key).or_insert_with(Vec::new);
        frags.push(fragment.clone());
        
        // Trie par offset
        frags.sort_by_key(|f| f.offset);
        
        // Vérifie si complet
        if self.is_complete(&frags) {
            // Réassemble
            match self.reassemble(frags) {
                Ok(data) => {
                    self.fragments.remove(&key);
                    self.stats.reassembled.fetch_add(1, Ordering::Relaxed);
                    FragmentResult::Complete(data)
                }
                Err(e) => {
                    self.fragments.remove(&key);
                    self.stats.errors.fetch_add(1, Ordering::Relaxed);
                    FragmentResult::Error(e)
                }
            }
        } else {
            self.stats.received.fetch_add(1, Ordering::Relaxed);
            FragmentResult::Incomplete
        }
    }
    
    /// Vérifie si tous les fragments sont présents
    fn is_complete(&self, fragments: &[IpFragment]) -> bool {
        if fragments.is_empty() {
            return false;
        }
        
        // Le dernier fragment doit avoir more_fragments = false
        let last = &fragments[fragments.len() - 1];
        if last.more_fragments {
            return false;
        }
        
        // Vérifie continuité (pas de trous)
        let mut expected_offset = 0u16;
        for frag in fragments {
            if frag.offset != expected_offset {
                return false; // Trou détecté
            }
            expected_offset += frag.length;
        }
        
        true
    }
    
    /// Réassemble les fragments en un paquet complet
    fn reassemble(&self, fragments: &[IpFragment]) -> Result<Vec<u8>, FragmentError> {
        if fragments.is_empty() {
            return Err(FragmentError::NoFragments);
        }
        
        // Calcule taille totale
        let total_size: usize = fragments.iter()
            .map(|f| f.length as usize)
            .sum();
        
        // Limite à 64K (limite IP)
        if total_size > 65535 {
            return Err(FragmentError::TooLarge);
        }
        
        // Copie les données
        let mut data = Vec::with_capacity(total_size);
        for frag in fragments {
            data.extend_from_slice(&frag.data);
        }
        
        Ok(data)
    }
    
    /// Nettoie les fragments expirés
    pub fn cleanup(&mut self, now: u64) {
        self.fragments.retain(|_key, frags| {
            if frags.is_empty() {
                return false;
            }
            
            let first_timestamp = frags[0].timestamp;
            if now > first_timestamp + self.timeout {
                self.stats.timeouts.fetch_add(1, Ordering::Relaxed);
                false
            } else {
                true
            }
        });
    }
    
    pub fn stats(&self) -> FragmentStatsSnapshot {
        self.stats.snapshot()
    }
}

/// Résultat de l'ajout d'un fragment
pub enum FragmentResult {
    /// Paquet complet réassemblé
    Complete(Vec<u8>),
    
    /// Encore des fragments manquants
    Incomplete,
    
    /// Erreur de réassemblage
    Error(FragmentError),
}

/// Erreurs de fragmentation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentError {
    NoFragments,
    TooLarge,
    InvalidOffset,
    Timeout,
}

/// Statistiques de fragmentation
struct FragmentStats {
    received: AtomicU64,
    reassembled: AtomicU64,
    timeouts: AtomicU64,
    errors: AtomicU64,
}

impl FragmentStats {
    fn new() -> Self {
        Self {
            received: AtomicU64::new(0),
            reassembled: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        }
    }
    
    fn snapshot(&self) -> FragmentStatsSnapshot {
        FragmentStatsSnapshot {
            received: self.received.load(Ordering::Relaxed),
            reassembled: self.reassembled.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FragmentStatsSnapshot {
    pub received: u64,
    pub reassembled: u64,
    pub timeouts: u64,
    pub errors: u64,
}

/// Gestionnaire de fragmentation (global)
pub struct FragmentManager {
    cache: SpinLock<FragmentCache>,
}

impl FragmentManager {
    pub const fn new() -> Self {
        Self {
            cache: SpinLock::new(FragmentCache::new()),
        }
    }
    
    pub fn add_fragment(
        &self,
        key: FragmentKey,
        fragment: IpFragment,
        now: u64,
    ) -> FragmentResult {
        self.cache.lock().add_fragment(key, fragment, now)
    }
    
    pub fn cleanup(&self, now: u64) {
        self.cache.lock().cleanup(now);
    }
    
    pub fn stats(&self) -> FragmentStatsSnapshot {
        self.cache.lock().stats()
    }
}

/// Instance globale
pub static FRAGMENT_MANAGER: FragmentManager = FragmentManager::new();

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_reassembly() {
        let mut cache = FragmentCache::new();
        
        let key = FragmentKey {
            src: [0; 16],
            dst: [1; 16],
            id: 1234,
            protocol: 6,
        };
        
        // Fragment 1 (offset 0, 500 bytes)
        let frag1 = IpFragment {
            offset: 0,
            length: 500,
            data: vec![1u8; 500],
            timestamp: 0,
            more_fragments: true,
        };
        
        // Fragment 2 (offset 500, 500 bytes, dernier)
        let frag2 = IpFragment {
            offset: 500,
            length: 500,
            data: vec![2u8; 500],
            timestamp: 0,
            more_fragments: false,
        };
        
        // Ajoute premier fragment
        let result = cache.add_fragment(key, frag1, 0);
        assert!(matches!(result, FragmentResult::Incomplete));
        
        // Ajoute dernier fragment -> réassembly complet
        let result = cache.add_fragment(key, frag2, 0);
        match result {
            FragmentResult::Complete(data) => {
                assert_eq!(data.len(), 1000);
                assert_eq!(&data[0..500], &[1u8; 500]);
                assert_eq!(&data[500..1000], &[2u8; 500]);
            }
            _ => panic!("Expected complete"),
        }
    }
    
    #[test]
    fn test_timeout() {
        let mut cache = FragmentCache::new();
        
        let key = FragmentKey {
            src: [0; 16],
            dst: [1; 16],
            id: 1234,
            protocol: 6,
        };
        
        let frag = IpFragment {
            offset: 0,
            length: 100,
            data: vec![0u8; 100],
            timestamp: 0,
            more_fragments: true,
        };
        
        cache.add_fragment(key, frag, 0);
        assert_eq!(cache.fragments.len(), 1);
        
        // Après timeout
        cache.cleanup(70_000_000); // 70 secondes
        assert_eq!(cache.fragments.len(), 0);
        assert_eq!(cache.stats.timeouts.load(Ordering::Relaxed), 1);
    }
}
