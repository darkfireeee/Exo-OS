//! Hardware Drivers
//!
//! Phase 0: VGA + Serial
//! Phase 1: Keyboard, PCI
//! Phase 2+: Block, Network, USB

// ═══════════════════════════════════════════════════════════
//  PHASE 0-1 - Drivers actifs
// ═══════════════════════════════════════════════════════════
pub mod char;    // ✅ Serial + Console
pub mod video;   // ✅ VGA text mode
pub mod input;   // ✅ Phase 1: Keyboard
pub mod pci;     // ✅ Phase 1: PCI bus enumeration
pub mod block;   // ✅ Phase 1: Block devices (pour VFS)

// ═══════════════════════════════════════════════════════════
//  PHASE 2+ - Drivers désactivés
// ═══════════════════════════════════════════════════════════
// pub mod net;     // ⏸️ Phase 3: E1000, RTL8139, VirtIO-Net
// pub mod usb;     // ⏸️ Phase 3: USB stack
// pub mod virtio;  // ⏸️ Phase 3: VirtIO devices

/// Error type for driver operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    InitFailed,
    DeviceNotFound,
    IoError,
    NotSupported,
    Timeout,
    InvalidParameter,
    ResourceBusy,
    NoMemory,
}

/// Result type for driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Basic information about a device.
pub struct DeviceInfo {
    pub name: &'static str,
    pub vendor_id: u16,
    pub device_id: u16,
}

/// Trait that all drivers must implement.
pub trait Driver {
    /// Returns the name of the driver.
    fn name(&self) -> &str;

    /// Initializes the driver.
    fn init(&mut self) -> DriverResult<()>;

    /// Probes for the device.
    fn probe(&self) -> DriverResult<DeviceInfo>;
}

/// Initialize hardware drivers (Phase 0: VGA + Serial uniquement)
pub fn init() {
    // Phase 0: VGA et Serial déjà initialisés dans boot
    // Rien à faire ici pour l'instant
    
    // ⏸️ Phase 1+: PCI, Network, Block, etc.
    // log::info!("Initializing hardware drivers...");
    // pci::init();
    // net::e1000::init();
    // net::rtl8139::init();
    // net::virtio_net::init();
}
