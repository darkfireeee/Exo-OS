//! # Network Interface Abstraction
//! 
//! Abstraction de haut niveau pour interfaces réseau

use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use crate::sync::SpinLock;
use super::netdev::{NetworkDevice, DeviceType};
use super::skb::SocketBuffer;
use crate::net::ip::{Ipv4Address as Ipv4Addr, Ipv6Address as Ipv6Addr};

/// Configuration d'une interface
#[derive(Debug, Clone)]
pub struct InterfaceConfig {
    /// Nom (eth0, wlan0, etc.)
    pub name: String,
    
    /// Adresses IPv4
    pub ipv4_addrs: Vec<Ipv4Address>,
    
    /// Adresses IPv6
    pub ipv6_addrs: Vec<Ipv6Address>,
    
    /// Gateway par défaut
    pub default_gateway: Option<[u8; 16]>,
    /// DNS servers
    pub dns_servers: Vec<[u8; 16]>,
    
    /// MTU
    pub mtu: u32,
    
    /// Flags
    pub up: bool,
    pub promiscuous: bool,
    pub allmulti: bool,
}

/// IPv4 configuration with netmask
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Config {
    pub addr: Ipv4Addr,
    pub netmask: [u8; 4],
    pub broadcast: [u8; 4],
}

/// IPv6 configuration with prefix length
#[derive(Debug, Clone, Copy)]
pub struct Ipv6Config {
    pub addr: Ipv6Addr,
    pub prefix_len: u8,
}

// Type aliases for backward compatibility
pub type Ipv4Address = Ipv4Config;
pub type Ipv6Address = Ipv6Config;   pub prefix_len: u8,
}

/// Interface réseau (couche au-dessus de NetworkDevice)
pub struct NetworkInterface {
    /// Device sous-jacent
    device: Arc<NetworkDevice>,
    
    /// Configuration
    config: SpinLock<InterfaceConfig>,
    
    /// Interface ID unique
    id: u32,
    
    /// Compteurs
    rx_packets: AtomicU32,
    tx_packets: AtomicU32,
    rx_bytes: AtomicU32,
    tx_bytes: AtomicU32,
    rx_errors: AtomicU32,
    tx_errors: AtomicU32,
}

impl NetworkInterface {
    pub fn new(device: Arc<NetworkDevice>, config: InterfaceConfig) -> Self {
        static NEXT_ID: AtomicU32 = AtomicU32::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        
        Self {
            device,
            config: SpinLock::new(config),
            id,
            rx_packets: AtomicU32::new(0),
            tx_packets: AtomicU32::new(0),
            rx_bytes: AtomicU32::new(0),
            tx_bytes: AtomicU32::new(0),
            rx_errors: AtomicU32::new(0),
            tx_errors: AtomicU32::new(0),
        }
    }
    
    /// Obtient le nom
    pub fn name(&self) -> String {
        self.config.lock().name.clone()
    }
    
    /// Obtient l'ID
    pub fn id(&self) -> u32 {
        self.id
    }
    
    /// Obtient le device sous-jacent
    pub fn device(&self) -> &Arc<NetworkDevice> {
        &self.device
    }
    
    /// Active l'interface
    pub fn bring_up(&self) -> Result<(), InterfaceError> {
        self.device.up().map_err(|_| InterfaceError::DeviceError)?;
        self.config.lock().up = true;
        Ok(())
    }
    
    /// Désactive l'interface
    pub fn bring_down(&self) -> Result<(), InterfaceError> {
        self.device.down().map_err(|_| InterfaceError::DeviceError)?;
        self.config.lock().up = false;
        Ok(())
    }
    
    /// Est-ce que l'interface est up?
    pub fn is_up(&self) -> bool {
        self.config.lock().up
    }
    
    /// Ajoute une adresse IPv4
    pub fn add_ipv4(&self, addr: Ipv4Address) {
        self.config.lock().ipv4_addrs.push(addr);
    }
    
    /// Ajoute une adresse IPv6
    pub fn add_ipv6(&self, addr: Ipv6Address) {
        self.config.lock().ipv6_addrs.push(addr);
    }
    
    /// Obtient les adresses IPv4
    pub fn ipv4_addrs(&self) -> Vec<Ipv4Address> {
        self.config.lock().ipv4_addrs.clone()
    }
    
    /// Obtient les adresses IPv6
    pub fn ipv6_addrs(&self) -> Vec<Ipv6Address> {
        self.config.lock().ipv6_addrs.clone()
    }
    
    /// Set MTU
    pub fn set_mtu(&self, mtu: u32) -> Result<(), InterfaceError> {
        self.device.set_mtu(mtu).map_err(|_| InterfaceError::InvalidMtu)?;
        self.config.lock().mtu = mtu;
        Ok(())
    }
    
    /// Envoie un paquet
    pub fn send(&self, skb: SocketBuffer) -> Result<(), InterfaceError> {
        if !self.is_up() {
            return Err(InterfaceError::InterfaceDown);
        }
        
        let len = skb.len() as u32;
        
        match self.device.transmit(skb) {
            Ok(()) => {
                self.tx_packets.fetch_add(1, Ordering::Relaxed);
                self.tx_bytes.fetch_add(len, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => {
                self.tx_errors.fetch_add(1, Ordering::Relaxed);
                Err(InterfaceError::TransmitError)
            }
        }
    }
    
    /// Reçoit un paquet
    pub fn receive(&self, skb: SocketBuffer) {
        let len = skb.len() as u32;
        self.rx_packets.fetch_add(1, Ordering::Relaxed);
        self.rx_bytes.fetch_add(len, Ordering::Relaxed);
        self.device.receive(skb);
    }
    
    /// Obtient les stats
    pub fn stats(&self) -> InterfaceStats {
        InterfaceStats {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            rx_errors: self.rx_errors.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InterfaceStats {
    pub rx_packets: u32,
    pub tx_packets: u32,
    pub rx_bytes: u32,
    pub tx_bytes: u32,
    pub rx_errors: u32,
    pub tx_errors: u32,
}

/// Erreurs d'interface
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceError {
    DeviceError,
    InterfaceDown,
    InvalidMtu,
    TransmitError,
    NotFound,
}

/// Gestionnaire d'interfaces
pub struct InterfaceManager {
    interfaces: SpinLock<Vec<Arc<NetworkInterface>>>,
}

impl InterfaceManager {
    pub const fn new() -> Self {
        Self {
            interfaces: SpinLock::new(Vec::new()),
        }
    }
    
    /// Enregistre une interface
    pub fn register(&self, iface: Arc<NetworkInterface>) {
        self.interfaces.lock().push(iface);
    }
    
    /// Trouve une interface par nom
    pub fn find_by_name(&self, name: &str) -> Option<Arc<NetworkInterface>> {
        self.interfaces.lock()
            .iter()
            .find(|iface| iface.name() == name)
            .cloned()
    }
    
    /// Trouve une interface par ID
    pub fn find_by_id(&self, id: u32) -> Option<Arc<NetworkInterface>> {
        self.interfaces.lock()
            .iter()
            .find(|iface| iface.id() == id)
            .cloned()
    }
    
    /// Liste toutes les interfaces
    pub fn list(&self) -> Vec<Arc<NetworkInterface>> {
        self.interfaces.lock().clone()
    }
}

/// Instance globale
pub static INTERFACE_MANAGER: InterfaceManager = InterfaceManager::new();
