//! # Network Device Management
//! 
//! Gestion des périphériques réseau avec API moderne

use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use crate::sync::SpinLock;
use super::skb::SocketBuffer;

/// Type de device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Ethernet,
    Loopback,
    Wireless,
    Virtual,
    Tunnel,
}

/// État du device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Down,
    Up,
    Testing,
    Dormant,
}

/// Flags du device
bitflags::bitflags! {
    pub struct DeviceFlags: u32 {
        const UP = 1 << 0;
        const BROADCAST = 1 << 1;
        const DEBUG = 1 << 2;
        const LOOPBACK = 1 << 3;
        const POINTOPOINT = 1 << 4;
        const RUNNING = 1 << 6;
        const NOARP = 1 << 7;
        const PROMISC = 1 << 8;
        const ALLMULTI = 1 << 9;
        const MULTICAST = 1 << 12;
    }
}

/// Statistiques réseau (compteurs atomiques)
#[derive(Debug)]
pub struct DeviceStats {
    pub rx_packets: AtomicU64,
    pub tx_packets: AtomicU64,
    pub rx_bytes: AtomicU64,
    pub tx_bytes: AtomicU64,
    pub rx_errors: AtomicU64,
    pub tx_errors: AtomicU64,
    pub rx_dropped: AtomicU64,
    pub tx_dropped: AtomicU64,
    pub collisions: AtomicU64,
}

impl DeviceStats {
    pub fn new() -> Self {
        Self {
            rx_packets: AtomicU64::new(0),
            tx_packets: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            rx_errors: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
            rx_dropped: AtomicU64::new(0),
            tx_dropped: AtomicU64::new(0),
            collisions: AtomicU64::new(0),
        }
    }
    
    pub fn snapshot(&self) -> DeviceStatsSnapshot {
        DeviceStatsSnapshot {
            rx_packets: self.rx_packets.load(Ordering::Relaxed),
            tx_packets: self.tx_packets.load(Ordering::Relaxed),
            rx_bytes: self.rx_bytes.load(Ordering::Relaxed),
            tx_bytes: self.tx_bytes.load(Ordering::Relaxed),
            rx_errors: self.rx_errors.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
            rx_dropped: self.rx_dropped.load(Ordering::Relaxed),
            tx_dropped: self.tx_dropped.load(Ordering::Relaxed),
            collisions: self.collisions.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DeviceStatsSnapshot {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub collisions: u64,
}

/// Opérations du device (vtable)
pub trait DeviceOps: Send + Sync {
    /// Ouvre le device
    fn open(&self) -> Result<(), DeviceError>;
    
    /// Ferme le device
    fn close(&self) -> Result<(), DeviceError>;
    
    /// Envoie un paquet
    fn xmit(&self, skb: SocketBuffer) -> Result<(), DeviceError>;
    
    /// Change l'adresse MAC
    fn set_mac(&self, mac: [u8; 6]) -> Result<(), DeviceError>;
    
    /// Change le MTU
    fn set_mtu(&self, mtu: u32) -> Result<(), DeviceError>;
    
    /// Reçoit un paquet (polling)
    fn poll(&self) -> Option<SocketBuffer>;
}

/// Network Device
pub struct NetworkDevice {
    /// Nom (eth0, wlan0, etc.)
    pub name: String,
    
    /// Index unique
    pub index: u32,
    
    /// Type
    pub device_type: DeviceType,
    
    /// État
    pub state: AtomicU32, // DeviceState
    
    /// Flags
    pub flags: SpinLock<DeviceFlags>,
    
    /// Adresse MAC
    pub mac_addr: SpinLock<[u8; 6]>,
    
    /// MTU
    pub mtu: AtomicU32,
    
    /// Statistiques
    pub stats: DeviceStats,
    
    /// Opérations
    pub ops: Arc<dyn DeviceOps>,
    
    /// TX queue
    tx_queue: SpinLock<Vec<SocketBuffer>>,
    
    /// RX queue
    rx_queue: SpinLock<Vec<SocketBuffer>>,
}

impl NetworkDevice {
    pub fn new(
        name: String,
        index: u32,
        device_type: DeviceType,
        ops: Arc<dyn DeviceOps>,
    ) -> Self {
        Self {
            name,
            index,
            device_type,
            state: AtomicU32::new(DeviceState::Down as u32),
            flags: SpinLock::new(DeviceFlags::empty()),
            mac_addr: SpinLock::new([0u8; 6]),
            mtu: AtomicU32::new(1500),
            stats: DeviceStats::new(),
            ops,
            tx_queue: SpinLock::new(Vec::new()),
            rx_queue: SpinLock::new(Vec::new()),
        }
    }
    
    /// Active le device
    pub fn up(&self) -> Result<(), DeviceError> {
        self.ops.open()?;
        self.state.store(DeviceState::Up as u32, Ordering::Release);
        self.flags.lock().insert(DeviceFlags::UP | DeviceFlags::RUNNING);
        Ok(())
    }
    
    /// Désactive le device
    pub fn down(&self) -> Result<(), DeviceError> {
        self.ops.close()?;
        self.state.store(DeviceState::Down as u32, Ordering::Release);
        self.flags.lock().remove(DeviceFlags::UP | DeviceFlags::RUNNING);
        Ok(())
    }
    
    /// Envoie un paquet
    pub fn transmit(&self, skb: SocketBuffer) -> Result<(), DeviceError> {
        // Vérifie état
        if !self.is_up() {
            return Err(DeviceError::DeviceDown);
        }
        
        // Envoie
        self.ops.xmit(skb)?;
        
        // Stats
        self.stats.tx_packets.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Queue un paquet TX
    pub fn queue_tx(&self, skb: SocketBuffer) {
        self.tx_queue.lock().push(skb);
    }
    
    /// Déqueue et envoie
    pub fn flush_tx(&self) -> Result<usize, DeviceError> {
        let mut queue = self.tx_queue.lock();
        let count = queue.len();
        
        for skb in queue.drain(..) {
            self.ops.xmit(skb)?;
            self.stats.tx_packets.fetch_add(1, Ordering::Relaxed);
        }
        
        Ok(count)
    }
    
    /// Reçoit un paquet
    pub fn receive(&self, skb: SocketBuffer) {
        self.stats.rx_packets.fetch_add(1, Ordering::Relaxed);
        self.stats.rx_bytes.fetch_add(skb.len() as u64, Ordering::Relaxed);
        
        self.rx_queue.lock().push(skb);
    }
    
    /// Poll pour RX
    pub fn poll_rx(&self) -> Option<SocketBuffer> {
        self.rx_queue.lock().pop()
    }
    
    /// Est-ce que le device est UP?
    #[inline]
    pub fn is_up(&self) -> bool {
        self.flags.lock().contains(DeviceFlags::UP | DeviceFlags::RUNNING)
    }
    
    /// MAC address
    pub fn mac(&self) -> [u8; 6] {
        *self.mac_addr.lock()
    }
    
    /// Set MAC
    pub fn set_mac(&self, mac: [u8; 6]) -> Result<(), DeviceError> {
        self.ops.set_mac(mac)?;
        *self.mac_addr.lock() = mac;
        Ok(())
    }
    
    /// MTU
    pub fn mtu(&self) -> u32 {
        self.mtu.load(Ordering::Relaxed)
    }
    
    /// Set MTU
    pub fn set_mtu(&self, mtu: u32) -> Result<(), DeviceError> {
        if mtu < 68 || mtu > 65535 {
            return Err(DeviceError::InvalidMtu);
        }
        
        self.ops.set_mtu(mtu)?;
        self.mtu.store(mtu, Ordering::Release);
        Ok(())
    }
}

/// Gestionnaire global de devices
pub struct DeviceManager {
    devices: SpinLock<Vec<Arc<NetworkDevice>>>,
    next_index: AtomicU32,
}

impl DeviceManager {
    pub const fn new() -> Self {
        Self {
            devices: SpinLock::new(Vec::new()),
            next_index: AtomicU32::new(1),
        }
    }
    
    /// Enregistre un device
    pub fn register(&self, device: Arc<NetworkDevice>) {
        self.devices.lock().push(device);
    }
    
    /// Trouve un device par nom
    pub fn find_by_name(&self, name: &str) -> Option<Arc<NetworkDevice>> {
        self.devices.lock()
            .iter()
            .find(|dev| dev.name == name)
            .cloned()
    }
    
    /// Trouve un device par index
    pub fn find_by_index(&self, index: u32) -> Option<Arc<NetworkDevice>> {
        self.devices.lock()
            .iter()
            .find(|dev| dev.index == index)
            .cloned()
    }
    
    /// Liste tous les devices
    pub fn list(&self) -> Vec<Arc<NetworkDevice>> {
        self.devices.lock().clone()
    }
    
    /// Alloue un index unique
    pub fn alloc_index(&self) -> u32 {
        self.next_index.fetch_add(1, Ordering::Relaxed)
    }
}

/// Instance globale
pub static DEVICE_MANAGER: DeviceManager = DeviceManager::new();

/// Erreurs device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceError {
    DeviceDown,
    InvalidMtu,
    NotFound,
    AlreadyExists,
    HardwareError,
    NoMemory,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    struct DummyOps;
    
    impl DeviceOps for DummyOps {
        fn open(&self) -> Result<(), DeviceError> { Ok(()) }
        fn close(&self) -> Result<(), DeviceError> { Ok(()) }
        fn xmit(&self, _skb: SocketBuffer) -> Result<(), DeviceError> { Ok(()) }
        fn set_mac(&self, _mac: [u8; 6]) -> Result<(), DeviceError> { Ok(()) }
        fn set_mtu(&self, _mtu: u32) -> Result<(), DeviceError> { Ok(()) }
        fn poll(&self) -> Option<SocketBuffer> { None }
    }
    
    #[test]
    fn test_device_up_down() {
        let dev = NetworkDevice::new(
            "eth0".into(),
            1,
            DeviceType::Ethernet,
            Arc::new(DummyOps),
        );
        
        assert!(!dev.is_up());
        
        dev.up().unwrap();
        assert!(dev.is_up());
        
        dev.down().unwrap();
        assert!(!dev.is_up());
    }
}
